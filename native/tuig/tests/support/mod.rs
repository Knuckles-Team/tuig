//! Shared loopback fixtures. Each test crate uses a subset of them.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// Bind an ephemeral loopback listener and return it with its `http://` URL.
pub async fn bind() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// Bind a blocking loopback listener for tests that run no async runtime.
pub fn bind_blocking() -> std::net::TcpListener {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap()
}

pub async fn serve_raw(response: String) -> (String, tokio::task::JoinHandle<String>) {
    serve_raw_after(response, Duration::ZERO).await
}

pub async fn serve_raw_after(
    response: String,
    delay: Duration,
) -> (String, tokio::task::JoinHandle<String>) {
    let (listener, url) = bind().await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            let text = String::from_utf8_lossy(&request);
            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if body.len() >= length {
                    break;
                }
            }
        }
        tokio::time::sleep(delay).await;
        let _ = socket.write_all(response.as_bytes()).await;
        drop(socket);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err(),
            "a unary request must never retry or follow redirects"
        );
        String::from_utf8(request).unwrap()
    });
    (url, server)
}

pub async fn serve(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    serve_raw(format!(
        "HTTP/1.1 {status}\r\nLocation: /redirected\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
    .await
}

/// A registry fixture: it serves the listed operations, answers `message/send`
/// with a task naming a profile and records every request path and body.
pub const DIGEST: &str = "fixture-digest-0002";

pub type Requests = Arc<Mutex<Vec<(String, String)>>>;

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

pub async fn fixture(operations: &[(&str, Value)], served_profile: &str) -> (String, Requests) {
    let (listener, url) = bind().await;
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
