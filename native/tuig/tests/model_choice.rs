//! TUIG-RUNTIME-R012.2.2 and R012.4: the model picker and the served command matrix.
//!
//! The fixture serves one registry and the operations it advertises, answers
//! `message/send` with a task naming a profile, and records every request so a
//! test can prove which routes received traffic.
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tuig_native::{
    ArboraClient, TaskSubmission,
    model_choice::{ModelAction, ModelChoice, run},
    served_command::{MATRIX, ServedCommand},
};

mod support;

const DIGEST: &str = "fixture-digest-0002";

type Requests = Arc<Mutex<Vec<(String, String)>>>;

fn operation_reply(value: Value) -> Value {
    json!({
        "ok": true,
        "meta": {"registry_digest": DIGEST, "api_version": "v1"},
        "result": {"value": value},
    })
}

fn task_reply(request_id: &str, profile: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "kind": "task", "id": "task-1", "contextId": "context-1",
            "metadata": {"arbora": {"runId": "run-1", "modelProfile": profile}},
            "status": {"state": "submitted"},
        },
    })
}

async fn fixture(operations: &[(&str, Value)], served_profile: &str) -> (String, Requests) {
    let (listener, url) = support::bind().await;
    let requests: Requests = Arc::default();
    let seen = requests.clone();
    let operations: Vec<(String, Value)> = operations
        .iter()
        .map(|(id, value)| ((*id).to_owned(), value.clone()))
        .collect();
    let served_profile = served_profile.to_owned();
    let _server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let (head, body) = loop {
                let mut chunk = [0_u8; 4096];
                let count = socket.read(&mut chunk).await.unwrap_or_default();
                buffer.extend_from_slice(&chunk[..count]);
                let text = String::from_utf8_lossy(&buffer).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .to_lowercase()
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())
                        .unwrap_or(0_usize);
                    if body.len() >= length || count == 0 {
                        break (head.to_owned(), body.to_owned());
                    }
                } else if count == 0 {
                    break (text, String::new());
                }
            };
            let mut line = head.lines().next().unwrap_or_default().split(' ');
            let (method, path) = (line.next().unwrap_or(""), line.next().unwrap_or(""));
            seen.lock().unwrap().push((path.to_owned(), body.clone()));
            let reply = if path == "/api/v1/registry" {
                json!({
                    "registry_digest": DIGEST, "api_version": "v1",
                    "ops": operations.iter().map(|(id, _)| json!({"id": id})).collect::<Vec<_>>(),
                })
            } else if path == "/a2a" && method == "POST" {
                let request: Value = serde_json::from_str(&body).unwrap();
                task_reply(request["id"].as_str().unwrap(), &served_profile)
            } else if let Some(id) = path.strip_prefix("/api/v1/ops/") {
                let value = operations.iter().find(|(known, _)| known == id);
                operation_reply(value.map_or(Value::Null, |(_, value)| value.clone()))
            } else {
                Value::Null
            };
            let text = reply.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (url, requests)
}

fn client(url: &str) -> ArboraClient {
    ArboraClient::new(url, Some("fixture-token")).unwrap()
}

fn profiles() -> Value {
    json!({"kind": "chat", "models": [{"id": "alpha"}, {"id": "beta"}]})
}

// spec: TUIG-RUNTIME-R012.2.2
#[tokio::test]
async fn selected_profile_is_displayed_from_task_metadata() {
    let (url, requests) = fixture(&[("models.list", profiles())], "alpha").await;
    let client = client(&url);
    let choice = ModelChoice::default();
    let listing = run(&client, &choice, ModelAction::List).await;
    assert!(
        listing.contains("alpha") && listing.contains("beta"),
        "{listing}"
    );
    let notice = run(&client, &choice, ModelAction::Select("beta".to_owned())).await;
    assert!(notice.contains("beta"), "{notice}");
    assert_eq!(choice.selected().as_deref(), Some("beta"));
    assert!(
        run(&client, &choice, ModelAction::List)
            .await
            .contains("* beta")
    );

    let submission = TaskSubmission::new("hello")
        .unwrap()
        .with_model_profile(choice.selected());
    let task = client.submit_task(&submission).await.unwrap();
    assert_eq!(
        task.model_profile.as_deref(),
        Some("alpha"),
        "the task's profile is shown even when it differs from the preference"
    );
    let sent = requests.lock().unwrap();
    let message: Value = serde_json::from_str(&sent.last().unwrap().1).unwrap();
    assert_eq!(
        message["params"]["message"]["metadata"]["arboraModelProfile"],
        "beta"
    );
}

