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
