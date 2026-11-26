//! TUIG-RUNTIME-R012.3.1.1: P rereads the previous task page with its
//! original opaque cursor.
//!
//! The fixture server is the source of truth for pagination: it answers each
//! `tasks/list` read with the page its cursor denotes and records every
//! cursor the terminal sends, so forward/back/forward walks, empty pages,
//! refusal and retry, refresh reset, duplicate input and close-suppressed
//! completions are all observable. The terminal stores no conversation.
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tuig_native::{
    ArboraClient, AuthMode,
    correlation::TaskState,
    task_history::{HistoryStatus, TaskHistory},
};

mod support;

/// The cursor whose read the fixture holds until the test releases it.
const GATED: &str = "opaque/gate";

type Serve = Box<dyn Fn(Option<&str>) -> (u16, Value) + Send + Sync>;

fn task_entry(number: u64, state: &str, reply: Option<&str>) -> Value {
    let mut task = json!({
        "kind": "task",
        "id": format!("a2a-task-{number:060x}"),
        "contextId": format!("a2a-context-{number:060x}"),
        "status": {"state": state},
        "metadata": {},
    });
    if let Some(reply) = reply {
        task["artifacts"] = json!([{
            "artifactId": format!("artifact-{number}"),
            "parts": [{"kind": "text", "text": reply}],
        }]);
    }
    task
}

fn list_result(tasks: Vec<Value>, next_cursor: Option<&str>) -> Value {
    json!({"tasks": tasks, "nextCursor": next_cursor})
}

fn refused() -> (u16, Value) {
    (403, Value::Null)
}

