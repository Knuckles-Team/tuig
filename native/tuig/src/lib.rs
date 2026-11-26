pub mod acp;
pub mod clock;
pub mod coding_approval;
pub mod correlation;
pub mod execution_graph;
pub mod host_effects;
pub mod host_logs;
pub mod host_router;
pub mod app {
    pub use crate::host_logs::{HostLaunchReceipt, HostLogReader, run_host_terminal};
}
pub mod model_choice;
pub mod one_shot;
pub mod operation_command;
pub mod plan_confirm;
pub mod public_run_events;
pub mod recovery;
pub mod run_inspector;
pub mod run_inspector_client;
pub mod served_command;
pub mod task_history;
pub mod task_output;
pub mod task_stream;
pub mod terminal_event;
pub mod turn_usage;
pub mod workstream;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;

use correlation::{TaskCorrelation, TaskEvent, TaskState};
use reqwest::{Client, Url, header};
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const SUBMISSION_UNKNOWN: &str = "Submission outcome unknown. It will not be resubmitted. No task identifier is available to refresh.";

pub const CONTINUATION_EXTENSION: &str =
    "https://knuckles-team.github.io/arbora/extensions/completed-text-history/v1";
pub const CONTINUATION_UNAVAILABLE: &str =
    "Durable context continuation is unavailable; no turn was submitted";

pub fn validate_context_id(value: &str) -> Result<(), &'static str> {
    let suffix = value
        .strip_prefix("a2a-context-")
        .ok_or("Invalid continuation context identifier")?;
    if suffix.len() != 64
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Invalid continuation context identifier");
    }
    Ok(())
}

pub struct TaskSubmission {
    prompt: String,
    message_id: String,
    context_id: Option<String>,
    tool_task: Option<one_shot::OneShotRequest>,
    model_profile: Option<String>,
}

impl TaskSubmission {
    pub fn new(prompt: &str) -> Result<Self, &'static str> {
        if prompt.trim().is_empty() || prompt.len() > 65_536 {
            return Err("Prompt must contain text and be at most 65536 bytes");
        }
        let mut identity = [0_u8; 16];
        getrandom::getrandom(&mut identity).map_err(|_| "Cannot generate submission identity")?;
        let message_id = identity.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self {
            prompt: prompt.to_owned(),
            message_id,
            context_id: None,
            tool_task: None,
            model_profile: None,
        })
    }

    /// One governed tool task; the request's idempotency key is the message identity.
    pub fn tool_task(request: &one_shot::OneShotRequest) -> Self {
        Self {
            prompt: request.operation().to_owned(),
            message_id: request.idempotency_key().to_owned(),
            context_id: None,
            tool_task: Some(request.clone()),
            model_profile: None,
        }
    }

    fn extension(&self) -> Option<&'static str> {
        match (&self.context_id, &self.tool_task) {
            (Some(_), _) => Some(CONTINUATION_EXTENSION),
            (None, Some(_)) => Some(one_shot::TOOL_TASK_EXTENSION),
            (None, None) => None,
        }
    }

    pub fn continuing(prompt: &str, context_id: &str) -> Result<Self, &'static str> {
        validate_context_id(context_id)?;
        let mut submission = Self::new(prompt)?;
        submission.context_id = Some(context_id.to_owned());
        Ok(submission)
    }

    /// Send the caller's model preference; Arbora decides which profile runs.
    pub fn with_model_profile(mut self, profile: Option<String>) -> Self {
        self.model_profile = profile;
        self
    }

    pub fn message_id(&self) -> &str {
        &self.message_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub correlation: TaskCorrelation,
    pub state: TaskState,
    pub terminal_event: terminal_event::TerminalEvent,
    pub output: task_output::TaskOutput,
    pub usage: turn_usage::TurnUsage,
    /// The plan Arbora issued for this task, when the task carries one.
    pub plan: Option<plan_confirm::PlanBinding>,
    pub tool_task: one_shot::ToolTaskMetadata,
    /// The profile the task metadata names, which may differ from the preference.
    pub model_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMode {
    #[default]
    Bearer,
    None,
}

impl AuthMode {
    pub fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value {
            None | Some("bearer") => Ok(Self::Bearer),
            Some("none") => Ok(Self::None),
            _ => Err("AGENT_AUTH_MODE must be bearer or none"),
        }
    }
}

