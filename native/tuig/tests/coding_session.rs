//! TUIG-RUNTIME-R011: coding session model, host serving and containment.
use std::collections::BTreeSet;
use std::path::PathBuf;
use tuig_native::acp::serve_local_read;
use tuig_native::coding_approval::{
    ApprovalError, ApprovalRequest, Capability, CodingSessionView, RequestContent, contained_path,
};

struct TempDir(PathBuf);
impl TempDir {
    fn create(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "tuig-coding-session-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_request(root: &str, session: &str, id: &str) -> ApprovalRequest {
    ApprovalRequest {
        session_id: session.to_owned(),
        request_id: id.to_owned(),
        workspace_root: root.to_owned(),
        content: RequestContent::Write {
            resolved_path: format!("{root}/note.txt"),
            old_text: String::new(),
            new_text: "changed".to_owned(),
        },
    }
}

// spec: TUIG-RUNTIME-R011.1
#[test]
fn test_session_model_closed_capability_set() {
    // The closed set: every known method parses to its capability.
    assert_eq!(
        Capability::parse("fs/read_text_file"),
        Ok(Capability::ReadText)
    );
    assert_eq!(
        Capability::parse("fs/write_text_file"),
        Ok(Capability::WriteText)
    );
    assert_eq!(
        Capability::parse("terminal/create"),
        Ok(Capability::CreateTerminal)
    );
    assert_eq!(
        Capability::parse("terminal/output"),
        Ok(Capability::TerminalOutput)
    );
    assert_eq!(
        Capability::parse("terminal/wait_for_exit"),
        Ok(Capability::WaitForExit)
    );
    assert_eq!(
        Capability::parse("terminal/kill"),
        Ok(Capability::KillTerminal)
    );

    // Unknown methods map to a typed refusal.
    for unknown in [
        "",
        "fs/delete_file",
        "terminal/paste",
        "process/spawn",
        "fs/read_text_file\n",
        "FS/read_text_file",
    ] {
        assert_eq!(
            Capability::parse(unknown),
            Err(ApprovalError::CapabilityNotGranted),
            "{unknown:?}"
        );
    }

    // The view holds the producer grant and receives Arbora decisions;
    // it holds no policy of its own.
    let root = "/workspace";
    let granted = BTreeSet::from([Capability::ReadText, Capability::WriteText]);
    let mut view = CodingSessionView::new("session-1".to_owned(), root.to_owned(), granted);
    assert_eq!(view.session_id(), "session-1");
    assert_eq!(view.workspace_root(), "/workspace");
    assert!(view.allows("fs/read_text_file"));
    assert!(view.allows("fs/write_text_file"));
    assert!(!view.allows("terminal/create"));
    assert!(!view.allows("fs/delete_file"));

    // Not connected yet: decisions from Arbora are refused.
    assert_eq!(
        view.receive(write_request(root, "session-1", "req-1")),
        Err(ApprovalError::GatewayUnavailable)
    );
    view.set_connected(true);

    // A write for a capability the producer did not grant is refused.
    let mut ungranted = CodingSessionView::new(
        "session-1".to_owned(),
        root.to_owned(),
        BTreeSet::from([Capability::ReadText]),
    );
    ungranted.set_connected(true);
    assert_eq!(
        ungranted.receive(write_request(root, "session-1", "req-1")),
        Err(ApprovalError::CapabilityNotGranted)
    );
    assert!(ungranted.pending().is_none());

    // A granted write is held as the pending request.
    assert!(
        view.receive(write_request(root, "session-1", "req-1"))
            .is_ok()
    );
    assert!(view.pending().is_some_and(|p| {
        p.request_id == "req-1"
            && matches!(
                &p.content,
                RequestContent::Write {
                    resolved_path, ..
                } if resolved_path.ends_with("note.txt")
            )
    }));
}

// spec: TUIG-RUNTIME-R011.2
#[test]
fn test_capability_handlers_serve_local_host() {
    let root = TempDir::create("host");
    std::fs::write(root.path().join("note.txt"), "host text\n").unwrap();

    let granted = BTreeSet::from([Capability::ReadText]);
    let view = CodingSessionView::new(
        "session-1".to_owned(),
        root.path().to_string_lossy().to_string(),
        granted,
    );

    // A read inside the root returns the file text and prompts zero times.
    let text = serve_local_read(&view, "fs/read_text_file", "note.txt").unwrap();
    assert_eq!(text, "host text\n");
    assert!(view.pending().is_none(), "the read served without a prompt");

    // Unknown methods are refused with a typed error.
    assert_eq!(
        serve_local_read(&view, "fs/delete_file", "note.txt"),
        Err(ApprovalError::CapabilityNotGranted)
    );

    // Write and terminal effects are never served locally.
    assert_eq!(
        serve_local_read(&view, "fs/write_text_file", "note.txt"),
        Err(ApprovalError::CapabilityNotGranted)
    );
    assert_eq!(
        serve_local_read(&view, "terminal/create", "sh"),
        Err(ApprovalError::CapabilityNotGranted)
    );

    // No local policy bypass may grant authority: an empty grant refuses.
    let empty = CodingSessionView::new(
        "session-1".to_owned(),
        root.path().to_string_lossy().to_string(),
        BTreeSet::new(),
    );
    assert_eq!(
        serve_local_read(&empty, "fs/read_text_file", "note.txt"),
        Err(ApprovalError::CapabilityNotGranted)
    );
}

// spec: TUIG-RUNTIME-R011.3
#[test]
fn test_path_containment_refuses_escape() {
    let root = TempDir::create("contain");
    std::fs::write(root.path().join("inner.txt"), "inside\n").unwrap();
    let root_str = root.path().to_string_lossy().to_string();

    let outside = std::env::temp_dir().join(format!(
        "tuig-coding-session-outside-{}",
        std::process::id()
    ));
    std::fs::write(&outside, "outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, root.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(
        root.path().join("inner.txt"),
        root.path().join("inner_link"),
    )
    .unwrap();

    let granted = BTreeSet::from([Capability::ReadText]);
    let view = CodingSessionView::new("session-1".to_owned(), root_str.clone(), granted);

    // Parent segments are refused.
    let outside_name = outside.file_name().unwrap().to_string_lossy();
    assert_eq!(
        contained_path(&root_str, &format!("../{outside_name}")),
        Err(ApprovalError::CapabilityNotGranted)
    );

    // Absolute paths outside the root are refused.
    assert_eq!(
        contained_path(&root_str, outside.to_string_lossy().as_ref()),
        Err(ApprovalError::CapabilityNotGranted)
    );

    // A symlink leaving the root is refused; the handler reads zero bytes.
    assert_eq!(
        contained_path(&root_str, "escape"),
        Err(ApprovalError::CapabilityNotGranted)
    );
    assert_eq!(
        serve_local_read(&view, "fs/read_text_file", "escape"),
        Err(ApprovalError::CapabilityNotGranted)
    );

    // A symlink that stays inside the root resolves to the inner file.
    let resolved = contained_path(&root_str, "inner_link").unwrap();
    assert_eq!(std::fs::read_to_string(&resolved).unwrap(), "inside\n");
    assert_eq!(
        serve_local_read(&view, "fs/read_text_file", "inner_link").unwrap(),
        "inside\n"
    );

    // A relative target inside the root is served.
    assert_eq!(
        serve_local_read(&view, "fs/read_text_file", "inner.txt").unwrap(),
        "inside\n"
    );

    let _ = std::fs::remove_file(&outside);
}

fn session_state(root: &TempDir) -> tuig_native::coding_approval::CodingApprovalState {
    let mut state = tuig_native::coding_approval::CodingApprovalState::new(
        "session-1".to_owned(),
        root.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        BTreeSet::from([
            Capability::ReadText,
            Capability::WriteText,
            Capability::CreateTerminal,
        ]),
    );
    state.set_connected(true);
    state
}

// spec: TUIG-RUNTIME-R011.4
#[test]
fn test_write_shows_diff_then_applies_on_approval() {
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_effects::{decide_write, propose_write};
    let root = TempDir::create("write");
    let note = root.path().join("note.txt");
    std::fs::write(&note, "old\n").unwrap();
    let mut state = session_state(&root);

    propose_write(&mut state, "w1", "note.txt", "new\n").unwrap();
    let text = state.preview().unwrap().text();
    assert!(text.contains("-old\n") && text.contains("+new\n"), "{text}");
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "old\n");

