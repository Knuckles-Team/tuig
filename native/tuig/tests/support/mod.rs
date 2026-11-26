//! Shared loopback fixtures. Each test crate uses a subset of them.
#![allow(dead_code)]

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