pub struct ArboraClient {
    http: Client,
    card_url: Url,
    authenticated: bool,
    auth_mode: AuthMode,
    submission_timeout: Duration,
    committed_event_reads: bool,
    clock: clock::Clock,
}

impl ArboraClient {
    pub fn new(endpoint: &str, token: Option<&str>) -> Result<Self, &'static str> {
        Self::with_auth_mode(endpoint, token, AuthMode::Bearer)
    }

    pub fn with_auth_mode(
        endpoint: &str,
        token: Option<&str>,
        auth_mode: AuthMode,
    ) -> Result<Self, &'static str> {
        if auth_mode == AuthMode::None && token.is_some() {
            return Err("Anonymous mode cannot use a caller credential");
        }
        let mut url = Self::validate_endpoint(endpoint)?;
        Self::auth_policy(&url, token, auth_mode)?;
        let headers = Self::credential_headers(token)?;
        url.set_path("/.well-known/agent-card.json");
        let http = Self::transport(headers, auth_mode)?;
        Ok(Self {
            http,
            card_url: url,
            authenticated: token.is_some(),
            auth_mode,
            submission_timeout: Duration::from_secs(300),
            committed_event_reads: false,
            clock: clock::Clock::system(),
        })
    }

    /// Replace the time source that expiry checks read.
    pub fn with_clock(mut self, clock: clock::Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn clock(&self) -> &clock::Clock {
        &self.clock
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    fn validate_endpoint(endpoint: &str) -> Result<Url, &'static str> {
        if endpoint
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        {
            return Err("Invalid Arbora endpoint");
        }
        let url = Url::parse(endpoint).map_err(|_| "Invalid Arbora endpoint")?;
        if !Self::endpoint_shape_ok(&url) {
            return Err("Invalid Arbora endpoint");
        }
        Ok(url)
    }

    fn endpoint_shape_ok(url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/"
            && url.port() != Some(0)
    }

    fn auth_policy(
        url: &Url,
        token: Option<&str>,
        auth_mode: AuthMode,
    ) -> Result<(), &'static str> {
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if auth_mode == AuthMode::None && !local {
            return Err("Anonymous Arbora requires a loopback endpoint");
        }
        if !local && url.scheme() != "https" {
            return Err("Remote Arbora requires TLS");
        }
        if !local && token.is_none() {
            return Err("Remote Arbora requires a caller credential");
        }
        Ok(())
    }

    fn credential_headers(token: Option<&str>) -> Result<header::HeaderMap, &'static str> {
        let mut headers = header::HeaderMap::new();
        if let Some(token) = token {
            if token.is_empty() || !token.bytes().all(|c| (33..=126).contains(&c)) {
                return Err("Invalid caller credential");
            }
            let mut value = header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "Invalid caller credential")?;
            value.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, value);
        }
        Ok(headers)
    }

    fn transport(headers: header::HeaderMap, auth_mode: AuthMode) -> Result<Client, &'static str> {
        let mut builder = Client::builder();
        if auth_mode == AuthMode::None {
            builder = builder
                .no_proxy()
                .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
        }
        builder
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "Cannot initialize Arbora transport")
    }

    pub fn with_submission_timeout(mut self, timeout: Duration) -> Result<Self, &'static str> {
        if !(1..=900).contains(&timeout.as_secs()) || timeout.subsec_nanos() != 0 {
            return Err("Submission timeout must be whole seconds between 1 and 900");
        }
        self.submission_timeout = timeout;
        Ok(self)
    }

    pub fn endpoint(&self) -> String {
        let mut endpoint = self.card_url.clone();
        endpoint.set_path("/");
        endpoint.to_string()
    }

    pub fn with_committed_event_reads(mut self, enabled: bool) -> Self {
        self.committed_event_reads = enabled;
        self
    }
}