    let denied = decide_write(&mut state, Choice::Deny, "person").unwrap();
    assert!(!denied.applied);
    assert_eq!(denied.refusal(), Some(ApprovalError::ApprovalDenied));
    assert_eq!(std::fs::read(&note).unwrap(), b"old\n");

    propose_write(&mut state, "w2", "note.txt", "new\n").unwrap();
    let approved = decide_write(&mut state, Choice::Approve, "person").unwrap();
    assert!(approved.applied && approved.refusal().is_none());
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "new\n");

    propose_write(&mut state, "w3", "fresh.txt", "created\n").unwrap();
    assert!(!root.path().join("fresh.txt").exists());
    decide_write(&mut state, Choice::Approve, "person").unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("fresh.txt")).unwrap(),
        "created\n"
    );

    propose_write(&mut state, "w4", "note.txt", "stale\n").unwrap();
    std::fs::write(&note, "changed elsewhere\n").unwrap();
    assert_eq!(
        decide_write(&mut state, Choice::Approve, "person").unwrap_err(),
        ApprovalError::RequestChanged
    );
    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        "changed elsewhere\n"
    );
}

// spec: TUIG-RUNTIME-R011.5
#[tokio::test]
async fn test_command_displayed_before_run() {
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_effects::{decide_command, propose_command};
    let root = TempDir::create("command");
    let mut state = session_state(&root);
    let marker = root.path().join("ran");

    propose_command(&mut state, "c1", "touch ran", ".", "writes a file").unwrap();
    let text = state.preview().unwrap().text();
    for expected in [
        "Command: touch ran",
        "Working directory: ",
        "Danger: writes a file",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(!marker.exists(), "nothing runs before the decision");

    let (denied, terminal) = decide_command(&mut state, Choice::Deny, "person", 1024)
        .await
        .unwrap();
    assert!(terminal.is_none() && !denied.applied);
    assert!(!marker.exists(), "a denied command spawns zero processes");

    propose_command(&mut state, "c2", "touch ran", ".", "writes a file").unwrap();
    let (approved, terminal) = decide_command(&mut state, Choice::Approve, "person", 1024)
        .await
        .unwrap();
    assert!(approved.applied);
    assert_eq!(terminal.unwrap().wait_for_exit().await, Ok(Some(0)));
    assert!(marker.exists());

    assert_eq!(
        propose_command(&mut state, "c3", "ls", "..", "reads").unwrap_err(),
        ApprovalError::PathOutsideRoot
    );
    assert_eq!(
        propose_command(&mut state, "c4", "  ", ".", "reads").unwrap_err(),
        ApprovalError::InvalidMetadata
    );
}

// spec: TUIG-RUNTIME-R011.6
#[tokio::test]
async fn test_terminal_output_streams_in_order() {
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_effects::{decide_command, propose_command};
    let root = TempDir::create("terminal");
    let mut state = session_state(&root);

    propose_command(
        &mut state,
        "t1",
        "printf one; sleep 1; printf two",
        ".",
        "reads",
    )
    .unwrap();
    let (_, terminal) = decide_command(&mut state, Choice::Approve, "person", 1024)
        .await
        .unwrap();
    let mut terminal = terminal.unwrap();
    let mut seen = terminal.output();
    for _ in 0..100 {
        if seen.text.contains("one") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        seen = terminal.output();
    }
    assert_eq!(seen.text, "one");
    assert!(!seen.exited, "output renders while the process still runs");
    assert_eq!(terminal.wait_for_exit().await, Ok(Some(0)));
    let done = terminal.output();
    assert_eq!(done.text, "onetwo");
    assert!(done.exited && !done.truncated);
    assert_eq!(done.to_json()["exitStatus"]["exitCode"], 0);

    propose_command(&mut state, "t2", "yes x | head -c 20000", ".", "reads").unwrap();
    let (_, terminal) = decide_command(&mut state, Choice::Approve, "person", 100)
        .await
        .unwrap();
    let mut terminal = terminal.unwrap();
    terminal.wait_for_exit().await.unwrap();
    let limited = terminal.output();
    assert_eq!(limited.text.len(), 100);
    assert!(limited.truncated);
    assert_eq!(limited.to_json()["truncated"], true);

    propose_command(&mut state, "t3", "sleep 30", ".", "reads").unwrap();
    let (_, terminal) = decide_command(&mut state, Choice::Approve, "person", 100)
        .await
        .unwrap();
    let mut terminal = terminal.unwrap();
    terminal.kill().await.unwrap();
    assert_eq!(terminal.wait_for_exit().await, Ok(None));
    assert!(terminal.output().exited);
}

// spec: TUIG-RUNTIME-R011.7
#[test]
fn test_resume_reattaches_without_granting() {
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_effects::{SessionRecord, decide_write, propose_write};
    let root = TempDir::create("resume");
    let note = root.path().join("note.txt");
    std::fs::write(&note, "old\n").unwrap();
    let mut before = session_state(&root);
    propose_write(&mut before, "w1", "note.txt", "new\n").unwrap();
    let shown = before.preview().unwrap().text();
    let record = SessionRecord::capture(&before);
    let saved = root.path().join("session.json");
    record.save(&saved).unwrap();
    drop(before);

    let loaded = SessionRecord::load(&saved).unwrap();
    assert_eq!(loaded, record);
    assert_eq!(loaded.pending_request_id.as_deref(), Some("w1"));
    assert!(!std::fs::read_to_string(&saved).unwrap().contains("new"));

    let mut ungranted = loaded.reattach(BTreeSet::new());
    ungranted.set_connected(true);
    assert_eq!(
        propose_write(&mut ungranted, "w1", "note.txt", "new\n").unwrap_err(),
        ApprovalError::CapabilityNotGranted
    );
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "old\n");

    let mut resumed = loaded.reattach(BTreeSet::from([Capability::WriteText]));
    resumed.set_connected(true);
    propose_write(&mut resumed, "w1", "note.txt", "new\n").unwrap();
    assert_eq!(resumed.preview().unwrap().text(), shown);
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "old\n");
    decide_write(&mut resumed, Choice::Approve, "person").unwrap();
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "new\n");
}

