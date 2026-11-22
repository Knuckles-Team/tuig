//! TUIG-RUNTIME-R001.1: the /op command and the versioned registry dispatch.
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tuig_native::{ArboraClient, operation_command::OperationCommand};

const DIGEST: &str = "fixture-digest-0001";

#[derive(Clone)]
struct Captured {
    method: String,
    path: String,
    headers: String,
    body: String,
}

/// A loopback fixture that serves one registry and answers operation posts.
async fn serve(
    digest: Option<String>,
    ops: Vec<String>,
    op_response: Value,
) -> (String, tokio::task::JoinHandle<()>, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = captured.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = socket.split();
            let mut buffer = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let count = match read.read(&mut chunk).await {
                    Ok(count) => count,
                    Err(_) => 0,
                };
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
            if head.is_empty() {
                continue;
            }
            let request_line = head.lines().next().unwrap_or_default();
            let mut parts = request_line.split(' ');
            let method = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("");
            server_captured.lock().unwrap().push(Captured {
                method: method.to_owned(),
                path: path.to_owned(),
                headers: head.to_lowercase(),
                body: body.to_owned(),
            });
            let status_line = if digest.is_none() {
                "HTTP/1.1 404 Not Found"
            } else if method == "GET" && path == "/api/v1/registry" {
                "HTTP/1.1 200 OK"
            } else if method == "POST" && path.starts_with("/api/v1/ops/") {
                "HTTP/1.1 200 OK"
            } else {
                "HTTP/1.1 404 Not Found"
            };
            let body_text = if status_line == "HTTP/1.1 200 OK" {
                if method == "GET" {
                    json!({
                        "registry_digest": digest,
                        "api_version": "v1",
                        "ops": ops.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
                    })
                    .to_string()
                } else {
                    op_response.to_string()
                }
            } else {
                String::new()
            };
            let response = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
                body_text.len()
            );
            let _ = write.write_all(response.as_bytes()).await;
        }
    });
    (url, server, captured)
}

fn effect_requests(captured: &[Captured]) -> Vec<&Captured> {
    captured.iter().filter(|c| c.method == "POST").collect()
}

// spec: TUIG-RUNTIME-R001.1
#[tokio::test]
async fn op_command_builds_one_versioned_request() {
    let op_response = json!({
        "ok": true,
        "meta": {"registry_digest": DIGEST, "api_version": "v1"},
        "result": {"value": {"runId": "run-1", "revision": 4}},
    });
    let (url, server, captured) = serve(Some(DIGEST.to_owned()), vec!["runs.get".to_owned()], op_response.clone()).await;
    let client = ArboraClient::new(&url, Some("fixture-token")).unwrap();
    let payload = r#"{ "runId" : "run-1",  "nested":{ "a":[1,2] } }"#;
    let command = OperationCommand::parse_command(&format!("/op runs.get {payload}")).unwrap();
    let value = client
        .invoke_operation(command.operation(), command.payload())
        .await
        .unwrap();
    assert_eq!(value, json!({"runId": "run-1", "revision": 4}));
    server.abort();
    let captured = captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2, "registry read then one dispatch");
    assert_eq!(
        (captured[0].method.as_str(), captured[0].path.as_str()),
        ("GET", "/api/v1/registry")
    );
    assert!(captured[0].headers.contains("authorization: bearer fixture-token"));
    assert_eq!(
        (captured[1].method.as_str(), captured[1].path.as_str()),
        ("POST", "/api/v1/ops/runs.get")
    );
    assert!(captured[1].headers.contains("authorization: bearer fixture-token"));
    assert!(captured[1].headers.contains("content-type: application/json"));
    assert_eq!(captured[1].body, payload, "payload reaches the client unchanged");
    let parsed: Value = serde_json::from_str(&captured[1].body).unwrap();
    assert_eq!(parsed["nested"]["a"], json!([1, 2]));
    assert_eq!(effect_requests(&captured).len(), 1, "exactly one versioned request");
}

// spec: TUIG-RUNTIME-R001.1
#[tokio::test]
async fn refusal_cases_make_zero_effect_requests() {
    let ok_response = json!({
        "ok": true,
        "meta": {"registry_digest": DIGEST, "api_version": "v1"},
        "result": {"value": {}},
    });

    // An operation the registry does not advertise never reaches a dispatch.
    {
        let (url, server, captured) = serve(Some(DIGEST.to_owned()), vec!["runs.get".to_owned()], ok_response.clone()).await;
        let client = ArboraClient::new(&url, Some("fixture-token")).unwrap();
        let command = OperationCommand::parse_command("/op runs.missing {}").unwrap();
        assert!(
            client
                .invoke_operation(command.operation(), command.payload())
                .await
                .is_err()
        );
        server.abort();
        let captured = captured.lock().unwrap().clone();
        assert_eq!(effect_requests(&captured).len(), 0);
    }

    // An absent registry refuses before any dispatch.
    {
        let (url, server, captured) = serve(None, vec![], ok_response.clone()).await;
        let client = ArboraClient::new(&url, Some("fixture-token")).unwrap();
        assert!(client.invoke_operation("runs.get", "{}").await.is_err());
        server.abort();
        let captured = captured.lock().unwrap().clone();
        assert_eq!(effect_requests(&captured).len(), 0);
    }

    // A response whose registry digest does not match is refused as unavailable.
    {
        let bad_response = json!({
            "ok": true,
            "meta": {"registry_digest": "other-digest-0002", "api_version": "v1"},
            "result": {"value": {}},
        });
        let (url, server, _captured) = serve(Some(DIGEST.to_owned()), vec!["runs.get".to_owned()], bad_response).await;
        let client = ArboraClient::new(&url, Some("fixture-token")).unwrap();
        assert!(client.invoke_operation("runs.get", "{}").await.is_err());
        server.abort();
    }
}
