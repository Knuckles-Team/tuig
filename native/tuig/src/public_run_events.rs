//! The versioned public projection, never opaque producer payloads or thoughts.
use crate::{ArboraClient, run_inspector::ReadRefusal};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

pub const SCHEMA: &str = "arbora-run-events-v1";
pub const MAX_EVENTS: usize = 1000;
pub const MAX_VIEW_BYTES: usize = 131_072;
const MAX_SEQUENCE: u64 = i64::MAX as u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicEvent {
    pub event_id: String,
    pub sequence: u64,
    pub summary: Option<String>,
    pub text: String,
    pub terminal: bool,
}

#[derive(Debug)]
pub struct PublicPage {
    pub events: Vec<PublicEvent>,
    pub next_after: u64,
    pub has_more: bool,
    pub terminal: bool,
}

pub fn literal(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            (!c.is_control() || matches!(c, '\n' | '\t'))
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

fn identifier(value: &Value, maximum: usize) -> Option<&str> {
    value.as_str().filter(|s| {
        !s.trim().is_empty()
            && s.chars().count() <= maximum
            && !s.chars().any(char::is_control)
            && literal(s) == *s
    })
}

impl PublicEvent {
    fn parse(value: &Value, run_id: &str) -> Result<Self, ReadRefusal> {
        let (sequence, event_id, task_id, kind) = Self::header(value, run_id)?;
        let payload = value["payload"]
            .as_object()
            .ok_or(ReadRefusal::Unavailable)?;
        let summary = Self::summary(payload, kind == "public_summary")?;
        let text = match &summary {
            Some(summary) => summary.clone(),
            None => Self::trace_text(sequence, kind, task_id, payload)?,
        };
        Ok(Self {
            event_id: event_id.to_owned(),
            sequence,
            summary,
            text,
            terminal: Self::terminal(kind),
        })
    }

    fn terminal(kind: &str) -> bool {
        matches!(kind, "graph_complete" | "error" | "run_cancelled")
    }

    fn header<'a>(
        value: &'a Value,
        run_id: &str,
    ) -> Result<(u64, &'a str, &'a str, &'a str), ReadRefusal> {
        let sequence = value["sequence"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= MAX_SEQUENCE)
            .ok_or(ReadRefusal::Unavailable)?;
        let event_id = identifier(&value["event_id"], 1024).ok_or(ReadRefusal::Unavailable)?;
        let task_id = identifier(&value["task_id"], 512).ok_or(ReadRefusal::Unavailable)?;
        let kind = value["type"].as_str().ok_or(ReadRefusal::Unavailable)?;
        if !Self::header_ok(value, run_id, kind) {
            return Err(ReadRefusal::Unavailable);
        }
        Ok((sequence, event_id, task_id, kind))
    }

    fn header_ok(value: &Value, run_id: &str, kind: &str) -> bool {
        value["schema_version"].as_str() == Some(SCHEMA)
            && value["run_id"].as_str() == Some(run_id)
            && value["terminal"].as_bool() == Some(Self::terminal(kind))
            && value["category"].as_str() == Some(Self::category(kind == "public_summary"))
            && Self::known_kind(kind)
    }

    fn category(is_summary: bool) -> &'static str {
        if is_summary {
            "public_summary"
        } else {
            "execution_trace"
        }
    }

    fn known_kind(kind: &str) -> bool {
        matches!(
            kind,
            "run_started"
                | "node_transition"
                | "graph_complete"
                | "error"
                | "run_cancelled"
                | "task_started"
                | "task_completed"
                | "task_failed"
                | "task_cancelled"
                | "task_blocked"
                | "public_summary"
        )
    }

    fn summary(
        payload: &Map<String, Value>,
        is_summary: bool,
    ) -> Result<Option<String>, ReadRefusal> {
        if !is_summary {
            return Ok(None);
        }
        let summary = payload
            .get("summary")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.chars().count() <= 16_384)
            .ok_or(ReadRefusal::Unavailable)?;
        Ok(Some(literal(summary)))
    }

    fn trace_text(
        sequence: u64,
        kind: &str,
        task_id: &str,
        payload: &Map<String, Value>,
    ) -> Result<String, ReadRefusal> {
        let mut fields = vec![format!("{sequence}. {kind} · task {task_id}")];
        for key in ["status", "node", "from_node", "to_node", "error_type"] {
            if let Some(field) = payload.get(key) {
                let text = field
                    .as_str()
                    .filter(|s| s.chars().count() <= 256)
                    .ok_or(ReadRefusal::Unavailable)?;
                fields.push(format!("{key}: {}", literal(text)));
            }
        }
        if let Some(field) = payload.get("step") {
            let step = field
                .as_u64()
                .filter(|n| *n <= MAX_SEQUENCE)
                .ok_or(ReadRefusal::Unavailable)?;
            fields.push(format!("step: {step}"));
        }
        if let Some(field) = payload.get("depends_on") {
            fields.push(format!("depends_on: {}", Self::depends_on(field)?));
        }
        Ok(fields.join("\n"))
    }

    fn depends_on(field: &Value) -> Result<String, ReadRefusal> {
        let items = field
            .as_array()
            .filter(|items| items.len() <= 64)
            .ok_or(ReadRefusal::Unavailable)?;
        let dependencies: Result<Vec<&str>, _> = items
            .iter()
            .map(|item| identifier(item, 512).ok_or(ReadRefusal::Unavailable))
            .collect();
        Ok(dependencies?.join(", "))
    }
}