// spec: TUIG-RUNTIME-R011.10
#[test]
fn test_typed_refusals_reach_gateway() {
    use serde_json::json;
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_effects::{decide_write, open_session, propose_write, refusal_response};
    let root = TempDir::create("refusal");
    let note = root.path().join("note.txt");
    std::fs::write(&note, "old\n").unwrap();
    let root_text = root.path().to_string_lossy().into_owned();
    let mut state = session_state(&root);

    let outside = propose_write(&mut state, "r1", "../escape.txt", "x").unwrap_err();
    propose_write(&mut state, "r2", "note.txt", "new\n").unwrap();
    let denied = decide_write(&mut state, Choice::Deny, "person")
        .unwrap()
        .refusal()
        .unwrap();
    let untrusted = open_session("session-1", &root_text, BTreeSet::new(), false)
        .err()
        .unwrap();
    let mut disconnected = session_state(&root);
    disconnected.set_connected(false);
    let gateway = propose_write(&mut disconnected, "r3", "note.txt", "new\n").unwrap_err();
    let mut ungranted = tuig_native::coding_approval::CodingApprovalState::new(
        "session-1".to_owned(),
        state.workspace_root().to_owned(),
        BTreeSet::new(),
    );
    ungranted.set_connected(true);
    let capability = propose_write(&mut ungranted, "r4", "note.txt", "new\n").unwrap_err();

    for (error, reason) in [
        (outside, "PATH_OUTSIDE_ROOT"),
        (capability, "CAPABILITY_NOT_GRANTED"),
        (denied, "APPROVAL_DENIED"),
        (untrusted, "ROOT_NOT_TRUSTED"),
        (gateway, "GATEWAY_UNAVAILABLE"),
    ] {
        let response = refusal_response(&json!(7), &error);
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["message"], reason);
        assert_eq!(response["error"]["data"]["reason"], reason);
        assert!(response.get("result").is_none());
    }
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "old\n");
    assert!(!root.path().parent().unwrap().join("escape.txt").exists());
}

