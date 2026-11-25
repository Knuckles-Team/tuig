//! Interactive remote session: state, rendering and the terminal event loop.
//! The impl is split into lifecycle, input and request-action blocks so each
//! stays within the shared method-count limit.
use crate::composer::Composer;
use crate::host_log_view::HostLogView;
use crate::inspector_view::InspectorView;
use crate::presentation::{ThemeSelection, Viewport, accepts_key_event};
use crate::public_details_view::PublicDetailsView;
use crate::task_history_view::TaskHistoryView;
use crate::{
    CANCELLATION_UNKNOWN, Cancellation, OPERATION_REFUSED, PLAN_REFUSED, PLAN_UNAVAILABLE,
    ReadStatus, SUBMISSION_UNKNOWN, confirm_visible, draw_confirmation, draw_task_viewport,
    observation_continues, observation_ready, read_status, request_completed, snapshot_status,
    stop_expired_observation, submission_completed,
};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
    KeyModifiers, MouseEvent,
};
use futures_util::StreamExt;
use ratatui::Frame;
use std::{env, sync::Arc};
use tuig_native::{
    ArboraClient, TaskSnapshot, TaskSubmission, correlation::TaskCorrelation,
    operation_command::OperationCommand, plan_confirm::PlanBinding,
};

pub type ReadRequest = tokio::task::JoinHandle<Result<ReadStatus, &'static str>>;
pub type ReadResult = Result<Result<ReadStatus, &'static str>, tokio::task::JoinError>;

enum LoopAction {
    RequestDone(Box<ReadResult>),
    Observe,
    ViewsTick,
    Input(Option<std::io::Result<Event>>),
    None,
}

enum KeyOutcome {
    Handled,
    Quit,
}

impl KeyOutcome {
    fn quit(self) -> bool {
        matches!(self, Self::Quit)
    }
}

pub struct Session {
    client: Arc<ArboraClient>,
    themes: ThemeSelection,
    submitting: bool,
    automatic_observation: bool,
    can_compose: bool,
    composer: Composer,
    observation_deadline: Option<tokio::time::Instant>,
    task_id: Option<String>,
    pending: bool,
    observation_active: bool,
    status: String,
    binding: Option<TaskCorrelation>,
    latest_task: Option<TaskSnapshot>,
    cancellation: Cancellation,
    remote_cancel: bool,
    submission_unknown: bool,
    observe: bool,
    confirmation_visible: bool,
    viewport: Viewport,
    inspector: InspectorView,
    history: TaskHistoryView,
    logs: HostLogView,
    details: PublicDetailsView,
}

