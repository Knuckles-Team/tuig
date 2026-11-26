//! Host effects behind the Arbora approval exchange (TUIG-RUNTIME-R011).
//!
//! Every effect runs on the person's host only after containment and an
//! explicit decision recorded by `CodingApprovalState`. Nothing here grants
//! authority: the capability set and the admission come from Arbora.

use crate::acp::MAX_OUTPUT_BYTES;
use crate::coding_approval::{
    ApprovalError, ApprovalRequest, Capability, Choice, CodingApprovalState, CodingSessionView,
    DecisionRecord, RequestContent, contained_path,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

const MAX_COMMAND_BYTES: usize = 4096;

/// Open a session only for a root that Arbora admitted.
pub fn open_session(
    session_id: &str,
    root: &str,
    capabilities: BTreeSet<Capability>,
    root_trusted: bool,
) -> Result<CodingSessionView, ApprovalError> {
    if !root_trusted {
        return Err(ApprovalError::RootNotTrusted);
    }
    Ok(CodingSessionView::new(
        session_id.to_owned(),
        root.to_owned(),
        capabilities,
    ))
}

fn clean(text: &str) -> Result<(), ApprovalError> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(ApprovalError::InvalidMetadata);
    }
    Ok(())
}

fn canonical_root(root: &str) -> Result<PathBuf, ApprovalError> {
    Path::new(root)
        .canonicalize()
        .map_err(|_| ApprovalError::InvalidMetadata)
}

/// Resolve a write target, which may not exist yet, strictly inside the root.
fn contained_write_target(root: &str, path: &str) -> Result<PathBuf, ApprovalError> {
    clean(path)?;
    if let Ok(existing) = contained_path(root, path) {
        return Ok(existing);
    }
    let root = canonical_root(root)?;
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let name = joined.file_name().ok_or(ApprovalError::PathOutsideRoot)?;
    let parent = joined
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .filter(|parent| parent.starts_with(&root))
        .ok_or(ApprovalError::PathOutsideRoot)?;
    if joined.symlink_metadata().is_ok() {
        return Err(ApprovalError::PathOutsideRoot);
    }
    Ok(parent.join(name))
}

fn contained_directory(root: &str, dir: &str) -> Result<PathBuf, ApprovalError> {
    clean(dir)?;
    let root = canonical_root(root)?;
    let candidate = if Path::new(dir).is_absolute() {
        PathBuf::from(dir)
    } else {
        root.join(dir)
    };
    candidate
        .canonicalize()
        .ok()
        .filter(|resolved| resolved.starts_with(&root) && resolved.is_dir())
        .ok_or(ApprovalError::PathOutsideRoot)
}

fn request(
    state: &CodingApprovalState,
    request_id: &str,
    content: RequestContent,
) -> ApprovalRequest {
    ApprovalRequest {
        session_id: state.session_id().to_owned(),
        request_id: request_id.to_owned(),
        workspace_root: state.workspace_root().to_owned(),
        content,
    }
}

// contract: TUIG-RUNTIME-R011.4
pub fn propose_write(
    state: &mut CodingApprovalState,
    request_id: &str,
    path: &str,
    new_text: &str,
) -> Result<(), ApprovalError> {
    let resolved = contained_write_target(state.workspace_root(), path)?;
    let old_text = match std::fs::read_to_string(&resolved) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err(ApprovalError::InvalidMetadata),
    };
    let content = RequestContent::Write {
        resolved_path: resolved.to_string_lossy().into_owned(),
        old_text,
        new_text: new_text.to_owned(),
    };
    let pending = request(state, request_id, content);
    state.receive(pending)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectOutcome {
    pub record: DecisionRecord,
    pub applied: bool,
}

impl EffectOutcome {
    /// A denial reaches the gateway as an error, never as success.
    pub fn refusal(&self) -> Option<ApprovalError> {
        (self.record.choice == Choice::Deny).then_some(ApprovalError::ApprovalDenied)
    }
}