/// A loopback A2A fixture serving `tasks/list` pages for the given cursor.
async fn history_server(
    serve: Serve,
    gate: Option<Arc<AtomicBool>>,
) -> (String, Arc<Mutex<Vec<Option<String>>>>) {
    let (listener, url) = support::bind().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let server_calls = calls.clone();
    let _server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = socket.split();
            let mut buffer = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let count = read.read(&mut chunk).await.unwrap_or_default();
                if count == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..count]);
                let text = String::from_utf8_lossy(&buffer);
                if let Some((head, _)) = text.split_once("\r\n\r\n") {
                    let content_length = head
                        .lines()
                        .find_map(|line| line.split_once(':'))
                        .filter(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buffer.len() >= head.len() + content_length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&buffer).into_owned();
            let (_head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
            let request: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let request_id = request.get("id").cloned().unwrap_or(Value::Null);
            let method = request.get("method").and_then(Value::as_str);
            let cursor = request
                .pointer("/params/cursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            server_calls.lock().unwrap().push(cursor.clone());
            if let Some(gate) = &gate
                && cursor.as_deref() == Some(GATED)
                && !gate.load(Ordering::SeqCst)
            {
                while !gate.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            let (status, result) = serve(cursor.as_deref());
            let body = if status != 200 {
                String::new()
            } else if method == Some("tasks/list") {
                json!({"jsonrpc": "2.0", "id": request_id, "result": result}).to_string()
            } else {
                json!({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32601, "message": "Method not found"}}).to_string()
            };
            let response = if status != 200 {
                format!(
                    "HTTP/1.1 {status} Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = write.write_all(response.as_bytes()).await;
        }
    });
    (url, calls)
}

async fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        if tokio::time::Instant::now() >= deadline {
            panic!("fixture deadline exceeded");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Open the history and settle its first read.
async fn open_history(client: &Arc<ArboraClient>) -> TaskHistory {
    let mut history = TaskHistory::default();
    history.open(client.clone());
    history.complete().await;
    history
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn previous_page_rereads_original_cursor() {
    let pages = |cursor: Option<&str>| match cursor {
        None => (
            200,
            list_result(
                vec![task_entry(1, "completed", Some("one"))],
                Some("opaque/a"),
            ),
        ),
        Some("opaque/a") => (
            200,
            list_result(
                vec![task_entry(2, "completed", Some("two"))],
                Some("opaque/b"),
            ),
        ),
        Some("opaque/b") => (
            200,
            list_result(vec![task_entry(3, "completed", None)], None),
        ),
        _ => refused(),
    };
    let (url, calls) = history_server(Box::new(pages), None).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    assert_eq!(history.status(), HistoryStatus::Page);
    assert_eq!(history.page().unwrap().tasks.len(), 1);
    assert_eq!(history.next_cursor(), Some("opaque/a"));
    assert_eq!(history.path().to_vec(), vec![None]);

    // Forward to page 2, then to page 3.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("opaque/a".to_owned())]
    );
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![
            None,
            Some("opaque/a".to_owned()),
            Some("opaque/b".to_owned())
        ]
    );

    // P rereads page 2 with the original cursor that fetched it.
    history.previous(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("opaque/a".to_owned())]
    );
    assert_eq!(
        history.page().unwrap().tasks[0].correlation.task_id(),
        format!("a2a-task-{v:060x}", v = 2_u64)
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            None,
            Some("opaque/a".to_owned()),
            Some("opaque/b".to_owned()),
            Some("opaque/a".to_owned())
        ]
    );

    // P back to page 1, then N forward to page 2 again.
    history.previous(client.clone());
    history.complete().await;
    assert_eq!(history.path().to_vec(), vec![None]);
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("opaque/a".to_owned())]
    );

    // R resets the path to the first server page.
    history.reset(client.clone());
    history.complete().await;
    assert_eq!(history.path().to_vec(), vec![None]);
    assert_eq!(history.page().unwrap().tasks[0].state, TaskState::Completed);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            None,
            Some("opaque/a".to_owned()),
            Some("opaque/b".to_owned()),
            Some("opaque/a".to_owned()),
            None,
            Some("opaque/a".to_owned()),
            None
        ]
    );
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn empty_pages_keep_position_and_render_without_rows() {
    let pages = |cursor: Option<&str>| match cursor {
        None => (200, list_result(Vec::new(), Some("opaque"))),
        Some("opaque") => (
            200,
            list_result(vec![task_entry(2, "completed", Some("two"))], None),
        ),
        _ => refused(),
    };
    let (url, calls) = history_server(Box::new(pages), None).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    assert_eq!(history.status(), HistoryStatus::Page);
    assert!(
        history.page().unwrap().tasks.is_empty(),
        "the empty page is a page"
    );
    assert_eq!(history.next_cursor(), Some("opaque"));
    assert_eq!(history.path().to_vec(), vec![None]);

    // P on the first page is a no-op.
    history.previous(client.clone());
    assert!(!history.pending());
    assert_eq!(*calls.lock().unwrap(), vec![None]);

    // N across the empty page to the populated one.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("opaque".to_owned())]
    );
    assert_eq!(history.page().unwrap().tasks.len(), 1);
    assert_eq!(history.next_cursor(), None);

    // P back over the populated page re-reads the empty page.
    history.previous(client.clone());
    history.complete().await;
    assert_eq!(history.path().to_vec(), vec![None]);
    assert!(history.page().unwrap().tasks.is_empty());
    assert_eq!(history.next_cursor(), Some("opaque"));

    // N on the last page is a no-op.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("opaque".to_owned())]
    );
    history.next(client.clone());
    assert!(!history.pending());
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            None,
            Some("opaque".to_owned()),
            None,
            Some("opaque".to_owned())
        ]
    );
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn refused_reads_retain_position_for_retry() {
    let denied = Arc::new(AtomicBool::new(false));
    let denied_clone = denied.clone();
    let pages = move |cursor: Option<&str>| match cursor {
        None => (
            200,
            list_result(
                vec![task_entry(1, "completed", Some("one"))],
                Some("opaque"),
            ),
        ),
        Some("opaque") => {
            if denied_clone.swap(true, Ordering::SeqCst) {
                (
                    200,
                    list_result(vec![task_entry(2, "completed", Some("two"))], None),
                )
            } else {
                refused()
            }
        }
        _ => refused(),
    };
    let (url, calls) = history_server(Box::new(pages), None).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    assert_eq!(history.path().to_vec(), vec![None]);

    // The first N read is refused; rows clear, position and continuation stay.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(history.status(), HistoryStatus::Refused);
    assert!(
        history.page().is_none(),
        "old rows are cleared after failure"
    );
    assert_eq!(history.path().to_vec(), vec![None]);
    assert_eq!(history.next_cursor(), Some("opaque"));

    // N retries the same cursor and succeeds.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(history.status(), HistoryStatus::Page);
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("opaque".to_owned())]
    );
    assert_eq!(history.page().unwrap().tasks.len(), 1);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![None, Some("opaque".to_owned()), Some("opaque".to_owned())]
    );

    // P from the retry position re-reads the first page.
    history.previous(client.clone());
    history.complete().await;
    assert_eq!(history.path().to_vec(), vec![None]);
    assert_eq!(history.page().unwrap().tasks.len(), 1);
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn refresh_resets_the_path() {
    let pages = |cursor: Option<&str>| match cursor {
        None => (
            200,
            list_result(
                vec![task_entry(1, "completed", Some("one"))],
                Some("opaque/a"),
            ),
        ),
        Some("opaque/a") => (
            200,
            list_result(
                vec![task_entry(2, "completed", Some("two"))],
                Some("opaque/b"),
            ),
        ),
        Some("opaque/b") => (
            200,
            list_result(vec![task_entry(3, "completed", None)], None),
        ),
        _ => refused(),
    };
    let (url, calls) = history_server(Box::new(pages), None).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    history.next(client.clone());
    history.complete().await;
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![
            None,
            Some("opaque/a".to_owned()),
            Some("opaque/b".to_owned())
        ]
    );

    // R rereads the first server page and drops the walked path.
    history.reset(client.clone());
    history.complete().await;
    assert_eq!(history.path().to_vec(), vec![None]);
    assert_eq!(history.next_cursor(), Some("opaque/a"));
    assert_eq!(history.page().unwrap().tasks.len(), 1);

    // R is idempotent on the first page.
    history.reset(client.clone());
    history.complete().await;
    assert_eq!(history.path().to_vec(), vec![None]);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            None,
            Some("opaque/a".to_owned()),
            Some("opaque/b".to_owned()),
            None,
            None
        ]
    );
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn duplicate_navigation_is_ignored_while_pending() {
    let pages = |cursor: Option<&str>| match cursor {
        None => (
            200,
            list_result(vec![task_entry(1, "completed", Some("one"))], Some(GATED)),
        ),
        Some(GATED) => (
            200,
            list_result(vec![task_entry(2, "completed", Some("two"))], None),
        ),
        _ => refused(),
    };
    let gate = Arc::new(AtomicBool::new(false));
    let (url, calls) = history_server(Box::new(pages), Some(gate.clone())).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    history.next(client.clone());
    assert!(history.pending());
    until(|| {
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|cursor| cursor.as_deref() == Some(GATED))
    })
    .await;

    // Duplicate navigation while the read is pending issues no new request.
    history.previous(client.clone());
    history.reset(client.clone());
    history.next(client.clone());
    assert_eq!(
        *calls.lock().unwrap(),
        vec![None, Some(GATED.to_owned())],
        "no second request while the first is pending"
    );

    // The in-flight read settles when the server finally answers.
    gate.store(true, Ordering::SeqCst);
    history.complete().await;
    assert!(!history.pending());
    assert_eq!(history.status(), HistoryStatus::Page);
    assert_eq!(history.path().to_vec(), vec![None, Some(GATED.to_owned())]);
    assert_eq!(history.page().unwrap().tasks.len(), 1);
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn late_completion_after_close_is_discarded() {
    let pages = |cursor: Option<&str>| match cursor {
        None => (
            200,
            list_result(vec![task_entry(1, "completed", Some("one"))], Some(GATED)),
        ),
        Some(GATED) => (
            200,
            list_result(vec![task_entry(2, "completed", Some("two"))], None),
        ),
        _ => refused(),
    };
    let gate = Arc::new(AtomicBool::new(false));
    let (url, calls) = history_server(Box::new(pages), Some(gate.clone())).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    history.next(client.clone());
    until(|| {
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|cursor| cursor.as_deref() == Some(GATED))
    })
    .await;

    // Close invalidates the pending read and clears rows and position.
    history.close();
    assert!(!history.is_open());
    assert!(!history.pending());
    assert!(history.page().is_none());
    assert!(history.path().is_empty());

    // The server's late answer is discarded; the closed view does not move.
    gate.store(true, Ordering::SeqCst);
    history.complete().await;
    assert!(!history.is_open());
    assert!(history.page().is_none());
    assert!(history.path().is_empty());
    assert_ne!(history.status(), HistoryStatus::Page);
    assert_eq!(*calls.lock().unwrap(), vec![None, Some(GATED.to_owned())]);
}