// Session lifecycle, rendering and request results.
impl Session {
    pub fn new(
        client: Arc<ArboraClient>,
        task_id: Option<String>,
        submission: Option<TaskSubmission>,
        observe: bool,
    ) -> (Self, ReadRequest) {
        let theme = env::var("AGENT_THEME").unwrap_or_else(|_| "tokyo-night".to_owned());
        let no_color = env::var("NO_COLOR").unwrap_or_default();
        let themes = ThemeSelection::new(&theme, &no_color);
        let submitting = submission.is_some();
        let automatic_observation = observe || submitting;
        let can_compose = task_id.is_none() && submission.is_none() && !observe;
        let mut composer = Composer::default();
        composer.open = can_compose;
        let observation_deadline = submitting
            .then(|| tokio::time::Instant::now() + std::time::Duration::from_secs(4 * 60 * 60));
        let status = if submitting {
            "Submitting Arbora task..."
        } else if task_id.is_some() {
            "Reading Arbora task..."
        } else {
            "Discovering Arbora..."
        }
        .to_owned();
        let request = tokio::spawn({
            let client = client.clone();
            let task_id = task_id.clone();
            async move {
                match submission {
                    Some(submission) => client.submit_task(&submission).await.map(ReadStatus::Task),
                    None => read_status(&client, task_id.as_deref(), None).await,
                }
            }
        });
        (
            Self {
                client,
                themes,
                submitting,
                automatic_observation,
                can_compose,
                composer,
                observation_deadline,
                task_id,
                pending: true,
                observation_active: observe,
                status,
                binding: None,
                latest_task: None,
                cancellation: Cancellation::default(),
                remote_cancel: false,
                submission_unknown: false,
                observe,
                confirmation_visible: false,
                viewport: Viewport::default(),
                inspector: InspectorView::default(),
                history: TaskHistoryView::default(),
                logs: HostLogView::default(),
                details: PublicDetailsView::default(),
            },
            request,
        )
    }
    fn draw_screen(&mut self, frame: &mut Frame) {
        let style = self.themes.style();
        let border_style = self.themes.border_style();
        self.confirmation_visible = false;
        if self.composer.open {
            self.composer.draw(frame, style, border_style);
        } else if self.cancellation == Cancellation::Confirming {
            self.confirmation_visible = draw_confirmation(frame, &self.status, style, border_style);
        } else if self.logs.is_open() {
            self.logs.draw(frame, style, border_style);
        } else if self.inspector.is_open() {
            self.inspector
                .draw(frame, style, border_style, std::time::Instant::now());
        } else if self.history.is_open() {
            self.history.draw(frame, style, border_style);
        } else {
            draw_task_viewport(
                frame,
                &self.status,
                style,
                border_style,
                &mut self.viewport,
                self.observe,
                &mut self.details,
            );
        }
    }
    fn views_active(&self) -> bool {
        self.inspector.is_open()
            || self.history.is_open()
            || self.logs.is_open()
            || self.details.active()
    }
    fn views_tick(&mut self) {
        self.logs.tick();
        self.details
            .tick(self.client.clone(), std::time::Instant::now());
    }
    fn apply_request_result(&mut self, result: ReadResult) {
        self.observation_active = self.automatic_observation
            && matches!(&result,
                Ok(Ok(ReadStatus::Task(snapshot))) if observation_continues(snapshot, self.binding.as_ref()));
        let accepted = matches!(&result, Ok(Ok(ReadStatus::Task(snapshot)))
            if self.binding.as_ref().is_none_or(|bound: &TaskCorrelation| bound.validate_binding(&snapshot.correlation).is_ok()));
        if let Ok(Ok(ReadStatus::Task(snapshot))) = &result
            && self.binding.as_ref().is_none_or(|bound: &TaskCorrelation| {
                bound.validate_binding(&snapshot.correlation).is_ok()
            })
        {
            self.latest_task = Some(snapshot.clone());
        }
        if self.submitting {
            self.status = submission_completed(result, &mut self.binding, &mut self.cancellation);
            self.task_id = self
                .binding
                .as_ref()
                .map(|bound| bound.task_id().to_owned());
            self.submission_unknown = self.binding.is_none();
            self.submitting = false;
        } else {
            self.status = request_completed(
                result,
                &mut self.binding,
                &mut self.cancellation,
                self.remote_cancel,
            );
            if self.can_compose {
                self.status.push_str("\n\np: return to prompt draft");
            }
        }
        let updated = if accepted {
            self.binding.as_ref()
        } else {
            None
        };
        self.inspector.task_updated(updated);
        self.details.task_updated(updated);
    }
    fn restart_observation_read(&mut self, request: &mut ReadRequest) -> bool {
        if stop_expired_observation(
            self.observation_deadline,
            tokio::time::Instant::now(),
            &mut self.observation_active,
            &mut self.status,
        ) {
            return true;
        }
        let client = self.client.clone();
        let task_id = self.task_id.clone();
        let binding = self.binding.clone();
        *request = tokio::spawn(async move {
            read_status(&client, task_id.as_deref(), binding.as_ref()).await
        });
        self.pending = true;
        self.remote_cancel = false;
        self.cancellation.read_started();
        false
    }
    pub async fn event_loop(
        &mut self,
        client: &Arc<ArboraClient>,
        terminal: &mut ratatui::DefaultTerminal,
        request: &mut ReadRequest,
        events: &mut EventStream,
        observation_tick: &mut tokio::time::Interval,
        redraw: &mut tokio::time::Interval,
    ) -> Result<(), &'static str> {
        loop {
            self.inspector.client_updated(client);
            terminal
                .draw(|frame| self.draw_screen(frame))
                .map_err(|_| "Cannot draw terminal")?;
            let confirmation_visible = self.confirmation_visible;
            let action = wait_event(self, request, events, observation_tick, redraw).await;
            if let LoopAction::RequestDone(result) = action {
                self.pending = false;
                self.apply_request_result(*result);
            } else if let LoopAction::Observe = action
                && self.restart_observation_read(request)
            {
                continue;
            } else if let LoopAction::ViewsTick = action {
                self.views_tick();
            } else if let LoopAction::Input(input) = action
                && self
                    .handle_input(input, confirmation_visible, request)
                    .await?
            {
                return Ok(());
            }
        }
    }
    pub async fn finish(&mut self, request: &mut ReadRequest) {
        request.abort();
        self.inspector.close();
        self.history.close();
        self.logs.close();
        self.details.abort();
        let _ = crossterm::execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture
        );
        ratatui::restore();
        if self.submitting {
            let status =
                submission_completed(request.await, &mut self.binding, &mut self.cancellation);
            eprintln!("{status}");
        } else if self.submission_unknown {
            eprintln!("{SUBMISSION_UNKNOWN}");
        }
    }
}