impl PublicPage {
    pub fn parse(value: &Value, run_id: &str, after: u64) -> Result<Self, ReadRefusal> {
        if !Self::frame_ok(value, run_id, after) {
            return Err(ReadRefusal::Unavailable);
        }
        let raw = value["events"]
            .as_array()
            .filter(|events| events.len() <= MAX_EVENTS)
            .ok_or(ReadRefusal::Unavailable)?;
        let events = Self::ordered_events(raw, run_id, after)?;
        let next_after = events.last().map_or(after, |event| event.sequence);
        let terminal = Self::terminal_flag(&events, after, value);
        let has_more = value["has_more"]
            .as_bool()
            .ok_or(ReadRefusal::Unavailable)?;
        if !Self::cursor_ok(value, has_more, &events, terminal, next_after) {
            return Err(ReadRefusal::Unavailable);
        }
        Ok(Self {
            events,
            next_after,
            has_more,
            terminal,
        })
    }

    fn frame_ok(value: &Value, run_id: &str, after: u64) -> bool {
        after <= MAX_SEQUENCE
            && identifier(&json!(run_id), 512).is_some()
            && value["schema_version"].as_str() == Some(SCHEMA)
            && value["run_id"].as_str() == Some(run_id)
            && value["availability"].as_str() == Some("available")
            && value["after"].as_u64() == Some(after)
            && value.to_string().len() <= 1_048_576
    }

    fn ordered_events(
        raw: &[Value],
        run_id: &str,
        after: u64,
    ) -> Result<Vec<PublicEvent>, ReadRefusal> {
        let mut events: Vec<PublicEvent> = Vec::new();
        let mut ids = HashSet::new();
        for (index, raw) in raw.iter().enumerate() {
            let event = PublicEvent::parse(raw, run_id)?;
            if Self::rejects(&events, &event, &mut ids, after, index) {
                return Err(ReadRefusal::Unavailable);
            }
            events.push(event);
        }
        Ok(events)
    }

    fn rejects(
        events: &[PublicEvent],
        event: &PublicEvent,
        ids: &mut HashSet<String>,
        after: u64,
        index: usize,
    ) -> bool {
        Some(event.sequence) != after.checked_add(index as u64 + 1)
            || !ids.insert(event.event_id.clone())
            || events.last().is_some_and(|event| event.terminal)
    }

