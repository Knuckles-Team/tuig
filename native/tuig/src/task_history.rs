//! Caller task history: bounded `tasks/list` pages over opaque navigation cursors.
//!
//! The terminal never resumes a conversation from history: it only rereads
//! pages Arbora serves. P rereads the previous page with the original opaque
//! cursor that fetched it, N reads the page the current page issued, and R
//! resets the path to the first server page. A failed read retains the
//! navigation position for retry, a cyclic continuation is refused before it
//! advances, duplicate navigation is ignored while a read is pending, and a
//! late completion after close is discarded.
use crate::{
    ArboraClient, MAX_RESPONSE_BYTES,
    correlation::{TaskCorrelation, TaskEvent, TaskState},
    public_run_events::literal,
};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

pub const PAGE_LIMIT: usize = 50;
const MAX_TASKS: usize = 100;
const MAX_CURSOR_CHARS: usize = 1024;
const MAX_ARTIFACTS: usize = 32;
const MAX_PARTS: usize = 32;
const MAX_PART_BYTES: usize = 65_536;
const MAX_REPLY_CHARS: usize = 16_384;
const REQUEST_ID: &str = "task-history";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub correlation: TaskCorrelation,
    pub state: TaskState,
    /// The only reply the page carries: the first text artifact, or none.
    pub reply: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPage {
    pub tasks: Vec<TaskEntry>,
    /// The opaque continuation for the next page; none at the end of the list.
    pub next_cursor: Option<String>,
}

fn opaque_cursor(value: &Value) -> Result<Option<String>, &'static str> {
    match value {
        Value::Null => Ok(None),
        Value::String(cursor) => {
            let characters = cursor.chars().count();
            if characters == 0
                || characters > MAX_CURSOR_CHARS
                || cursor.chars().any(char::is_control)
            {
                return Err("Invalid task history cursor");
            }
            Ok(Some(cursor.clone()))
        }
        _ => Err("Invalid task history cursor"),
    }
}

fn reply(artifacts: Option<&Value>) -> Result<Option<String>, &'static str> {
    let Some(artifacts) = artifacts.and_then(Value::as_array) else {
        return Ok(None);
    };
    if artifacts.len() > MAX_ARTIFACTS {
        return Err("Invalid task history reply");
    }
    let mut reply = None;
    for (index, artifact) in artifacts.iter().enumerate() {
        artifact
            .get("artifactId")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty() && id.len() <= 512)
            .ok_or("Invalid task history reply")?;
        let parts = artifact
            .get("parts")
            .and_then(Value::as_array)
            .filter(|parts| !parts.is_empty() && parts.len() <= MAX_PARTS)
            .ok_or("Invalid task history reply")?;
        if index != 0 {
            continue;
        }
        let mut text = String::new();
        for part in parts {
            if part.get("kind").and_then(Value::as_str) != Some("text") {
                return Err("Invalid task history reply");
            }
            let chunk = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty() && text.len() <= MAX_PART_BYTES)
                .ok_or("Invalid task history reply")?;
            text.push_str(chunk);
        }
        reply = Some(literal(&text).chars().take(MAX_REPLY_CHARS).collect());
    }
    Ok(reply)
}

impl TaskPage {
    pub fn parse(value: &Value) -> Result<Self, &'static str> {
        let next_cursor = opaque_cursor(&value["nextCursor"])?;
        let raw = value
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or("Invalid task history page")?;
        if raw.len() > MAX_TASKS {
            return Err("Invalid task history page");
        }
        let mut tasks = Vec::new();
        let mut seen = HashSet::new();
        for task in raw {
            let correlation = TaskCorrelation::from_task(task)?;
            let TaskEvent::Status { state, .. } = correlation.validate(task)? else {
                return Err("Invalid task history page");
            };
            if !seen.insert(correlation.task_id().to_owned()) {
                return Err("Invalid task history page");
            }
            tasks.push(TaskEntry {
                correlation,
                state,
                reply: reply(task.get("artifacts"))?,
            });
        }
        Ok(Self { tasks, next_cursor })
    }
}

