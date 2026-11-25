//! Read-only presentation boundary for TUIG-RUNTIME-R010.
//! Inputs are normalized by an authorized Arbora adapter, not wire models.
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Paragraph, Wrap},
};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub text: String,
    pub status: PlanStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanSnapshot {
    pub run_id: String,
    pub revision: String,
    pub observed_at: String,
    pub items: Vec<PlanItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanRefusal {
    Unavailable,
    WrongRun,
    Expired,
    Regressing,
    Malformed,
}

/// Interprets the producer's revision and expiry contract. No implementation is
/// supplied until AU publishes that contract and Arbora serves it.
pub trait PlanValidator {
    fn validate(
        &self,
        plan: &PlanSnapshot,
        previous_revision: Option<&str>,
        now: Instant,
    ) -> Result<Instant, PlanRefusal>;
}

#[derive(Clone, Debug, Default)]
pub struct AppliedConfig {
    pub reference: Option<String>,
    pub context_policy: Option<String>,
    pub plan_scaffold: Option<String>,
    pub action_space: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunProjection {
    pub run_id: String,
    pub summary_available: bool,
    pub peak_context_tokens: Option<u64>,
    pub context_budget_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub termination_reason: Option<String>,
    pub config: AppliedConfig,
    pub current_plan: Option<PlanSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadRefusal {
    Unavailable,
    Denied,
    Timeout,
    WrongRun,
}

pub struct RunInspector {
    run_id: String,
    projection: Option<RunProjection>,
    plan_deadline: Option<Instant>,
    // Retain only the comparison token after denial; never render it or old items.
    previous_revision: Option<String>,
}

impl RunInspector {
    pub fn new(run_id: String) -> Self {
        Self {
            run_id,
            projection: None,
            plan_deadline: None,
            previous_revision: None,
        }
    }

    pub fn select_run(&mut self, run_id: String) {
        *self = Self::new(run_id);
    }

    pub fn accept(
        &mut self,
        result: Result<RunProjection, ReadRefusal>,
        validator: Option<&dyn PlanValidator>,
        now: Instant,
    ) -> Result<(), ReadRefusal> {
        self.projection = None;
        self.plan_deadline = None;
        let mut projection = result?;
        if projection.run_id != self.run_id {
            return Err(ReadRefusal::WrongRun);
        }
        Self::sanitize_cost(&mut projection);
        self.apply_plan(&mut projection, validator, now);
        self.projection = Some(projection);
        Ok(())
    }

    fn sanitize_cost(projection: &mut RunProjection) {
        if projection
            .cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            projection.cost_usd = None;
        }
    }

    fn apply_plan(
        &mut self,
        projection: &mut RunProjection,
        validator: Option<&dyn PlanValidator>,
        now: Instant,
    ) {
        if let Some(plan) = projection.current_plan.take() {
            let validated = plan_validation(
                &plan,
                &self.run_id,
                self.previous_revision.as_deref(),
                validator,
                now,
            );
            if let Ok(deadline) = validated
                && deadline > now
            {
                self.previous_revision = Some(plan.revision.clone());
                self.plan_deadline = Some(deadline);
                projection.current_plan = Some(plan);
            }
        }
    }

    pub fn text(&mut self, now: Instant) -> String {
        self.expire_plan(now);
        let mut lines = vec![format!("Run: {}", literal(&self.run_id))];
        let Some(run) = &self.projection else {
            lines.push("Run inspector unavailable".into());
            return lines.join("\n");
        };
        if !run.summary_available {
            lines.push("Run summary unavailable; current-plan read only".into());
        }
        lines.extend(Self::run_lines(run));
        lines.extend(Self::config_lines(&run.config));
        match &run.current_plan {
            None => lines.push("Current plan: unavailable".into()),
            Some(plan) => lines.extend(Self::plan_lines(plan)),
        }
        lines.join("\n")
    }

    fn expire_plan(&mut self, now: Instant) {
        if self.plan_deadline.is_some_and(|deadline| deadline <= now) {
            if let Some(projection) = self.projection.as_mut() {
                projection.current_plan = None;
            }
            self.plan_deadline = None;
        }
    }

    fn run_lines(run: &RunProjection) -> Vec<String> {
        vec![
            format!("Peak context tokens: {}", number(run.peak_context_tokens)),
            format!(
                "Context budget tokens: {}",
                number(run.context_budget_tokens)
            ),
            format!(
                "Per-run cost (USD): {}",
                run.cost_usd
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unavailable".into())
            ),
            format!(
                "Termination reason: {}",
                terminal_reason(run.termination_reason.as_deref())
            ),
            format!(
                "Applied configuration reference: {}",
                optional(&run.config.reference)
            ),
        ]
    }

    fn config_lines(config: &AppliedConfig) -> Vec<String> {
        // Values without a recorded decision reference cannot be called applied.
        let config = if config
            .reference
            .as_ref()
            .is_some_and(|reference| !reference.is_empty())
        {
            config.clone()
        } else {
            AppliedConfig::default()
        };
        vec![
            format!("Context policy: {}", optional(&config.context_policy)),
            format!("Plan scaffold: {}", optional(&config.plan_scaffold)),
            format!("Action space: {}", optional(&config.action_space)),
        ]
    }

    fn plan_lines(plan: &PlanSnapshot) -> Vec<String> {
        let mut lines = vec![
            format!("Current plan revision: {}", literal(&plan.revision)),
            format!("Observed at: {}", literal(&plan.observed_at)),
        ];
        if plan.items.is_empty() {
            lines.push("Current plan: available (empty)".into());
        }
        for item in &plan.items {
            let status = match item.status {
                PlanStatus::Pending => "pending",
                PlanStatus::InProgress => "in_progress",
                PlanStatus::Done => "done",
            };
            lines.push(format!("[{status}] {}", literal(&item.text)));
        }
        lines
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, now: Instant) {
        frame.render_widget(
            Paragraph::new(self.text(now))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title("Run inspector (read-only)")),
            area,
        );
    }
}

fn plan_validation(
    plan: &PlanSnapshot,
    run_id: &str,
    previous_revision: Option<&str>,
    validator: Option<&dyn PlanValidator>,
    now: Instant,
) -> Result<Instant, PlanRefusal> {
    if plan.run_id != run_id {
        return Err(PlanRefusal::WrongRun);
    }
    if plan.revision.is_empty() || plan.observed_at.is_empty() {
        return Err(PlanRefusal::Malformed);
    }
    validator
        .ok_or(PlanRefusal::Unavailable)
        .and_then(|validator| validator.validate(plan, previous_revision, now))
}

fn number(value: Option<u64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unavailable".into())
}
fn optional(value: &Option<String>) -> String {
    value
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(|s| literal(s))
        .unwrap_or_else(|| "unavailable".into())
}
fn literal(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}
pub fn terminal_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("final_answer") => "Final answer",
        Some("context_overflow") => "Context overflow",
        Some("stuck") => "Stuck",
        Some("step_budget") => "Step budget",
        Some("error") => "Error",
        _ => "unavailable",
    }
}