// spec: TUIG-RUNTIME-R011.11
#[test]
fn test_evidence_record_has_no_secrets() {
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_effects::{decide_write, propose_write};
    let root = TempDir::create("evidence");
    std::fs::write(root.path().join("note.txt"), "old-secret-body\n").unwrap();
    let mut state = session_state(&root);
    propose_write(&mut state, "e1", "note.txt", "new-secret-body\n").unwrap();
    decide_write(&mut state, Choice::Approve, "person").unwrap();
    let evidence = state.evidence_text();
    for expected in [
        "Session: session-1",
        "Request: e1",
        "Method: fs/write_text_file",
        "note.txt",
        "Decision: Approve",
        "Decided by: person",
    ] {
        assert!(evidence.contains(expected), "{evidence}");
    }
    assert!(!evidence.contains("secret-body"));
    assert_eq!(state.records().len(), 1);
}

mod stream {
    use super::{Capability, TempDir};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split};
    use tuig_native::acp::{HostRequest, TextClient};
    use tuig_native::coding_approval::Choice;
    use tuig_native::host_router::HostRouter;

    fn all() -> BTreeSet<Capability> {
        Capability::ALL.into()
    }

    /// Run one turn in which the agent sends a single callback request and
    /// report the frame the client wrote back and the turn's outcome.
    async fn callback_turn(
        method: &'static str,
        host_answers: bool,
    ) -> (Value, Result<Value, &'static str>, bool) {
        let (local, remote) = tokio::io::duplex(8192);
        let (reader, writer) = split(local);
        let (remote_reader, mut remote_writer) = split(remote);
        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(remote_reader).lines();
            for (id, result) in [
                (1, json!({"protocolVersion":1})),
                (2, json!({"sessionId":"s"})),
            ] {
                lines.next_line().await.unwrap().unwrap();
                let frame = json!({"jsonrpc":"2.0","id":id,"result":result});
                remote_writer
                    .write_all(format!("{frame}\n").as_bytes())
                    .await
                    .unwrap();
            }
            lines.next_line().await.unwrap().unwrap();
            let request = json!({"jsonrpc":"2.0","id":9,"method":method,"params":{}});
            remote_writer
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            serde_json::from_str::<Value>(&lines.next_line().await.unwrap().unwrap()).unwrap()
        });
        let mut client = TextClient::new(reader, writer).with_host_capabilities();
        assert_eq!(client.session_id(), None);
        client.initialize("/workspace").await.unwrap();
        assert_eq!(client.session_id(), Some("s"));
        client.prompt("go").await.unwrap();
        let mut consulted = false;
        let outcome = client
            .response_hosted(
                3,
                async |_| Ok(()),
                async |request: HostRequest| {
                    consulted = true;
                    host_answers.then(|| json!({"jsonrpc":"2.0","id":request.id,"result":{}}))
                },
            )
            .await;
        (agent.await.unwrap(), outcome, consulted)
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn an_unanswered_capability_request_keeps_the_refusal() {
        let (frame, outcome, consulted) = callback_turn("fs/read_text_file", false).await;
        assert!(consulted);
        assert_eq!(frame["error"]["code"], -32601);
        assert!(outcome.is_err());
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn a_method_outside_the_closed_set_never_reaches_the_host() {
        let (frame, outcome, consulted) = callback_turn("session/request_permission", true).await;
        assert!(!consulted);
        assert_eq!(frame["result"]["outcome"]["outcome"], "cancelled");
        assert!(outcome.is_err());
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn test_requests_reach_view_from_client_stream() {
        let root = TempDir::create("stream");
        let cwd = root.path().canonicalize().unwrap();
        let cwd_text = cwd.to_string_lossy().into_owned();
        let (local, remote) = tokio::io::duplex(8192);
        let (reader, writer) = split(local);
        let (remote_reader, mut remote_writer) = split(remote);
        let agent_root = cwd_text.clone();
        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(remote_reader).lines();
            let initialize: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(initialize["params"]["clientCapabilities"]["terminal"], true);
            for frame in [
                json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}),
                json!({"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}),
            ] {
                if frame["id"] == 2 {
                    let create = lines.next_line().await.unwrap().unwrap();
                    assert!(create.contains(&agent_root));
                }
                remote_writer
                    .write_all(format!("{frame}\n").as_bytes())
                    .await
                    .unwrap();
            }
            let _prompt = lines.next_line().await.unwrap().unwrap();
            let mut answers = Vec::new();
            for (id, path) in [(7, "note.txt"), (8, "../outside.txt")] {
                let request = json!({"jsonrpc":"2.0","id":id,"method":"fs/write_text_file","params":{"sessionId":"s","path":path,"content":"changed"}});
                remote_writer
                    .write_all(format!("{request}\n").as_bytes())
                    .await
                    .unwrap();
                answers.push(
                    serde_json::from_str::<Value>(&lines.next_line().await.unwrap().unwrap())
                        .unwrap(),
                );
            }
            let done = json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}});
            remote_writer
                .write_all(format!("{done}\n").as_bytes())
                .await
                .unwrap();
            answers
        });

        let mut router = HostRouter::new("s", &cwd_text, all());
        let mut client = TextClient::new(reader, writer).with_host_capabilities();
        client.initialize(&cwd_text).await.unwrap();
        client.prompt("edit").await.unwrap();
        let mut shown = Vec::new();
        let mut pending_ids = Vec::new();
        let mut ask = async |preview: tuig_native::coding_approval::ApprovalPreview| {
            shown.push(preview.text());
            Choice::Approve
        };
        let result = client
            .response_hosted(
                3,
                async |_| Ok(()),
                async |request: HostRequest| {
                    let frame = router.handle(&request, &mut ask).await;
                    pending_ids.push(router.state().pending_request_id().map(str::to_owned));
                    Some(frame)
                },
            )
            .await
            .unwrap();
        assert_eq!(result["stopReason"], "end_turn");

        let answers = agent.await.unwrap();
        assert_eq!(shown.len(), 1, "only the contained write reaches the view");
        assert!(shown[0].contains("+changed"));
        assert_eq!(
            std::fs::read_to_string(cwd.join("note.txt")).unwrap(),
            "changed"
        );
        assert_eq!(answers[0]["result"], Value::Null);
        assert_eq!(answers[0]["id"], 7);
        assert_eq!(answers[1]["error"]["data"]["reason"], "PATH_OUTSIDE_ROOT");
        assert!(!cwd.parent().unwrap().join("outside.txt").exists());
        assert_eq!(
            pending_ids,
            [None, None],
            "each request is decided before the next"
        );
        assert_eq!(router.state().records().len(), 1);
    }
}

