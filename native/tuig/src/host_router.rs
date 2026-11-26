//! Routes the agent's client-capability requests into the approval view (TUIG-RUNTIME-R011.8).
//!
//! The router grants nothing: the session scope comes from the caller, every
//! effect passes containment and an explicit person's decision, and every
//! refusal leaves as a typed JSON-RPC error.
use crate::acp::{HostRequest, MAX_OUTPUT_BYTES, serve_local_read};
use crate::coding_approval::{
    ApprovalError, ApprovalPreview, Capability, Choice, CodingApprovalState, CodingSessionView,
};
use crate::host_effects::{
    HostTerminal, decide_command, decide_write, propose_command, propose_write, refusal_response,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const DECIDER: &str = "person at the terminal";
const DANGER_NOT_ASSESSED: &str = "not assessed";
const MAX_TERMINALS: usize = 16;

pub struct HostRouter {
    view: CodingSessionView,
    state: CodingApprovalState,
    terminals: Vec<HostTerminal>,
}

impl HostRouter {
    /// Open the router on a session whose scope the caller admitted.
    pub fn new(session_id: &str, root: &str, capabilities: BTreeSet<Capability>) -> Self {
        let mut view =
            CodingSessionView::new(session_id.to_owned(), root.to_owned(), capabilities.clone());
        let mut state =
            CodingApprovalState::new(session_id.to_owned(), root.to_owned(), capabilities);
        view.set_connected(true);
        state.set_connected(true);
        Self {
            view,
            state,
            terminals: Vec::new(),
        }
    }

    /// The state the approval view reads: pending request and decision records.
    pub fn state(&self) -> &CodingApprovalState {
        &self.state
    }

    /// Answer one request; `ask` shows the preview and returns the person's choice.
    // contract: TUIG-RUNTIME-R011.8
    pub async fn handle(
        &mut self,
        request: &HostRequest,
        ask: &mut impl AsyncFnMut(ApprovalPreview) -> Choice,
    ) -> Value {
        match self.serve(request, ask).await {
            Ok(result) => json!({"jsonrpc": "2.0", "id": request.id, "result": result}),
            Err(error) => refusal_response(&request.id, &error),
        }
    }

    async fn serve(
        &mut self,
        request: &HostRequest,
        ask: &mut impl AsyncFnMut(ApprovalPreview) -> Choice,
    ) -> Result<Value, ApprovalError> {
        let capability = Capability::parse(&request.method)?;
        if !self.view.allows(&request.method) {
            return Err(ApprovalError::CapabilityNotGranted);
        }
        let id = request.id.to_string();
        let params = &request.params;
        match capability {
            Capability::ReadText => {
                let text = serve_local_read(&self.view, &request.method, text(params, "path")?)?;
                Ok(json!({"content": text}))
            }
            Capability::WriteText => {
                propose_write(
                    &mut self.state,
                    &id,
                    text(params, "path")?,
                    text(params, "content")?,
                )?;
                let choice = self.ask(ask).await?;
                let outcome = decide_write(&mut self.state, choice, DECIDER)?;
                outcome.refusal().map_or(Ok(Value::Null), Err)
            }
            Capability::CreateTerminal => self.create_terminal(&id, params, ask).await,
            Capability::TerminalOutput => Ok(self.terminal(params)?.output().to_json()),
            Capability::WaitForExit => {
                let code = self.terminal(params)?.wait_for_exit().await?;
                Ok(json!({"exitCode": code}))
            }
            Capability::KillTerminal => {
                self.terminal(params)?.kill().await?;
                Ok(Value::Null)
            }
        }
    }

    async fn ask(
        &self,
        ask: &mut impl AsyncFnMut(ApprovalPreview) -> Choice,
    ) -> Result<Choice, ApprovalError> {
        let preview = self.state.preview().ok_or(ApprovalError::RequestChanged)?;
        Ok(ask(preview).await)
    }

    async fn create_terminal(
        &mut self,
        id: &str,
        params: &Value,
        ask: &mut impl AsyncFnMut(ApprovalPreview) -> Choice,
    ) -> Result<Value, ApprovalError> {
        if self.terminals.len() >= MAX_TERMINALS {
            return Err(ApprovalError::PendingRequestExists);
        }
        let command = command_line(params)?;
        let directory = params["cwd"].as_str().unwrap_or(".");
        propose_command(
            &mut self.state,
            id,
            &command,
            directory,
            DANGER_NOT_ASSESSED,
        )?;
        let choice = self.ask(ask).await?;
        let (outcome, terminal) =
            decide_command(&mut self.state, choice, DECIDER, MAX_OUTPUT_BYTES).await?;
        if let Some(error) = outcome.refusal() {
            return Err(error);
        }
        self.terminals
            .push(terminal.ok_or(ApprovalError::InvalidMetadata)?);
        Ok(json!({"terminalId": format!("term-{}", self.terminals.len() - 1)}))
    }

    fn terminal(&mut self, params: &Value) -> Result<&mut HostTerminal, ApprovalError> {
        let index = text(params, "terminalId")?
            .strip_prefix("term-")
            .and_then(|suffix| suffix.parse::<usize>().ok())
            .ok_or(ApprovalError::InvalidMetadata)?;
        self.terminals
            .get_mut(index)
            .ok_or(ApprovalError::InvalidMetadata)
    }
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str, ApprovalError> {
    params[key].as_str().ok_or(ApprovalError::InvalidMetadata)
}

/// The agent names a program and arguments; the person approves the exact shell line.
fn command_line(params: &Value) -> Result<String, ApprovalError> {
    let mut line = quoted(text(params, "command")?);
    for argument in params["args"].as_array().into_iter().flatten() {
        line.push(' ');
        line.push_str(&quoted(
            argument.as_str().ok_or(ApprovalError::InvalidMetadata)?,
        ));
    }
    Ok(line)
}

fn quoted(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./=:@%+,".contains(c))
    {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}