// Session terminal input.
impl Session {
    fn route_mouse(&mut self, event: MouseEvent) {
        let _ = crate::route_mouse(
            event,
            self.cancellation == Cancellation::Confirming || self.composer.open,
            &mut self.logs,
            &mut self.inspector,
            &mut self.viewport,
            &mut self.details,
            &mut self.history,
        );
    }
    fn shortcut_handled(&mut self, key: KeyEvent) -> bool {
        if self.open_composer(key.code) {
            return true;
        }
        self.cancellation != Cancellation::Confirming
            && !self.logs.is_open()
            && self.inspector.search_key(key)
    }
    fn open_composer(&mut self, key: KeyCode) -> bool {
        if key != KeyCode::Char('p')
            || !self.can_compose
            || self.logs.is_open()
            || self.inspector.is_open()
            || self.history.is_open()
        {
            return false;
        }
        self.composer.open = true;
        true
    }
    fn panel_shortcut(&mut self, key: KeyEvent) -> bool {
        if self.cancellation == Cancellation::Confirming {
            return false;
        }
        if self.logs.key(key.code) {
            return true;
        }
        if self.open_logs(key) {
            return true;
        }
        if self.inspector.key(key.code, self.client.clone()) {
            return true;
        }
        if self.history.key(key.code, self.client.clone()) {
            return true;
        }
        if key.code == KeyCode::Char('i') && !self.pending {
            self.inspector.open(self.client.clone());
            return true;
        }
        self.details.key(key) || self.viewport.handle(key.code)
    }
    fn open_logs(&mut self, key: KeyEvent) -> bool {
        let requested = key.code == KeyCode::Char('l')
            || (key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL));
        if !requested || self.inspector.is_open() {
            return false;
        }
        self.logs.open(self.client.clone());
        true
    }
    async fn action_key(
        &mut self,
        key: KeyCode,
        confirmation_visible: bool,
        request: &mut ReadRequest,
    ) -> bool {
        if key == KeyCode::Char('c') && self.pending {
            self.cancel_pending(request).await;
            return true;
        }
        if key == KeyCode::Char('c') {
            return self.cancel_details();
        }
        if key == KeyCode::Char('r') {
            self.refresh_read(request);
            return true;
        }
        self.cancellation_shortcut(request, key, confirmation_visible)
    }
    async fn handle_event(
        &mut self,
        event: Event,
        confirmation_visible: bool,
        request: &mut ReadRequest,
    ) -> Result<KeyOutcome, &'static str> {
        if let Event::Key(key) = event {
            return self.key_event(key, confirmation_visible, request).await;
        }
        if let Event::Paste(text) = event {
            if self.composer.open {
                self.composer.paste(&text);
            }
            return Ok(KeyOutcome::Handled);
        }
        if let Event::Mouse(mouse) = event {
            self.route_mouse(mouse);
            return Ok(KeyOutcome::Handled);
        }
        Ok(KeyOutcome::Handled)
    }
    async fn key_event(
        &mut self,
        key: KeyEvent,
        confirmation_visible: bool,
        request: &mut ReadRequest,
    ) -> Result<KeyOutcome, &'static str> {
        if self.themes.key(key) || !accepts_key_event(key) {
            return Ok(KeyOutcome::Handled);
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(KeyOutcome::Quit);
        }
        if self.composer.open {
            self.composer_key(key, request)?;
            return Ok(KeyOutcome::Handled);
        }
        Ok(self.terminal_key(key, confirmation_visible, request).await)
    }
    async fn terminal_key(
        &mut self,
        key: KeyEvent,
        confirmation_visible: bool,
        request: &mut ReadRequest,
    ) -> KeyOutcome {
        if self.shortcut_handled(key) {
            return KeyOutcome::Handled;
        }
        if key.code == KeyCode::Char('q')
            || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return KeyOutcome::Quit;
        }
        if self.panel_shortcut(key) {
            return KeyOutcome::Handled;
        }
        if self
            .action_key(key.code, confirmation_visible, request)
            .await
        {
            return KeyOutcome::Handled;
        }
        KeyOutcome::Handled
    }
    async fn handle_input(
        &mut self,
        input: Option<std::io::Result<Event>>,
        confirmation_visible: bool,
        request: &mut ReadRequest,
    ) -> Result<bool, &'static str> {
        let Some(Ok(event)) = input else {
            return Err("Terminal input unavailable");
        };
        Ok(self
            .handle_event(event, confirmation_visible, request)
            .await?
            .quit())
    }
}