// contract: TUIG-RUNTIME-R011.4
pub fn decide_write(
    state: &mut CodingApprovalState,
    choice: Choice,
    actor: &str,
) -> Result<EffectOutcome, ApprovalError> {
    let preview = state.preview().ok_or(ApprovalError::RequestChanged)?;
    let RequestContent::Write {
        resolved_path,
        old_text,
        new_text,
    } = preview.request().content.clone()
    else {
        return Err(ApprovalError::CapabilityNotGranted);
    };
    let current = std::fs::read_to_string(&resolved_path).unwrap_or_default();
    if current != old_text {
        return Err(ApprovalError::RequestChanged);
    }
    let record = state.decide(&preview, choice, actor)?;
    if choice == Choice::Deny {
        return Ok(EffectOutcome {
            record,
            applied: false,
        });
    }
    let target = contained_write_target(state.workspace_root(), &resolved_path)?;
    if target.to_string_lossy() != resolved_path {
        return Err(ApprovalError::PathOutsideRoot);
    }
    std::fs::write(&target, new_text).map_err(|_| ApprovalError::InvalidMetadata)?;
    Ok(EffectOutcome {
        record,
        applied: true,
    })
}

/// A stable identity for the command line; it is not a security digest.
fn command_digest(command: &str, directory: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in command.bytes().chain([0]).chain(directory.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

// contract: TUIG-RUNTIME-R011.5
pub fn propose_command(
    state: &mut CodingApprovalState,
    request_id: &str,
    command: &str,
    working_directory: &str,
    danger: &str,
) -> Result<(), ApprovalError> {
    if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return Err(ApprovalError::InvalidMetadata);
    }
    let directory = contained_directory(state.workspace_root(), working_directory)?;
    let working_directory = directory.to_string_lossy().into_owned();
    let content = RequestContent::Command {
        command_digest: command_digest(command, &working_directory),
        command: command.to_owned(),
        working_directory,
        danger: danger.to_owned(),
    };
    let pending = request(state, request_id, content);
    state.receive(pending)
}

/// Decide the pending command and spawn it only after approval.
// contract: TUIG-RUNTIME-R011.5
pub async fn decide_command(
    state: &mut CodingApprovalState,
    choice: Choice,
    actor: &str,
    output_limit: usize,
) -> Result<(EffectOutcome, Option<HostTerminal>), ApprovalError> {
    let preview = state.preview().ok_or(ApprovalError::RequestChanged)?;
    let RequestContent::Command {
        command,
        working_directory,
        ..
    } = preview.request().content.clone()
    else {
        return Err(ApprovalError::CapabilityNotGranted);
    };
    let directory = contained_directory(state.workspace_root(), &working_directory)?;
    let record = state.decide(&preview, choice, actor)?;
    if choice == Choice::Deny {
        let outcome = EffectOutcome {
            record,
            applied: false,
        };
        return Ok((outcome, None));
    }
    let terminal = HostTerminal::spawn(&command, &directory, output_limit)?;
    let outcome = EffectOutcome {
        record,
        applied: true,
    };
    Ok((outcome, Some(terminal)))
}

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

/// One approved process whose output is captured in arrival order.
pub struct HostTerminal {
    child: Child,
    captured: Arc<Mutex<Captured>>,
    reader: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalOutput {
    pub text: String,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub exited: bool,
}

impl TerminalOutput {
    pub fn to_json(&self) -> Value {
        let status = self.exited.then(|| json!({"exitCode": self.exit_code}));
        json!({"output": self.text, "truncated": self.truncated, "exitStatus": status})
    }
}

fn capture(captured: &Mutex<Captured>, chunk: &[u8], limit: usize) {
    let mut captured = captured
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let room = limit.saturating_sub(captured.bytes.len());
    if chunk.len() > room {
        captured.truncated = true;
    }
    captured
        .bytes
        .extend_from_slice(&chunk[..chunk.len().min(room)]);
}

async fn pump(
    mut stdout: impl AsyncRead + Unpin,
    mut stderr: impl AsyncRead + Unpin,
    captured: Arc<Mutex<Captured>>,
    limit: usize,
) {
    let (mut out_open, mut err_open) = (true, true);
    let (mut out_buf, mut err_buf) = ([0_u8; 4096], [0_u8; 4096]);
    while out_open || err_open {
        tokio::select! {
            read = stdout.read(&mut out_buf), if out_open => match read {
                Ok(count) if count > 0 => capture(&captured, &out_buf[..count], limit),
                _ => out_open = false,
            },
            read = stderr.read(&mut err_buf), if err_open => match read {
                Ok(count) if count > 0 => capture(&captured, &err_buf[..count], limit),
                _ => err_open = false,
            },
        }
    }
}

impl HostTerminal {
    // contract: TUIG-RUNTIME-R011.6
    fn spawn(command: &str, directory: &Path, limit: usize) -> Result<Self, ApprovalError> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| ApprovalError::InvalidMetadata)?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(ApprovalError::InvalidMetadata);
        };
        let captured = Arc::new(Mutex::new(Captured::default()));
        let limit = limit.min(MAX_OUTPUT_BYTES);
        let reader = tokio::spawn(pump(stdout, stderr, captured.clone(), limit));
        Ok(Self {
            child,
            captured,
            reader: Some(reader),
        })
    }

    /// `terminal/output`: what has arrived so far, with a typed truncation flag.
    pub fn output(&mut self) -> TerminalOutput {
        let status = self.child.try_wait().ok().flatten();
        let captured = self
            .captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TerminalOutput {
            text: String::from_utf8_lossy(&captured.bytes).into_owned(),
            truncated: captured.truncated,
            exit_code: status.and_then(|status| status.code()),
            exited: status.is_some(),
        }
    }

    /// `terminal/wait_for_exit`: returns once the process ended and its output drained.
    pub async fn wait_for_exit(&mut self) -> Result<Option<i32>, ApprovalError> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|_| ApprovalError::InvalidMetadata)?;
        if let Some(reader) = self.reader.take() {
            let _ = reader.await;
        }
        Ok(status.code())
    }

    /// `terminal/kill`: ends the process and waits for it.
    pub async fn kill(&mut self) -> Result<(), ApprovalError> {
        self.child
            .kill()
            .await
            .map_err(|_| ApprovalError::InvalidMetadata)
    }
}

