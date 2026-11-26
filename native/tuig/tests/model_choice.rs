//! TUIG-RUNTIME-R012.2.2 and R012.4: the model picker and the served command matrix.
//!
//! The fixture serves one registry and the operations it advertises, answers
//! `message/send` with a task naming a profile, and records every request so a
//! test can prove which routes received traffic.
use serde_json::{Value, json};
use support::fixture;
use tuig_native::{
    ArboraClient, TaskSubmission,
    model_choice::{ModelAction, ModelChoice, run},
    served_command::{MATRIX, ServedCommand},
};

mod support;

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