// Session request actions.
impl Session {
    fn spawn_operation(&mut self, request: &mut ReadRequest, command: &OperationCommand) {
        request.abort();
        let client = self.client.clone();
        let operation = command.operation().to_owned();
        let payload = command.payload().to_owned();
        self.status = format!("Invoking {operation}...");
        *request = tokio::spawn(async move {
            client
                .invoke_operation(&operation, &payload)
                .await
                .map(|value| ReadStatus::Operation(format!("Operation {operation}:\n{value}")))
                .map_err(|_| OPERATION_REFUSED)
        });
        self.pending = true;
    }
    fn spawn_plan_confirmation(
        &mut self,
        request: &mut ReadRequest,
        plan: PlanBinding,
        binding: TaskCorrelation,
    ) {
        let client = self.client.clone();
        self.status = format!("Confirming plan {}...", plan.reference());
        *request = tokio::spawn(async move {
            client
                .confirm_plan(&binding, &plan)
                .await
                .map(|snapshot| {
                    ReadStatus::Operation(format!(
                        "Plan {} confirmed by Arbora\n\n{}",
                        plan.reference(),
                        snapshot_status(&snapshot),
                    ))
                })
                .map_err(|_| PLAN_REFUSED)
        });
        self.pending = true;
    }
    fn composer_key(
        &mut self,
        key: KeyEvent,
        request: &mut ReadRequest,
    ) -> Result<(), &'static str> {
        let Some(text) = self.composer.key(key) else {
            return Ok(());
        };
        self.composer.open = false;
        if let Some(command) = OperationCommand::parse_command(&text) {
            self.spawn_operation(request, &command);
            return Ok(());
        }
        if text.trim() == "/confirm" {
            request.abort();
            let issued = self.latest_task.as_ref().and_then(|task| task.plan.clone());
            match (issued, self.binding.clone()) {
                (Some(plan), Some(binding)) => self.spawn_plan_confirmation(request, plan, binding),
                _ => self.status = PLAN_UNAVAILABLE.to_owned(),
            }
            return Ok(());
        }
        if text.trim() == "/history" {
            self.history.open(self.client.clone());
            return Ok(());
        }
        let submission = TaskSubmission::new(&text)?;
        request.abort();
        let client = self.client.clone();
        *request =
            tokio::spawn(
                async move { client.submit_task(&submission).await.map(ReadStatus::Task) },
            );
        self.can_compose = false;
        self.submitting = true;
        self.pending = true;
        self.automatic_observation = true;
        self.observation_deadline =
            Some(tokio::time::Instant::now() + std::time::Duration::from_secs(4 * 60 * 60));
        self.status = "Submitting Arbora task...".to_owned();
        Ok(())
    }
    async fn cancel_pending(&mut self, request: &mut ReadRequest) {
        self.observation_active = false;
        request.abort();
        self.inspector.task_updated(None);
        self.details.task_updated(None);
        self.pending = false;
        let result = request.await;
        self.finish_canceled_request(result);
    }
    fn finish_canceled_request(&mut self, result: ReadResult) {
        if self.submitting {
            self.status = submission_completed(result, &mut self.binding, &mut self.cancellation);
            self.task_id = self
                .binding
                .as_ref()
                .map(|bound| bound.task_id().to_owned());
            self.submission_unknown = self.binding.is_none();
            self.submitting = false;
        } else if self.remote_cancel {
            self.cancellation = Cancellation::Unknown;
            self.status = CANCELLATION_UNKNOWN.to_owned();
        } else {
            self.status =
                request_completed(result, &mut self.binding, &mut self.cancellation, false);
        }
    }
    fn cancel_details(&mut self) -> bool {
        if !self.details.pending() {
            return false;
        }
        self.details.cancel_read();
        true
    }
    fn refresh_read(&mut self, request: &mut ReadRequest) {
        if self.pending || self.submission_unknown {
            return;
        }
        self.details.replay();
        self.status = if self.task_id.is_some() {
            "Reading Arbora task..."
        } else {
            "Discovering Arbora..."
        }
        .to_owned();
        let client = self.client.clone();
        let task_id = self.task_id.clone();
        let binding = self.binding.clone();
        *request = tokio::spawn(async move {
            read_status(&client, task_id.as_deref(), binding.as_ref()).await
        });
        self.pending = true;
        self.remote_cancel = false;
        self.cancellation.read_started();
    }
    fn cancellation_shortcut(
        &mut self,
        request: &mut ReadRequest,
        key: KeyCode,
        confirmation_visible: bool,
    ) -> bool {
        match key {
            KeyCode::Char('x') if !self.pending && !self.observe => {
                self.cancellation.arm();
                true
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.cancellation.dismiss();
                true
            }
            KeyCode::Char('y') if self.cancel_confirmed(confirmation_visible) => {
                self.start_remote_cancel(request);
                true
            }
            _ => false,
        }
    }

    fn cancel_confirmed(&mut self, confirmation_visible: bool) -> bool {
        !self.observe
            && !self.pending
            && confirm_visible(
                &mut self.cancellation,
                confirmation_visible,
                crossterm::terminal::size(),
            )
    }

    fn start_remote_cancel(&mut self, request: &mut ReadRequest) {
        if let Some(binding) = self.binding.clone() {
            let client = self.client.clone();
            *request =
                tokio::spawn(
                    async move { client.cancel_task(&binding).await.map(ReadStatus::Task) },
                );
            self.pending = true;
            self.remote_cancel = true;
            self.status = "Canceling remote task...".to_owned();
        }
    }
}

async fn wait_event(
    session: &mut Session,
    request: &mut ReadRequest,
    events: &mut EventStream,
    observation_tick: &mut tokio::time::Interval,
    redraw: &mut tokio::time::Interval,
) -> LoopAction {
    tokio::select! {
        result = request, if session.pending => LoopAction::RequestDone(Box::new(result)),
        _ = observation_tick.tick(), if observation_ready(session.observation_active, session.pending, session.cancellation) => LoopAction::Observe,
        _ = session.inspector.complete(), if session.inspector.pending() => LoopAction::None,
        _ = session.history.complete(), if session.history.pending() => LoopAction::None,
        _ = session.logs.complete(), if session.logs.pending() => LoopAction::None,
        _ = session.details.complete(), if session.details.pending() => LoopAction::None,
        _ = redraw.tick(), if session.views_active() => LoopAction::ViewsTick,
        event = events.next() => LoopAction::Input(event),
    }
}