mod router {
    use super::{Capability, TempDir};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use tuig_native::acp::HostRequest;
    use tuig_native::coding_approval::{ApprovalPreview, Choice};
    use tuig_native::host_router::HostRouter;

    fn request(method: &str, params: Value) -> HostRequest {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(11);
        HostRequest {
            id: json!(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
            method: method.to_owned(),
            params,
        }
    }

    fn open(root: &TempDir, capabilities: BTreeSet<Capability>) -> HostRouter {
        let resolved = root.path().canonicalize().unwrap();
        HostRouter::new("s", &resolved.to_string_lossy(), capabilities)
    }

    async fn answer(router: &mut HostRouter, request: HostRequest, choice: Choice) -> Value {
        let mut ask = async |_preview: ApprovalPreview| choice;
        router.handle(&request, &mut ask).await
    }

    fn reason(frame: &Value) -> &str {
        frame["error"]["data"]["reason"].as_str().unwrap()
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn reads_are_served_without_a_prompt_and_escapes_are_refused() {
        let root = TempDir::create("router-read");
        std::fs::write(root.path().join("a.txt"), "inside").unwrap();
        let outside = TempDir::create("router-read-outside");
        std::fs::write(outside.path().join("b.txt"), "outside").unwrap();
        let mut router = open(&root, Capability::ALL.into());
        let mut prompted = false;
        let mut ask = async |_preview: ApprovalPreview| {
            prompted = true;
            Choice::Deny
        };
        let served = router
            .handle(
                &request("fs/read_text_file", json!({"path": "a.txt"})),
                &mut ask,
            )
            .await;
        assert_eq!(served["result"]["content"], "inside");
        let escaped = router
            .handle(
                &request("fs/read_text_file", json!({"path": "../a.txt"})),
                &mut ask,
            )
            .await;
        assert_eq!(reason(&escaped), "CAPABILITY_NOT_GRANTED");
        let missing = router
            .handle(&request("fs/read_text_file", json!({})), &mut ask)
            .await;
        assert_eq!(reason(&missing), "CAPABILITY_NOT_GRANTED");
        assert!(!prompted);
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn ungranted_and_unknown_methods_are_refused_without_a_prompt() {
        let root = TempDir::create("router-scope");
        let mut router = open(&root, BTreeSet::from([Capability::ReadText]));
        for method in ["fs/write_text_file", "terminal/create", "session/other"] {
            let frame = answer(&mut router, request(method, json!({})), Choice::Approve).await;
            assert_eq!(reason(&frame), "CAPABILITY_NOT_GRANTED");
            assert!(frame["id"].is_u64());
        }
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn a_denied_write_leaves_the_file_and_answers_with_the_denial() {
        let root = TempDir::create("router-deny");
        std::fs::write(root.path().join("a.txt"), "before").unwrap();
        let mut router = open(&root, Capability::ALL.into());
        let params = json!({"path": "a.txt", "content": "after"});
        let frame = answer(
            &mut router,
            request("fs/write_text_file", params),
            Choice::Deny,
        )
        .await;
        assert_eq!(reason(&frame), "APPROVAL_DENIED");
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "before"
        );
        let missing = answer(
            &mut router,
            request("fs/write_text_file", json!({"path": "a.txt"})),
            Choice::Approve,
        )
        .await;
        assert!(missing.get("error").is_some());
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn an_approved_command_streams_waits_and_is_killed_by_terminal_id() {
        let root = TempDir::create("router-terminal");
        let mut router = open(&root, Capability::ALL.into());
        let mut shown = Vec::new();
        let mut ask = async |preview: ApprovalPreview| {
            shown.push(preview.text());
            Choice::Approve
        };
        let created = router
            .handle(
                &request(
                    "terminal/create",
                    json!({"command": "printf", "args": ["it's %s", "a b"]}),
                ),
                &mut ask,
            )
            .await;
        assert_eq!(created["result"]["terminalId"], "term-0");
        let waited = router
            .handle(
                &request("terminal/wait_for_exit", json!({"terminalId": "term-0"})),
                &mut ask,
            )
            .await;
        assert_eq!(waited["result"]["exitCode"], 0);
        let output = router
            .handle(
                &request("terminal/output", json!({"terminalId": "term-0"})),
                &mut ask,
            )
            .await;
        assert_eq!(output["result"]["output"], "it's a b");
        assert_eq!(output["result"]["exitStatus"]["exitCode"], 0);
        let sleeper = router
            .handle(
                &request(
                    "terminal/create",
                    json!({"command": "sleep", "args": ["30"]}),
                ),
                &mut ask,
            )
            .await;
        assert_eq!(sleeper["result"]["terminalId"], "term-1");
        let killed = router
            .handle(
                &request("terminal/kill", json!({"terminalId": "term-1"})),
                &mut ask,
            )
            .await;
        assert_eq!(killed["result"], Value::Null);
        for bad in [
            json!({"terminalId": "term-9"}),
            json!({"terminalId": "nope"}),
            json!({}),
        ] {
            let frame = router
                .handle(&request("terminal/output", bad), &mut ask)
                .await;
            assert!(frame.get("error").is_some());
        }
        assert!(shown[0].contains("'a b'"));
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn a_denied_or_malformed_command_spawns_nothing() {
        let root = TempDir::create("router-command");
        let mut router = open(&root, Capability::ALL.into());
        let denied = answer(
            &mut router,
            request(
                "terminal/create",
                json!({"command": "touch", "args": ["made"]}),
            ),
            Choice::Deny,
        )
        .await;
        assert_eq!(reason(&denied), "APPROVAL_DENIED");
        for bad in [
            json!({}),
            json!({"command": "touch", "args": [1]}),
            json!({"command": "touch", "cwd": ".."}),
        ] {
            let frame = answer(
                &mut router,
                request("terminal/create", bad),
                Choice::Approve,
            )
            .await;
            assert!(frame.get("error").is_some());
        }
        assert!(!root.path().join("made").exists());
    }
}

mod malformed {
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split};
    use tuig_native::acp::TextClient;

    /// The turn outcome when the agent answers the prompt with one odd frame.
    async fn turn_with(frame: Value) -> Result<Value, &'static str> {
        let (local, remote) = tokio::io::duplex(8192);
        let (reader, writer) = split(local);
        let (remote_reader, mut remote_writer) = split(remote);
        tokio::spawn(async move {
            let mut lines = BufReader::new(remote_reader).lines();
            for (id, result) in [
                (1, json!({"protocolVersion":1})),
                (2, json!({"sessionId":"s"})),
            ] {
                lines.next_line().await.unwrap().unwrap();
                let reply = json!({"jsonrpc":"2.0","id":id,"result":result});
                remote_writer
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            lines.next_line().await.unwrap().unwrap();
            remote_writer
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
            let _ = lines.next_line().await;
        });
        let mut client = TextClient::new(reader, writer).with_host_capabilities();
        client.initialize("/workspace").await.unwrap();
        client.prompt("go").await.unwrap();
        client.response(3, async |_| Ok(())).await
    }

    fn update(content: Value) -> Value {
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":content}}})
    }

    // spec: TUIG-RUNTIME-R011.8
    #[tokio::test]
    async fn malformed_agent_frames_fail_closed() {
        let cases = [
            (
                json!({"jsonrpc":"2.0","method":"session/update","result":{}}),
                "Invalid ACP method envelope",
            ),
            (
                json!({"jsonrpc":"2.0","id":1.5,"method":"fs/read_text_file","params":{}}),
                "Invalid ACP callback identifier",
            ),
            (
                json!({"jsonrpc":"2.0","method":"session/other","params":{}}),
                "Unsupported ACP notification",
            ),
            (
                update(json!({"type":"image","text":"x"})),
                "Unsupported ACP session update",
            ),
            (
                update(json!({"type":"text","text":7})),
                "Invalid ACP text update",
            ),
            (
                json!({"jsonrpc":"1.0","id":3,"result":{}}),
                "Invalid ACP envelope",
            ),
            (
                json!({"jsonrpc":"2.0","id":3,"result":{},"error":{}}),
                "Mismatched ACP response",
            ),
            (
                json!({"jsonrpc":"2.0","id":3,"error":{"code":1,"message":"x"}}),
                "Arbora ACP refused the request; no automatic retry",
            ),
            (
                json!({"jsonrpc":"2.0","id":3,"result":[]}),
                "Invalid ACP result",
            ),
        ];
        for (frame, expected) in cases {
            assert_eq!(turn_with(frame).await, Err(expected));
        }
    }
}