// spec: TUIG-RUNTIME-R012.3.1.1
#[tokio::test]
async fn cyclic_cursor_is_refused_before_advance() {
    let pages = |cursor: Option<&str>| match cursor {
        None => (
            200,
            list_result(vec![task_entry(1, "completed", Some("one"))], Some("first")),
        ),
        Some("first") => (
            200,
            list_result(
                vec![task_entry(2, "completed", Some("two"))],
                Some("second"),
            ),
        ),
        // The cycle closes back onto an already visited page.
        Some("second") => (
            200,
            list_result(vec![task_entry(3, "completed", None)], Some("first")),
        ),
        _ => refused(),
    };
    let (url, calls) = history_server(Box::new(pages), None).await;
    let client = Arc::new(ArboraClient::with_auth_mode(&url, None, AuthMode::None).unwrap());

    let mut history = open_history(&client).await;
    history.next(client.clone());
    history.complete().await;
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("first".to_owned())]
    );

    // The served continuation lands on a visited page: refused, no advance.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(history.status(), HistoryStatus::Refused);
    assert!(
        history.page().is_none(),
        "old rows are cleared after failure"
    );
    assert_eq!(
        history.path().to_vec(),
        vec![None, Some("first".to_owned())]
    );
    assert_eq!(history.next_cursor(), Some("second"));

    // The cycle keeps refusing; P still re-reads from the retained position.
    history.next(client.clone());
    history.complete().await;
    assert_eq!(history.status(), HistoryStatus::Refused);
    history.previous(client.clone());
    history.complete().await;
    assert_eq!(history.status(), HistoryStatus::Page);
    assert_eq!(history.path().to_vec(), vec![None]);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            None,
            Some("first".to_owned()),
            Some("second".to_owned()),
            Some("second".to_owned()),
            None
        ]
    );
}
