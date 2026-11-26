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