    fn terminal_flag(events: &[PublicEvent], after: u64, value: &Value) -> bool {
        events.last().is_some_and(|event| event.terminal)
            || (events.is_empty() && after > 0 && value["terminal"].as_bool() == Some(true))
    }

    fn cursor_ok(
        value: &Value,
        has_more: bool,
        events: &[PublicEvent],
        terminal: bool,
        next_after: u64,
    ) -> bool {
        value["next_after"].as_u64() == Some(next_after)
            && value["terminal"].as_bool() == Some(terminal)
            && !(has_more && (events.is_empty() || terminal))
    }
}

impl ArboraClient {
    pub async fn get_public_run_events(
        &self,
        run_id: &str,
        task_id: &str,
        after: u64,
    ) -> Result<PublicPage, ReadRefusal> {
        if after > MAX_SEQUENCE || identifier(&json!(task_id), 512).is_none() {
            return Err(ReadRefusal::Unavailable);
        }
        let value = self
            .read_run_operation(
                "runs.events",
                run_id,
                json!({"run_id": run_id, "task_id": task_id, "after": after, "limit": MAX_EVENTS}),
            )
            .await?;
        PublicPage::parse(&value, run_id, after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/public_run_events.json"
        ))
        .unwrap()
    }

    // spec: TUIG-RUNTIME-R008.2.3.2.2.1
    #[test]
    fn shared_fixture_renders_public_summary_separately_and_ignores_private_fields() {
        let mut value = fixture();
        for event in value["events"].as_array_mut().unwrap() {
            event["reasoning"] = json!("private sentinel");
            event["payload"]["thought"] = json!("private sentinel");
        }
        value["events"][1]["payload"]["summary"] =
            json!("[bold]public[/bold]\u{001b}\u{0007}\u{202e}\u{2066}");
        let page = PublicPage::parse(&value, "run-1", 0).unwrap();
        assert_eq!(
            page.events[1].summary.as_deref(),
            Some("[bold]public[/bold]")
        );
        assert!(!format!("{page:?}").contains("private sentinel"));
        assert!(page.terminal);
        assert_eq!(page.next_after, 5);
    }

    // spec: TUIG-RUNTIME-R008.2.3.2.2.1
    #[test]
    fn resumed_empty_terminal_page_uses_authoritative_cursor_without_invented_events() {
        let mut value = fixture();
        value["events"] = json!([]);
        value["after"] = json!(5);
        let page = PublicPage::parse(&value, "run-1", 5).unwrap();
        assert!(page.events.is_empty() && page.terminal && !page.has_more);
        assert_eq!(page.next_after, 5);
        value["has_more"] = json!(true);
        assert!(PublicPage::parse(&value, "run-1", 5).is_err());
        value["has_more"] = json!(false);
        value["after"] = json!(0);
        value["next_after"] = json!(0);
        assert!(PublicPage::parse(&value, "run-1", 0).is_err());
    }

    // spec: TUIG-RUNTIME-R008.2.3.2.2.1
    #[test]
    fn malformed_identity_order_cursor_category_and_terminal_are_refused() {
        for (pointer, change) in [
            ("/run_id", json!("foreign")),
            ("/schema_version", json!("unknown")),
            ("/after", json!(true)),
            ("/next_after", json!(6)),
            ("/terminal", json!(false)),
            ("/has_more", json!(true)),
            ("/events/0/run_id", json!("foreign")),
            ("/events/0/sequence", json!(true)),
            ("/events/0/sequence", json!(2)),
            ("/events/0/type", json!("reasoning")),
            ("/events/0/category", json!("public_summary")),
            ("/events/1/event_id", json!("public-event-1")),
            ("/events/1/payload/summary", json!("x".repeat(16_385))),
        ] {
            let mut value = fixture();
            *value.pointer_mut(pointer).unwrap() = change;
            assert!(PublicPage::parse(&value, "run-1", 0).is_err(), "{pointer}");
        }
    }
}