impl ArboraClient {
    // contract: TUIG-RUNTIME-R002.1.3
    pub async fn submit_task(
        &self,
        submission: &TaskSubmission,
    ) -> Result<TaskSnapshot, &'static str> {
        if !self.authenticated && self.auth_mode != AuthMode::None {
            return Err("Submission requires a caller credential");
        }
        self.require_submission_extension(submission).await?;
        let response = self.dispatch_submission(submission).await?;
        Self::activation_ok(&response, submission)?;
        let bytes = read_task_response(response)
            .await
            .map_err(|_| SUBMISSION_UNKNOWN)?;
        let task =
            parse_returned_task(submission.message_id(), &bytes).map_err(|_| SUBMISSION_UNKNOWN)?;
        if submission
            .context_id
            .as_deref()
            .is_some_and(|context_id| task.correlation.context_id() != context_id)
        {
            return Err(SUBMISSION_UNKNOWN);
        }
        Ok(task)
    }

    fn submission_message(submission: &TaskSubmission) -> serde_json::Value {
        let part = match &submission.tool_task {
            Some(request) => request.message_part(),
            None => serde_json::json!({"kind": "text", "text": submission.prompt}),
        };
        let mut message = serde_json::json!({
            "kind": "message", "role": "user",
            "messageId": submission.message_id(),
            "parts": [part],
        });
        if let Some(context_id) = &submission.context_id {
            message["contextId"] = serde_json::json!(context_id);
        }
        if let Some(profile) = &submission.model_profile {
            message["metadata"] = serde_json::json!({ model_choice::PREFERENCE_KEY: profile });
        }
        message
    }

    async fn dispatch_submission(
        &self,
        submission: &TaskSubmission,
    ) -> Result<reqwest::Response, &'static str> {
        let message = Self::submission_message(submission);
        let mut url = self.card_url.clone();
        url.set_path("/a2a");
        let mut request = self
            .http
            .post(url)
            .timeout(self.submission_timeout)
            .header("Idempotency-Key", submission.message_id());
        if let Some(extension) = submission.extension() {
            request = request.header("X-A2A-Extensions", extension);
        }
        request
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": submission.message_id(),
                "method": "message/send",
                "params": {"configuration": {"blocking": false}, "message": message},
            }))
            .send()
            .await
            .map_err(|_| SUBMISSION_UNKNOWN)
    }

    fn activation_ok(
        response: &reqwest::Response,
        submission: &TaskSubmission,
    ) -> Result<(), &'static str> {
        if !response.status().is_success() {
            return Err(SUBMISSION_UNKNOWN);
        }
        if let Some(extension) = submission.extension() {
            let mut activations = response.headers().get_all("X-A2A-Extensions").iter();
            if activations.next().and_then(|value| value.to_str().ok()) != Some(extension)
                || activations.next().is_some()
            {
                return Err(SUBMISSION_UNKNOWN);
            }
        }
        Ok(())
    }

    async fn require_submission_extension(
        &self,
        submission: &TaskSubmission,
    ) -> Result<(), &'static str> {
        let unavailable = if submission.tool_task.is_some() {
            one_shot::TOOL_TASK_UNAVAILABLE
        } else {
            CONTINUATION_UNAVAILABLE
        };
        match submission.extension() {
            Some(extension) => self.require_extension(extension, unavailable).await,
            None => Ok(()),
        }
    }

    async fn require_extension(
        &self,
        uri: &str,
        unavailable: &'static str,
    ) -> Result<(), &'static str> {
        let response = self
            .http
            .get(self.card_url.clone())
            .send()
            .await
            .map_err(|_| unavailable)?;
        if !response.status().is_success() {
            return Err(unavailable);
        }
        let bytes = read_task_response(response)
            .await
            .map_err(|_| unavailable)?;
        let card: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| unavailable)?;
        let extensions = card
            .pointer("/capabilities/extensions")
            .and_then(|value| value.as_array())
            .ok_or(unavailable)?;
        let mut matching = extensions
            .iter()
            .filter(|extension| extension.get("uri").and_then(|value| value.as_str()) == Some(uri));
        let extension = matching.next().ok_or(unavailable)?;
        if matching.next().is_some()
            || extension.get("required").and_then(|value| value.as_bool()) != Some(false)
        {
            return Err(unavailable);
        }
        Ok(())
    }

    pub async fn get_task(&self, task_id: &str) -> Result<TaskSnapshot, &'static str> {
        self.read_task(task_id, None)
            .await
            .map_err(|error| error.message())
    }

    pub async fn refresh_task(
        &self,
        binding: &TaskCorrelation,
    ) -> Result<TaskSnapshot, &'static str> {
        self.read_task(binding.task_id(), Some(binding))
            .await
            .map_err(|error| error.message())
    }

    // contract: TUIG-RUNTIME-R002.1.5
    pub async fn cancel_task(
        &self,
        binding: &TaskCorrelation,
    ) -> Result<TaskSnapshot, &'static str> {
        if binding.task_id().chars().count() > 80 {
            return Err("Invalid task identifier");
        }
        let mut url = self.card_url.clone();
        url.set_path("/a2a");
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": "task-cancel",
                "method": "tasks/cancel",
                "params": {"id": binding.task_id()},
            }))
            .send()
            .await
            .map_err(|_| "Arbora cancellation outcome unknown")?;
        if !response.status().is_success() {
            return Err("Arbora cancellation refused or unavailable");
        }
        let bytes = read_task_response(response).await?;
        let task = parse_task_snapshot(binding.task_id(), "task-cancel", &bytes)?;
        binding.validate_binding(&task.correlation)?;
        if task.state != TaskState::Canceled {
            return Err("Arbora cancellation not acknowledged");
        }
        Ok(TaskSnapshot {
            correlation: binding.clone(),
            state: task.state,
            terminal_event: terminal_event::TerminalEvent::default(),
            output: task_output::TaskOutput::default(),
            usage: turn_usage::TurnUsage::default(),
            plan: None,
            tool_task: one_shot::ToolTaskMetadata::default(),
            model_profile: None,
        })
    }

    // contract: TUIG-RUNTIME-R002.7
    pub async fn discover(&self) -> Result<String, &'static str> {
        let mut response = self
            .http
            .get(self.card_url.clone())
            .send()
            .await
            .map_err(|_| "Arbora discovery failed or timed out")?;
        if !response.status().is_success() {
            return Err("Arbora discovery refused or unavailable");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Arbora response interrupted")?
        {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err("Arbora Agent Card exceeds size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let card: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Arbora Agent Card")?;
        let name = card
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or("Arbora Agent Card has no name")?;
        Ok(name.chars().filter(|c| !c.is_control()).take(80).collect())
    }
}