// spec: TUIG-RUNTIME-R012.2.2
#[tokio::test]
async fn no_preference_is_sent_until_one_is_chosen_and_clearing_removes_it() {
    let (url, requests) = fixture(&[("models.list", profiles())], "alpha").await;
    let client = client(&url);
    let choice = ModelChoice::default();
    let plain = TaskSubmission::new("hello")
        .unwrap()
        .with_model_profile(choice.selected());
    client.submit_task(&plain).await.unwrap();
    let body: Value = serde_json::from_str(&requests.lock().unwrap().last().unwrap().1).unwrap();
    assert!(body["params"]["message"].get("metadata").is_none());

    run(&client, &choice, ModelAction::Select("alpha".to_owned())).await;
    run(&client, &choice, ModelAction::Clear).await;
    assert_eq!(choice.selected(), None);
}

// spec: TUIG-RUNTIME-R012.2.2
#[tokio::test]
async fn an_unlisted_profile_is_not_selected_and_an_unserved_list_sends_nothing() {
    let (url, _requests) = fixture(&[("models.list", profiles())], "alpha").await;
    let choice = ModelChoice::default();
    let notice = run(
        &client(&url),
        &choice,
        ModelAction::Select("gamma".to_owned()),
    )
    .await;
    assert!(notice.contains("not listed"), "{notice}");
    assert_eq!(choice.selected(), None);

    let (url, requests) = fixture(&[], "alpha").await;
    let notice = run(&client(&url), &choice, ModelAction::List).await;
    assert!(notice.contains("models.list"), "{notice}");
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|(path, _)| path == "/api/v1/registry"),
        "an unserved operation must receive no request"
    );
}

// spec: TUIG-RUNTIME-R012.4.3
#[tokio::test]
async fn every_registered_command_is_served_or_unavailable_and_no_unserved_route_is_hit() {
    let served = [
        ("fleet.status", json!({"children": {}})),
        ("fleet.catalog.list", json!({"items": []})),
        ("prompts.list", json!({"prompts": []})),
        ("usage.read", json!({"totals": {"total_tokens": 42.0}})),
    ];
    let (url, requests) = fixture(&served, "alpha").await;
    let client = client(&url);
    let choice = ModelChoice::default();
    for spec in &MATRIX {
        let command = ServedCommand::parse(&format!("{} term", spec.name)).unwrap();
        let notice = command.run(&client, &choice, "turn text").await;
        let is_served = served.iter().any(|(id, _)| *id == spec.operation);
        if is_served && spec.operation == "usage.read" {
            assert!(notice.contains("Tokens: 42"), "{notice}");
        } else if is_served {
            assert!(!notice.contains("unavailable"), "{}: {notice}", spec.name);
        } else {
            assert!(
                notice.contains("unavailable") && notice.contains(spec.operation),
                "{}: {notice}",
                spec.name
            );
        }
    }
    let usage = ServedCommand::parse("/usage")
        .unwrap()
        .run(&client, &choice, "turn text")
        .await;
    assert!(usage.contains("Tokens: 42") && usage.contains("Cost: unavailable"));
    for (path, _) in requests.lock().unwrap().iter() {
        if let Some(id) = path.strip_prefix("/api/v1/ops/") {
            assert!(served.iter().any(|(known, _)| *known == id), "{id}");
        }
    }
}

// spec: TUIG-RUNTIME-R012.4.3
#[tokio::test]
async fn a_command_that_needs_an_argument_sends_nothing_without_one() {
    let (url, requests) = fixture(&[("retrieval.search", json!({}))], "alpha").await;
    let notice = ServedCommand::parse("/kb")
        .unwrap()
        .run(&client(&url), &ModelChoice::default(), "")
        .await;
    assert!(notice.contains("needs an argument"), "{notice}");
    assert!(requests.lock().unwrap().is_empty());
}
