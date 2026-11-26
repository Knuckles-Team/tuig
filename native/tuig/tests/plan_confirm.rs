//! TUIG-RUNTIME-R001.3: one /confirm resumes exactly the issued plan.
//!
//! The fixture server is the source of truth for the issued plan: it serves
//! the task metadata the terminal reads, re-checks every confirmation
//! field against its own record, and consumes the single-use lease.
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tuig_native::{
    ArboraClient, AuthMode, TaskSubmission, clock::Clock, correlation::TaskState,
    plan_confirm::PlanBinding,
};

mod support;

const NOW_MS: u64 = 1_800_000_000_000;

const TASK_ID: &str = "task-1";
const CONTEXT_ID: &str = "context-1";
const RUN_ID: &str = "run-1";
const REFERENCE: &str = "plan-issued-1";
const SUBJECT: &str = "subject-1";
const OP: &str = "ops.example/read";
const PARAMS_DIGEST: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const REGISTRY_DIGEST: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
const LEASE: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

#[derive(Clone)]
struct PlanIssue {
    reference: &'static str,
    subject: &'static str,
    op: &'static str,
    params_digest: &'static str,
    registry_digest: &'static str,
    lease: &'static str,
    expires_at_ms: u64,
}

impl PlanIssue {
    fn plan_value(&self) -> Value {
        json!({
            "reference": self.reference,
            "subject": self.subject,
            "op": self.op,
            "paramsDigest": self.params_digest,
            "registryDigest": self.registry_digest,
            "expiresAt": self.expires_at_ms,
            "lease": self.lease,
        })
    }
}

fn fresh_issue() -> PlanIssue {
    PlanIssue {
        reference: REFERENCE,
        subject: SUBJECT,
        op: OP,
        params_digest: PARAMS_DIGEST,
        registry_digest: REGISTRY_DIGEST,
        lease: LEASE,
        expires_at_ms: NOW_MS + 3_600_000,
    }
}

#[derive(Clone)]
struct Captured {
    request: String,
    response: String,
}

fn http_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn task_response(request_id: &Value, state: &str, plan: &Value) -> String {
    let task = json!({
        "kind": "task",
        "id": TASK_ID,
        "contextId": CONTEXT_ID,
        "metadata": {"arbora": {"runId": RUN_ID, "plan": plan}},
        "status": {"state": state},
    });
    let body = json!({"jsonrpc": "2.0", "id": request_id, "result": task}).to_string();
    http_response(&body)
}

fn confirm_response(
    request: &Value,
    issued: &PlanIssue,
    effect: &AtomicBool,
    consumed: &AtomicBool,
) -> String {
    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
    let confirm = request
        .pointer("/params/message/parts/0/data/planConfirm")
        .cloned()
        .unwrap_or(Value::Null);
    let context_id = request
        .pointer("/params/message/contextId")
        .and_then(Value::as_str);
    let fields_match = confirm.get("reference").and_then(Value::as_str) == Some(issued.reference)
        && confirm.get("subject").and_then(Value::as_str) == Some(issued.subject)
        && confirm.get("op").and_then(Value::as_str) == Some(issued.op)
        && confirm.get("paramsDigest").and_then(Value::as_str) == Some(issued.params_digest)
        && confirm.get("registryDigest").and_then(Value::as_str) == Some(issued.registry_digest);
    let lease_fresh = confirm.get("lease").and_then(Value::as_str) == Some(issued.lease)
        && !consumed.swap(true, Ordering::SeqCst);
    let unexpired = issued.expires_at_ms > NOW_MS;
    if fields_match && lease_fresh && unexpired && context_id == Some(CONTEXT_ID) {
        effect.store(true, Ordering::SeqCst);
        task_response(&request_id, "completed", &issued.plan_value())
    } else {
        let body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32000, "message": "plan confirmation refused"},
        })
        .to_string();
        http_response(&body)
    }
}

