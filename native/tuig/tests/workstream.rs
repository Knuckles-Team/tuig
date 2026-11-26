//! Workstream peer: attach at session open, checkpoint and detach at exit.
use serde_json::{Value, json};
use support::fixture;
use tuig_native::{ArboraClient, workstream};

mod support;

fn client(url: &str) -> ArboraClient {
    ArboraClient::new(url, Some("fixture-token")).unwrap()
}

fn posted(requests: &support::Requests) -> Vec<(String, Value)> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(path, body)| {
            let id = path.strip_prefix("/api/v1/ops/")?;
            Some((id.to_owned(), serde_json::from_str(body).unwrap()))
        })
        .collect()
}

// spec: TUIG-RUNTIME-R013.1
#[tokio::test]
async fn attach_shows_the_served_context_and_exit_records_one_checkpoint_and_one_detach() {
    let served = [
        (
            "workstream.attach",
            json!({"context": "Last checkpoint from another harness"}),
        ),
        ("workstream.checkpoint", json!({"ok": true})),
        ("workstream.detach", json!({"ok": true})),
    ];
    let (url, requests) = fixture(&served, "alpha").await;
    let client = client(&url);
    let (stream, context) = workstream::open(&client, "/work/root").await;
    assert_eq!(context, "Last checkpoint from another harness");
    let stream = stream.expect("attached");
    let session = stream.session_id().to_owned();
    assert_eq!(workstream::close(&client, stream).await, "");

    let calls = posted(&requests);
    let ids: Vec<&str> = calls.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "workstream.attach",
            "workstream.checkpoint",
            "workstream.detach"
        ]
    );
    assert_eq!(calls[0].1["harness"], "tuig");
    assert_eq!(calls[0].1["workspace_root"], "/work/root");
    assert_eq!(calls[0].1["harness_session_id"], session);
    assert_eq!(calls[1].1["trigger"], "exit");
    assert_eq!(calls[2].1["harness_session_id"], session);
}

#[tokio::test]
async fn an_unserved_workstream_shows_one_notice_and_sends_no_operation() {
    let (url, requests) = fixture(&[], "alpha").await;
    let (stream, notice) = workstream::open(&client(&url), "/work/root").await;
    assert!(stream.is_none());
    assert!(notice.contains("workstream.attach"), "{notice}");
    assert!(posted(&requests).is_empty());
}

#[tokio::test]
async fn a_refused_exit_checkpoint_is_reported_and_does_not_detach() {
    let attach = [("workstream.attach", json!({"context": ""}))];
    let (url, requests) = fixture(&attach, "alpha").await;
    let client = client(&url);
    let (stream, context) = workstream::open(&client, "/r").await;
    assert_eq!(context, "");
    let notice = workstream::close(&client, stream.unwrap()).await;
    assert!(notice.contains("workstream.checkpoint"), "{notice}");
    assert_eq!(posted(&requests).len(), 1, "only the attach reached Arbora");
}

#[tokio::test]
async fn a_refused_detach_after_a_recorded_checkpoint_is_reported() {
    let served = [
        ("workstream.attach", json!({"context": "c"})),
        ("workstream.checkpoint", json!({"ok": true})),
    ];
    let (url, requests) = fixture(&served, "alpha").await;
    let client = client(&url);
    let (stream, _) = workstream::open(&client, "/r").await;
    let notice = workstream::close(&client, stream.unwrap()).await;
    assert!(notice.contains("Workstream unavailable"), "{notice}");
    assert_eq!(posted(&requests).len(), 2);
}