async fn read_task_response(mut response: reqwest::Response) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Arbora response interrupted")?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err("Arbora task snapshot exceeds size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_task_snapshot(
    task_id: &str,
    request_id: &str,
    bytes: &[u8],
) -> Result<TaskSnapshot, &'static str> {
    let snapshot = parse_returned_task(request_id, bytes)?;
    if snapshot.correlation.task_id() != task_id {
        return Err("Task correlation mismatch");
    }
    Ok(snapshot)
}

fn envelope_result<'a>(
    envelope: &'a serde_json::Value,
    request_id: &str,
) -> Result<&'a serde_json::Value, &'static str> {
    if envelope.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0")
        || envelope.get("id").and_then(|v| v.as_str()) != Some(request_id)
        || envelope.get("error").is_some()
    {
        return Err("Invalid or refused Arbora task response");
    }
    envelope.get("result").ok_or("Missing Arbora task snapshot")
}

fn parse_returned_task(request_id: &str, bytes: &[u8]) -> Result<TaskSnapshot, &'static str> {
    let envelope: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Invalid Arbora task response")?;
    let task = envelope_result(&envelope, request_id)?;
    let correlation = TaskCorrelation::from_task(task)?;
    let TaskEvent::Status { state, .. } = correlation.validate(task)? else {
        return Err("Invalid Arbora task snapshot");
    };
    let terminal_event = if request_id == "task-snapshot" {
        terminal_event::TerminalEvent::from_task(task, &correlation, state)?
    } else {
        terminal_event::TerminalEvent::default()
    };
    let output = task_output::TaskOutput::from_task(task, state)?;
    let usage = turn_usage::TurnUsage::from_task(task, &correlation);
    Ok(TaskSnapshot {
        usage,
        output,
        correlation,
        state,
        terminal_event,
        plan: plan_confirm::PlanBinding::from_task(task).ok(),
        tool_task: one_shot::ToolTaskMetadata::from_task(task),
        model_profile: model_choice::displayed_profile(task),
    })
}