/// A loopback A2A fixture that issues one plan and re-checks confirmations.
async fn plan_server(
    issued: PlanIssue,
    served_plan: Value,
) -> (
    String,
    Arc<Mutex<Vec<Captured>>>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
) {
    let (listener, url) = support::bind().await;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let effect = Arc::new(AtomicBool::new(false));
    let consumed = Arc::new(AtomicBool::new(false));
    let server_issued = issued;
    let server_served = served_plan;
    let server_captured = captured.clone();
    let server_effect = effect.clone();
    let server_consumed = consumed.clone();
    let _server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = socket.split();
            let mut buffer = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
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
            let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
            let request_line = head.lines().next().unwrap_or_default();
            let mut parts = request_line.split(' ');
            let method = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("");
            let request: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let request_id = request.get("id").cloned().unwrap_or(Value::Null);
            let confirm = request
                .pointer("/params/message/parts/0/data/planConfirm")
                .cloned()
                .is_some();
            let response = if method == "POST" && path == "/a2a" && !confirm {
                task_response(&request_id, "working", &server_served)
            } else if confirm {
                confirm_response(&request, &server_issued, &server_effect, &server_consumed)
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned()
            };
            server_captured.lock().unwrap().push(Captured {
                request: body.to_owned(),
                response: response.clone(),
            });
            let _ = write.write_all(response.as_bytes()).await;
        }
    });
    (url, captured, effect, consumed)
}

fn observed_plan(captured: &Arc<Mutex<Vec<Captured>>>) -> Value {
    captured
        .lock()
        .unwrap()
        .iter()
        .last()
        .and_then(response_value)
        .and_then(|envelope| envelope.get("result").cloned())
        .unwrap_or(Value::Null)
}

fn response_value(entry: &Captured) -> Option<Value> {
    entry
        .response
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str::<Value>(body).ok())
}

async fn client_and_snapshot(
    url: &str,
    captured: &Arc<Mutex<Vec<Captured>>>,
) -> (ArboraClient, tuig_native::TaskSnapshot) {
    let client = ArboraClient::with_auth_mode(url, None, AuthMode::None)
        .unwrap()
        .with_clock(Clock::from_fn(|| NOW_MS));
    let submission = TaskSubmission::new("inspect the issued plan").unwrap();
    let snapshot = client.submit_task(&submission).await.unwrap();
    assert_eq!(snapshot.state, TaskState::Working);
    assert_eq!(snapshot.correlation.task_id(), TASK_ID);
    // The plan the terminal confirms comes from what Arbora served.
    assert!(snapshot.plan.is_some());
    assert_eq!(
        PlanBinding::from_task(&observed_plan(captured))
            .as_ref()
            .ok(),
        snapshot.plan.as_ref()
    );
    (client, snapshot)
}

// spec: TUIG-RUNTIME-R001.3
#[tokio::test]
async fn confirm_resumes_exactly_the_issued_plan() {
    let issued = fresh_issue();
    let (url, captured, effect, consumed) = plan_server(issued.clone(), issued.plan_value()).await;
    let (client, snapshot) = client_and_snapshot(&url, &captured).await;

    let plan = snapshot.plan.unwrap();
    assert_eq!(plan.reference(), REFERENCE);
    assert_eq!(plan.subject(), SUBJECT);
    assert_eq!(plan.op(), OP);
    assert_eq!(plan.params_digest(), PARAMS_DIGEST);
    assert_eq!(plan.registry_digest(), REGISTRY_DIGEST);
    assert_eq!(plan.lease(), LEASE);
    assert!(!plan.is_expired(NOW_MS));

    let resumed = client
        .confirm_plan(&snapshot.correlation, &plan)
        .await
        .unwrap();
    assert_eq!(resumed.state, TaskState::Completed);
    assert!(
        effect.load(Ordering::SeqCst),
        "Arbora applied the effect exactly once"
    );
    assert!(consumed.load(Ordering::SeqCst));

    let confirm_requests = captured
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry.request.contains("planConfirm"))
        .map(|entry| serde_json::from_str::<Value>(&entry.request).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        confirm_requests.len(),
        1,
        "one /confirm issues exactly one effect request"
    );
    let sent = &confirm_requests[0]["params"]["message"];
    assert_eq!(sent["contextId"], CONTEXT_ID);
    let confirm_body = &sent["parts"][0]["data"]["planConfirm"];
    assert_eq!(confirm_body["reference"], REFERENCE);
    assert_eq!(confirm_body["subject"], SUBJECT);
    assert_eq!(confirm_body["op"], OP);
    assert_eq!(confirm_body["paramsDigest"], PARAMS_DIGEST);
    assert_eq!(confirm_body["registryDigest"], REGISTRY_DIGEST);
    assert_eq!(confirm_body["lease"], LEASE);
}