/// The typed reason that reaches the gateway for each refusal.
// contract: TUIG-RUNTIME-R011.10
pub fn refusal_reason(error: &ApprovalError) -> &'static str {
    match error {
        ApprovalError::PathOutsideRoot => "PATH_OUTSIDE_ROOT",
        ApprovalError::ApprovalDenied => "APPROVAL_DENIED",
        ApprovalError::RootNotTrusted => "ROOT_NOT_TRUSTED",
        ApprovalError::GatewayUnavailable => "GATEWAY_UNAVAILABLE",
        _ => "CAPABILITY_NOT_GRANTED",
    }
}

// contract: TUIG-RUNTIME-R011.10
pub fn refusal_response(id: &Value, error: &ApprovalError) -> Value {
    let reason = refusal_reason(error);
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": -32001, "message": reason, "data": {"reason": reason}},
    })
}

/// What survives a restart: identity and the pending request's name, never file bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub session_id: String,
    pub workspace_root: String,
    pub pending_request_id: Option<String>,
}

impl SessionRecord {
    pub fn capture(state: &CodingApprovalState) -> Self {
        Self {
            session_id: state.session_id().to_owned(),
            workspace_root: state.workspace_root().to_owned(),
            pending_request_id: state.pending_request_id().map(str::to_owned),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "workspaceRoot": self.workspace_root,
            "pendingRequestId": self.pending_request_id,
        })
    }

    pub fn from_json(value: &Value) -> Result<Self, ApprovalError> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            session_id: text("sessionId").ok_or(ApprovalError::InvalidMetadata)?,
            workspace_root: text("workspaceRoot").ok_or(ApprovalError::InvalidMetadata)?,
            pending_request_id: text("pendingRequestId"),
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ApprovalError> {
        let bytes =
            serde_json::to_vec(&self.to_json()).map_err(|_| ApprovalError::InvalidMetadata)?;
        std::fs::write(path, bytes).map_err(|_| ApprovalError::InvalidMetadata)
    }

    pub fn load(path: &Path) -> Result<Self, ApprovalError> {
        let bytes = std::fs::read(path).map_err(|_| ApprovalError::InvalidMetadata)?;
        let value = serde_json::from_slice(&bytes).map_err(|_| ApprovalError::InvalidMetadata)?;
        Self::from_json(&value)
    }

    /// Reattach to the same session. The capability set comes from the producer
    /// again; the restart itself grants nothing and the pending request must be re-sent.
    // contract: TUIG-RUNTIME-R011.7
    pub fn reattach(&self, producer_grant: BTreeSet<Capability>) -> CodingApprovalState {
        CodingApprovalState::new(
            self.session_id.clone(),
            self.workspace_root.clone(),
            producer_grant,
        )
    }
}