impl ArboraClient {
    /// One bounded page of the caller's task history through `tasks/list`.
    pub async fn list_tasks(&self, cursor: Option<&str>) -> Result<TaskPage, &'static str> {
        if cursor.is_some_and(|cursor| {
            cursor.trim().is_empty()
                || cursor.chars().count() > MAX_CURSOR_CHARS
                || cursor.chars().any(char::is_control)
        }) {
            return Err("Invalid task history cursor");
        }
        let mut url = self.card_url.clone();
        url.set_path("/a2a");
        let mut response = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": REQUEST_ID,
                "method": "tasks/list",
                "params": {"cursor": cursor, "limit": PAGE_LIMIT},
            }))
            .send()
            .await
            .map_err(|_| "Arbora task history read interrupted")?;
        if !response.status().is_success() {
            return Err("Arbora task history read refused or unavailable");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Arbora task history read interrupted")?
        {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err("Arbora task history page exceeds size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let envelope: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Arbora task history response")?;
        if envelope.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || envelope.get("id").and_then(Value::as_str) != Some(REQUEST_ID)
        {
            return Err("Invalid Arbora task history response");
        }
        if envelope.get("error").is_some() {
            return Err("Arbora task history read refused or unavailable");
        }
        let result = envelope
            .get("result")
            .ok_or("Invalid Arbora task history response")?;
        TaskPage::parse(result)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryStatus {
    /// A page read is in flight; rows and replies are cleared.
    #[default]
    Reading,
    /// The current page is on display, possibly empty.
    Page,
    /// The last read was refused; the navigation position is retained.
    Refused,
}

type PageRead = tokio::task::JoinHandle<(HistoryScope, Result<TaskPage, &'static str>)>;

#[derive(Clone)]
struct HistoryScope {
    client: Arc<ArboraClient>,
    generation: u64,
    /// The navigation path this read attempts; the position on success.
    path: Vec<Option<String>>,
}

#[derive(Default)]
pub struct TaskHistory {
    open: bool,
    generation: u64,
    client: Option<Arc<ArboraClient>>,
    request: Option<PageRead>,
    /// The original opaque cursor that fetched each successfully read page.
    path: Vec<Option<String>>,
    /// The continuation the last successful page issued.
    next_cursor: Option<String>,
    /// The only page content the terminal keeps.
    page: Option<TaskPage>,
    status: HistoryStatus,
}

impl TaskHistory {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn pending(&self) -> bool {
        self.request.is_some()
    }
    pub fn status(&self) -> HistoryStatus {
        self.status
    }
    pub fn page(&self) -> Option<&TaskPage> {
        self.page.as_ref()
    }
    /// The navigation position: one original cursor per successfully read page.
    pub fn path(&self) -> &[Option<String>] {
        &self.path
    }
    /// The continuation of the current position, for the next page.
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    /// Open the history on the first server page.
    pub fn open(&mut self, client: Arc<ArboraClient>) {
        self.open = true;
        self.begin_read(client, vec![None]);
    }

    /// P: reread the previous page with the original cursor that fetched it.
    pub fn previous(&mut self, client: Arc<ArboraClient>) {
        if self.path.len() < 2 {
            return;
        }
        let path = self.path[..self.path.len() - 1].to_vec();
        self.begin_read(client, path);
    }

    /// N: read the next page with the cursor the current page issued.
    pub fn next(&mut self, client: Arc<ArboraClient>) {
        let Some(cursor) = self.next_cursor.clone() else {
            return;
        };
        if self
            .path
            .iter()
            .any(|seen| seen.as_deref() == Some(cursor.as_str()))
        {
            return;
        }
        let mut path = self.path.clone();
        path.push(Some(cursor));
        self.begin_read(client, path);
    }

    /// R: reset the navigation path to the first server page.
    pub fn reset(&mut self, client: Arc<ArboraClient>) {
        self.begin_read(client, vec![None]);
    }

    /// Close: invalidate the pending read, including its late completion.
    pub fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(request) = self.request.take() {
            request.abort();
        }
        self.clear_content();
        self.open = false;
        self.path.clear();
        self.next_cursor = None;
    }

    /// Duplicate navigation while a read is pending is ignored.
    fn begin_read(&mut self, client: Arc<ArboraClient>, path: Vec<Option<String>>) {
        self.client = Some(client.clone());
        if !self.open || self.request.is_some() {
            return;
        }
        self.clear_content();
        let generation = self.generation;
        self.request = Some(tokio::spawn(async move {
            let cursor = path.last().cloned().flatten();
            let page = client.list_tasks(cursor.as_deref()).await;
            (
                HistoryScope {
                    client,
                    generation,
                    path,
                },
                page,
            )
        }));
    }

    /// Clear rows and replies during a read and after a failure.
    fn clear_content(&mut self) {
        self.page = None;
        self.status = HistoryStatus::Reading;
    }

    /// Await the in-flight read; a late completion after close is discarded.
    pub async fn complete(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        let Ok((scope, result)) = request.await else {
            return;
        };
        if !self.open
            || scope.generation != self.generation
            || !self
                .client
                .as_ref()
                .is_some_and(|client| Arc::ptr_eq(client, &scope.client))
        {
            return;
        }
        match result {
            Ok(page) => {
                if page.next_cursor.as_ref().is_some_and(|continuation| {
                    scope
                        .path
                        .iter()
                        .any(|seen| seen.as_deref() == Some(continuation.as_str()))
                }) {
                    // The server continued to a page already on the path.
                    self.status = HistoryStatus::Refused;
                } else {
                    self.path = scope.path;
                    self.next_cursor = page.next_cursor.clone();
                    self.page = Some(page);
                    self.status = HistoryStatus::Page;
                }
            }
            Err(_) => self.status = HistoryStatus::Refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(number: u64, state: &str) -> Value {
        json!({
            "kind": "task",
            "id": format!("a2a-task-{number:060x}"),
            "contextId": format!("a2a-context-{number:060x}"),
            "status": {"state": state},
            "metadata": {},
        })
    }

    fn page(tasks: Vec<Value>, next_cursor: Option<&str>) -> Value {
        json!({"tasks": tasks, "nextCursor": next_cursor})
    }

    // spec: TUIG-RUNTIME-R012.3.1.1
    #[test]
    fn a_served_page_keeps_task_context_and_reply_identity() {
        let first = entry(1, "completed");
        let mut second = entry(2, "working");
        second["artifacts"] = json!([{
            "artifactId": "artifact-2",
            "parts": [{"kind": "text", "text": "a\nb"}],
        }]);
        let parsed = TaskPage::parse(&page(vec![first.clone(), second], Some("opaque/a"))).unwrap();
        assert_eq!(parsed.next_cursor.as_deref(), Some("opaque/a"));
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[0].state, TaskState::Completed);
        assert_eq!(
            parsed.tasks[0].correlation.task_id(),
            format!("a2a-task-{v:060x}", v = 1_u64)
        );
        assert_eq!(parsed.tasks[0].reply, None, "a missing reply is explicit");
        assert_eq!(parsed.tasks[1].reply.as_deref(), Some("a\nb"));
    }

    // spec: TUIG-RUNTIME-R012.3.1.1
    #[test]
    fn a_reply_is_sanitized_and_bounded_to_the_first_text_artifact() {
        let mut task = entry(3, "completed");
        task["artifacts"] = json!([{
            "artifactId": "artifact-3",
            "parts": [
                {"kind": "text", "text": "\u{001b}\u{202e}safe"},
                {"kind": "text", "text": &"x".repeat(MAX_REPLY_CHARS + 1)},
            ],
        }]);
        let parsed = TaskPage::parse(&page(vec![task], None)).unwrap();
        let reply = parsed.tasks[0].reply.as_deref().unwrap();
        assert!(!reply.contains('\u{001b}'));
        assert!(!reply.contains('\u{202e}'));
        assert_eq!(reply.chars().count(), MAX_REPLY_CHARS);
        // Only the first artifact is rendered.
        let mut second = entry(4, "completed");
        second["artifacts"] = json!([
            {"artifactId": "a", "parts": [{"kind": "text", "text": "one"}]},
            {"artifactId": "b", "parts": [{"kind": "text", "text": "two"}]},
        ]);
        let parsed = TaskPage::parse(&page(vec![second], None)).unwrap();
        assert_eq!(parsed.tasks[0].reply.as_deref(), Some("one"));
    }

    // spec: TUIG-RUNTIME-R012.3.1.1
    #[test]
    fn an_empty_page_is_a_valid_page_with_its_continuation() {
        let parsed = TaskPage::parse(&page(Vec::new(), Some("opaque/a"))).unwrap();
        assert!(parsed.tasks.is_empty());
        assert_eq!(parsed.next_cursor.as_deref(), Some("opaque/a"));
    }

    // spec: TUIG-RUNTIME-R012.3.1.1
    #[test]
    fn malformed_pages_and_cursors_are_refused() {
        let mut base = page(
            vec![entry(1, "completed"), entry(2, "working")],
            Some("opaque/a"),
        );
        base["tasks"][0]["artifacts"] = json!([{
            "artifactId": "artifact-1",
            "parts": [{"kind": "text", "text": "x"}],
        }]);
        for (pointer, change) in [
            ("/nextCursor", json!(42)),
            ("/nextCursor", json!("")),
            ("/nextCursor", json!(&"c".repeat(MAX_CURSOR_CHARS + 1))),
            ("/nextCursor", json!("a\nb")),
            ("/tasks", json!(["task-1"])),
            ("/tasks/0/kind", json!("work-item")),
            ("/tasks/0/contextId", json!("")),
            ("/tasks/0/contextId", json!("a\nb")),
            ("/tasks/0/status/state", json!("unknown")),
            (
                "/tasks/0/artifacts",
                json!([{"artifactId": "", "parts": [{"kind": "text", "text": "x"}]}]),
            ),
            ("/tasks/0/artifacts/0/artifactId", json!("")),
            ("/tasks/0/artifacts/0/parts", json!([])),
            ("/tasks/0/artifacts/0/parts/0/kind", json!("binary")),
        ] {
            let mut value = base.clone();
            if pointer == "/tasks" {
                value["tasks"] = change.clone();
            } else {
                *value.pointer_mut(pointer).unwrap() = change;
            }
            assert!(TaskPage::parse(&value).is_err(), "{pointer}");
        }
        let value = page(vec![entry(1, "completed"), entry(1, "working")], None);
        assert!(TaskPage::parse(&value).is_err(), "duplicate task identity");
        let oversized = (0..=MAX_TASKS)
            .map(|number| entry(number as u64 + 1, "completed"))
            .collect::<Vec<_>>();
        assert!(
            TaskPage::parse(&page(oversized, None)).is_err(),
            "an oversized page is refused"
        );
    }
}