// spec: TUIG-RUNTIME-R001.3
#[tokio::test]
async fn tampered_confirmations_are_refused_before_any_effect() {
    let issued = fresh_issue();
    let clean = issued.plan_value();
    let tamper = |field: &str, value: Value| {
        let mut plan = clean.clone();
        plan[field] = value;
        plan
    };
    for plan in [
        tamper("op", json!("ops.example/tampered")),
        tamper("subject", json!("subject-tampered")),
        tamper("paramsDigest", json!("a1".repeat(31) + "00")),
        tamper("registryDigest", json!("b2".repeat(31) + "00")),
    ] {
        let (url, captured, effect, _) = plan_server(issued.clone(), plan).await;
        let (client, snapshot) = client_and_snapshot(&url, &captured).await;
        let plan = snapshot.plan.unwrap();
        assert!(
            client
                .confirm_plan(&snapshot.correlation, &plan)
                .await
                .is_err(),
            "a tampered confirmation must be refused"
        );
        assert!(
            !effect.load(Ordering::SeqCst),
            "no effect is applied for a tampered confirmation"
        );
        assert!(
            captured
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.response.contains("plan confirmation refused"))
        );
    }
}

// spec: TUIG-RUNTIME-R001.3
#[tokio::test]
async fn an_expired_plan_is_refused_before_any_effect_request() {
    let mut issued = fresh_issue();
    issued.expires_at_ms = NOW_MS - 1;
    let (url, captured, effect, consumed) = plan_server(issued.clone(), issued.plan_value()).await;
    let (client, snapshot) = client_and_snapshot(&url, &captured).await;
    let plan = snapshot.plan.unwrap();
    assert!(plan.is_expired(NOW_MS));
    assert!(
        client
            .confirm_plan(&snapshot.correlation, &plan)
            .await
            .is_err(),
        "the expired plan is refused before any effect request"
    );
    assert!(!effect.load(Ordering::SeqCst));
    assert!(!consumed.load(Ordering::SeqCst));
    assert_eq!(
        captured.lock().unwrap().len(),
        1,
        "only the initial task read reached the server"
    );
}

// spec: TUIG-RUNTIME-R001.3
#[tokio::test]
async fn a_replayed_confirmation_is_refused_with_a_stale_lease() {
    let issued = fresh_issue();
    let (url, captured, effect, _) = plan_server(issued.clone(), issued.plan_value()).await;
    let (client, snapshot) = client_and_snapshot(&url, &captured).await;
    let plan = snapshot.plan.unwrap();

    let resumed = client
        .confirm_plan(&snapshot.correlation, &plan)
        .await
        .unwrap();
    assert_eq!(resumed.state, TaskState::Completed);
    let replay = client.confirm_plan(&snapshot.correlation, &plan).await;
    assert!(
        replay.is_err(),
        "the server refuses the replayed, stale lease"
    );

    let confirm_responses = captured
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry.request.contains("planConfirm"))
        .map(response_value)
        .collect::<Option<Vec<_>>>()
        .unwrap();
    assert_eq!(confirm_responses.len(), 2);
    assert!(confirm_responses[0].get("error").is_none());
    assert!(
        confirm_responses[1].get("error").is_some(),
        "the second confirmation was refused"
    );
    assert!(
        effect.load(Ordering::SeqCst),
        "the effect happened exactly once"
    );
}
