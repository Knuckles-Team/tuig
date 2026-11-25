use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream, MouseEvent,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Text},
    widgets::{Block, Paragraph, Wrap},
};
use std::{env, ffi::OsString, process::ExitCode, sync::Arc};
use tuig_native::{
    ArboraClient, AuthMode, SUBMISSION_UNKNOWN, TaskSnapshot, TaskSubmission,
    correlation::{TaskCorrelation, TaskState},
};

mod cli;
mod composer;
mod headless;
mod host_log_view;
use host_log_view::HostLogView;
mod inspector_view;
use inspector_view::InspectorView;
mod local_runtime;
#[cfg(unix)]
mod local_session;
mod presentation;
mod public_details_view;
mod session;
mod spatial_graph;
mod task_history_view;
use presentation::Viewport;
#[cfg(test)]
use presentation::presentation_style;
use public_details_view::PublicDetailsView;
use task_history_view::TaskHistoryView;

#[cfg(test)]
fn draw(frame: &mut Frame, status: &str, style: Style) {
    draw_viewport(frame, status, style, &mut Viewport::default(), false);
}

#[cfg(test)]
fn draw_viewport(
    frame: &mut Frame,
    status: &str,
    style: Style,
    viewport: &mut Viewport,
    read_only: bool,
) {
    let cancel_help = if read_only {
        "read-only observation"
    } else {
        "x: cancel remote task"
    };
    let text = format!(
        "{status}\n\nTask streaming and approvals remain unavailable.\n\nF6: theme | l: host logs | i: run inspector | r: refresh read | {cancel_help} | c: cancel local request | q / Ctrl+C: quit"
    );
    let block = Block::bordered()
        .title("Arbora")
        .title_bottom("Up/Down PgUp/PgDn Home/End");
    let inner = block.inner(frame.area());
    let paragraph = Paragraph::new(text).style(style).wrap(Wrap { trim: false });
    viewport.measure(paragraph.line_count(inner.width), inner.height);
    frame.render_widget(
        paragraph.scroll((viewport.offset(), 0)).block(block),
        frame.area(),
    );
}

fn draw_task_viewport(
    frame: &mut Frame,
    status: &str,
    style: Style,
    border_style: Style,
    viewport: &mut Viewport,
    read_only: bool,
    details: &mut PublicDetailsView,
) {
    let (lines, headers) = task_viewport_lines(status, style, read_only, details);
    let block = Block::bordered()
        .border_style(border_style)
        .title_style(border_style)
        .title("Arbora")
        .title_bottom("Up/Down PgUp/PgDn Home/End");
    let inner = block.inner(frame.area());
    let paragraph = Paragraph::new(Text::from(lines.clone()))
        .style(style)
        .wrap(Wrap { trim: false });
    viewport.measure(paragraph.line_count(inner.width), inner.height);
    let positions = header_positions(&lines, &headers, inner.width);
    if let Some(index) = details.take_focus_request()
        && let Some((start, height)) = positions[index]
    {
        viewport.reveal(start, height);
    }
    details.set_header_regions(header_regions(&positions, viewport, inner));
    frame.render_widget(
        paragraph.scroll((viewport.offset(), 0)).block(block),
        frame.area(),
    );
}

fn task_viewport_lines(
    status: &str,
    style: Style,
    read_only: bool,
    details: &mut PublicDetailsView,
) -> (Vec<Line<'static>>, [Option<usize>; 2]) {
    let mut lines: Vec<Line<'static>> = status
        .lines()
        .map(|line| Line::from(line.to_owned()))
        .collect();
    lines.push(Line::from(""));
    let headers = details.append_lines(&mut lines, style);
    let cancel_help = if read_only {
        "read-only observation"
    } else {
        "x: cancel remote task"
    };
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Tab: select detail | Enter / Space / click: expand or collapse | Escape: leave detail",
    ));
    lines.push(Line::from(
        "Task streaming and approvals remain unavailable.",
    ));
    lines.push(Line::from(format!(
        "F6: theme | l: host logs | i: run inspector | r: refresh read | {cancel_help} | c: cancel local request | q / Ctrl+C: quit"
    )));
    (lines, headers)
}

fn header_positions(
    lines: &[Line<'static>],
    headers: &[Option<usize>; 2],
    width: u16,
) -> [Option<(usize, usize)>; 2] {
    headers.map(|header| {
        header.map(|header| {
            let start = if header == 0 {
                0
            } else {
                Paragraph::new(Text::from(lines[..header].to_vec()))
                    .wrap(Wrap { trim: false })
                    .line_count(width)
            };
            let height = Paragraph::new(Text::from(lines[header..=header].to_vec()))
                .wrap(Wrap { trim: false })
                .line_count(width);
            (start, height)
        })
    })
}

fn header_regions(
    positions: &[Option<(usize, usize)>; 2],
    viewport: &Viewport,
    inner: Rect,
) -> [Option<Rect>; 2] {
    let mut regions = [None; 2];
    for (index, position) in positions.iter().enumerate() {
        if let Some((start, height)) = *position {
            let offset = usize::from(viewport.offset());
            let visible_start = start.max(offset);
            let visible_end = (start + height).min(offset + usize::from(inner.height));
            if visible_start < visible_end && inner.width > 0 {
                regions[index] = Some(Rect::new(
                    inner.x,
                    inner.y + (visible_start - offset) as u16,
                    inner.width,
                    (visible_end - visible_start) as u16,
                ));
            }
        }
    }
    regions
}

fn route_mouse(
    event: MouseEvent,
    confirming: bool,
    logs: &mut HostLogView,
    inspector: &mut InspectorView,
    viewport: &mut Viewport,
    details: &mut PublicDetailsView,
    history: &mut TaskHistoryView,
) -> bool {
    if confirming {
        return false;
    }
    if logs.is_open() {
        return logs.mouse(event);
    }
    if inspector.is_open() {
        return inspector.mouse(event);
    }
    if history.is_open() {
        return history.mouse(event);
    }
    viewport.mouse(event) || details.mouse(event)
}

fn draw_confirmation(frame: &mut Frame, status: &str, style: Style, border_style: Style) -> bool {
    let fits = frame.area().width >= 34 && frame.area().height >= 5;
    let text = if fits {
        format!("Cancel selected remote task?\ny: confirm | n / Escape: dismiss\n\n{status}")
    } else {
        "Resize to confirm. n / Escape: dismiss".to_owned()
    };
    frame.render_widget(
        Paragraph::new(text)
            .style(style)
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .border_style(border_style)
                    .title_style(border_style)
                    .title("Confirm cancellation"),
            ),
        frame.area(),
    );
    fits
}

fn confirm_visible(
    cancellation: &mut Cancellation,
    visible: bool,
    size: std::io::Result<(u16, u16)>,
) -> bool {
    visible
        && matches!(size, Ok((width, height)) if width >= 34 && height >= 5)
        && cancellation.confirm()
}

fn selected_task(value: Result<String, env::VarError>) -> Result<Option<String>, &'static str> {
    match value {
        Ok(task)
            if task.trim().is_empty() || task.len() > 512 || task.chars().any(char::is_control) =>
        {
            Err("AGENT_TASK_ID must be a non-empty task identifier")
        }
        Ok(task) => Ok(Some(task)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err("AGENT_TASK_ID must be valid Unicode"),
    }
}

fn submission_timeout(
    value: Result<String, env::VarError>,
) -> Result<std::time::Duration, &'static str> {
    let seconds = match value {
        Err(env::VarError::NotPresent) => 300,
        Ok(value) if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => value
            .parse::<u64>()
            .map_err(|_| "AGENT_SUBMISSION_TIMEOUT_SECONDS must be 1..900")?,
        _ => return Err("AGENT_SUBMISSION_TIMEOUT_SECONDS must be 1..900"),
    };
    if !(1..=900).contains(&seconds) {
        return Err("AGENT_SUBMISSION_TIMEOUT_SECONDS must be 1..900");
    }
    Ok(std::time::Duration::from_secs(seconds))
}

fn committed_event_reads(value: Result<String, env::VarError>) -> Result<bool, &'static str> {
    match value {
        Err(env::VarError::NotPresent) => Ok(false),
        Ok(value) if value == "true" => Ok(true),
        Ok(value) if value == "false" => Ok(false),
        _ => Err("AGENT_INCLUDE_COMMITTED_EVENT must be true or false"),
    }
}

fn selected_prompt(args: &[OsString], task_selected: bool) -> Result<Option<String>, &'static str> {
    if args.is_empty() {
        return Ok(None);
    }
    let [flag, prompt] = args else {
        return Err("Expected exactly --prompt TEXT, or no arguments");
    };
    if flag != "--prompt" {
        return Err("Unknown argument; expected --prompt TEXT");
    }
    if task_selected {
        return Err("--prompt cannot be combined with AGENT_TASK_ID");
    }
    let prompt = prompt.to_str().ok_or("Prompt must be valid Unicode")?;
    if prompt.trim().is_empty() || prompt.len() > 65_536 {
        return Err("Prompt must contain text and be at most 65536 bytes");
    }
    Ok(Some(prompt.to_owned()))
}

fn selected_observation(
    args: &[OsString],
    task_selected: bool,
) -> Result<Option<String>, &'static str> {
    if args.first().is_none_or(|flag| flag != "--observe-task") {
        return Ok(None);
    }
    let [_, task] = args else {
        return Err("Expected exactly --observe-task TASK_ID");
    };
    if task_selected {
        return Err("--observe-task cannot be combined with AGENT_TASK_ID");
    }
    let task = task.to_str().ok_or("Task ID must be valid Unicode")?;
    selected_task(Ok(task.to_owned()))
}

fn observation_ready(active: bool, pending: bool, cancellation: Cancellation) -> bool {
    active && !pending && cancellation != Cancellation::Confirming
}

fn stop_expired_observation(
    deadline: Option<tokio::time::Instant>,
    now: tokio::time::Instant,
    active: &mut bool,
    status: &mut String,
) -> bool {
    if !observation_expired(deadline, now) {
        return false;
    }
    *active = false;
    status.push_str(
        "\n\nAutomatic observation timed out. Remote execution may continue. Press r to refresh.",
    );
    true
}

fn observation_expired(deadline: Option<tokio::time::Instant>, now: tokio::time::Instant) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

fn observation_continues(snapshot: &TaskSnapshot, binding: Option<&TaskCorrelation>) -> bool {
    matches!(snapshot.state, TaskState::Submitted | TaskState::Working)
        && binding.is_none_or(|initial| {
            initial.validate_binding(&snapshot.correlation).is_ok()
                && snapshot.terminal_event.validate_binding(initial).is_ok()
        })
}

fn snapshot_status(snapshot: &TaskSnapshot) -> String {
    let state = match snapshot.state {
        TaskState::Submitted => "submitted",
        TaskState::Working => "working",
        TaskState::InputRequired => "input-required",
        TaskState::AuthRequired => "auth-required",
        TaskState::Completed => "completed",
        TaskState::Canceled => "canceled",
        TaskState::Failed => "failed",
        TaskState::Rejected => "rejected",
    };
    let approval = if snapshot.state == TaskState::InputRequired {
        "Approval: pending; not granted locally.\n"
    } else {
        ""
    };
    format!(
        "Task ID: {}\nContext ID: {}\nRun ID: {}\nState: {state}\n{approval}\n{}\n\n{}\n\n{}",
        snapshot.correlation.task_id(),
        snapshot.correlation.context_id(),
        snapshot.correlation.run_id().unwrap_or("unavailable"),
        snapshot.terminal_event.text(),
        snapshot.output.text(),
        snapshot.usage.text(),
    )
}

// The Task variant deliberately carries the whole snapshot: the observation
// loop hands it to the compositor every 500 ms, and boxing it would add a
// heap allocation to that hot read path for no observable gain.
#[allow(clippy::large_enum_variant)]
enum ReadStatus {
    Discovery(String),
    Task(TaskSnapshot),
    Operation(String),
}

async fn read_status(
    client: &ArboraClient,
    task_id: Option<&str>,
    binding: Option<&TaskCorrelation>,
) -> Result<ReadStatus, &'static str> {
    if let Some(binding) = binding {
        return client.refresh_task(binding).await.map(ReadStatus::Task);
    }
    match task_id {
        Some(task_id) => client.get_task(task_id).await.map(ReadStatus::Task),
        None => client.discover().await.map(ReadStatus::Discovery),
    }
}

fn completed_status(result: session::ReadResult, binding: &mut Option<TaskCorrelation>) -> String {
    match result {
        Ok(Ok(ReadStatus::Discovery(name))) => format!("Agent Card received: {name}"),
        Ok(Ok(ReadStatus::Task(mut snapshot))) => {
            if let Err(error) = settle_task_binding(&mut snapshot, binding) {
                return error.to_owned();
            }
            snapshot_status(&snapshot)
        }
        Ok(Ok(ReadStatus::Operation(text))) => text,
        Ok(Err(error)) => error.to_owned(),
        Err(error) if error.is_cancelled() => {
            "Local read canceled. No remote task was canceled.".to_owned()
        }
        Err(_) => "Read request interrupted".to_owned(),
    }
}

/// Adopt or validate the task binding for a completed read, then settle the
/// snapshot's correlation with it.
fn settle_task_binding(
    snapshot: &mut TaskSnapshot,
    binding: &mut Option<TaskCorrelation>,
) -> Result<(), &'static str> {
    if let Some(initial) = binding {
        initial.validate_binding(&snapshot.correlation)?;
        snapshot.terminal_event.validate_binding(initial)?;
        snapshot.correlation = initial.clone();
    } else {
        *binding = Some(snapshot.correlation.clone());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Cancellation {
    #[default]
    Unavailable,
    Ready,
    Confirming,
    Pending,
    Unknown,
}

impl Cancellation {
    fn arm(&mut self) {
        if *self == Self::Ready {
            *self = Self::Confirming;
        }
    }

    fn confirm(&mut self) -> bool {
        if *self != Self::Confirming {
            return false;
        }
        *self = Self::Pending;
        true
    }

    fn dismiss(&mut self) {
        if *self == Self::Confirming {
            *self = Self::Ready;
        }
    }

    fn read_started(&mut self) {
        if *self != Self::Unknown {
            *self = Self::Unavailable;
        }
    }
}

const CANCELLATION_UNKNOWN: &str =
    "Cancellation outcome unknown. Press r to read task state before trying again.";

const OPERATION_REFUSED: &str = "Operation refused: not advertised in the Arbora registry, the registry read failed, or the response envelope was refused.";
const PLAN_REFUSED: &str = "Plan confirmation refused: the plan is missing, expired or tampered, or Arbora refused the confirmation. No effect was applied.";
const PLAN_UNAVAILABLE: &str = "No issued plan to confirm; no effect was requested.";

fn request_completed(
    result: session::ReadResult,
    binding: &mut Option<TaskCorrelation>,
    cancellation: &mut Cancellation,
    remote_cancel: bool,
) -> String {
    let accepted_state = match &result {
        Ok(Ok(ReadStatus::Task(snapshot)))
            if binding
                .as_ref()
                .is_none_or(|initial| initial.validate_binding(&snapshot.correlation).is_ok()) =>
        {
            Some(snapshot.state)
        }
        _ => None,
    };
    if remote_cancel {
        if binding.is_none() || accepted_state != Some(TaskState::Canceled) {
            *cancellation = Cancellation::Unknown;
            return CANCELLATION_UNKNOWN.to_owned();
        }
    } else if *cancellation == Cancellation::Unknown && accepted_state.is_none() {
        return CANCELLATION_UNKNOWN.to_owned();
    }
    *cancellation = match accepted_state {
        Some(
            TaskState::Submitted
            | TaskState::Working
            | TaskState::InputRequired
            | TaskState::AuthRequired,
        ) => Cancellation::Ready,
        _ => Cancellation::Unavailable,
    };
    completed_status(result, binding)
}

fn submission_completed(
    result: session::ReadResult,
    binding: &mut Option<TaskCorrelation>,
    cancellation: &mut Cancellation,
) -> String {
    match result {
        Ok(Ok(ReadStatus::Task(snapshot))) => request_completed(
            Ok(Ok(ReadStatus::Task(snapshot))),
            binding,
            cancellation,
            false,
        ),
        _ => {
            *cancellation = Cancellation::Unavailable;
            SUBMISSION_UNKNOWN.to_owned()
        }
    }
}

fn enable_terminal_input(writer: &mut impl std::io::Write) -> std::io::Result<()> {
    let result = crossterm::execute!(writer, EnableMouseCapture, EnableBracketedPaste);
    if result.is_err() {
        let _ = crossterm::execute!(writer, DisableBracketedPaste, DisableMouseCapture);
    }
    result
}

async fn run(
    client: Arc<ArboraClient>,
    task_id: Option<String>,
    submission: Option<TaskSubmission>,
    observe: bool,
) -> Result<(), &'static str> {
    let mut terminal = ratatui::init();
    if enable_terminal_input(&mut std::io::stdout()).is_err() {
        ratatui::restore();
        return Err("Cannot enable terminal input");
    }
    let (mut session, mut request) =
        session::Session::new(client.clone(), task_id, submission, observe);
    let mut events = EventStream::new();
    let mut observation_tick = tokio::time::interval(std::time::Duration::from_millis(500));
    observation_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut redraw = tokio::time::interval(std::time::Duration::from_millis(250));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = session
        .event_loop(
            &client,
            &mut terminal,
            &mut request,
            &mut events,
            &mut observation_tick,
            &mut redraw,
        )
        .await;
    session.finish(&mut request).await;
    result
}

const CLI_HELP: &str = "Tuig: native Arbora terminal

Usage: tuig [--headless] [--prompt TEXT | --observe-task TASK_ID]
       tuig --local-runtime --local-model PROVIDER:MODEL [--prompt TEXT]
       tuig --local-runtime --local-model PROVIDER:MODEL --headless --prompt TEXT
       tuig --help

Options:
  --headless              Print unary snapshots without opening a terminal.
                          Requires --prompt or --observe-task.
  --prompt TEXT           Submit one text task (1..65536 bytes).
  --continue CONTEXT_ID   Continue an advertised durable context with --prompt.
  --observe-task TASK_ID   Observe an existing task without remote cancellation.
  --local-runtime         Spawn internal arbora-acp (POSIX, 16 KiB prompt).
  --local-model VALUE     Provider:model for explicit local runtime only.
  -h, --help              Print this help without connecting or opening a terminal.

With no arguments, compose one task or read AGENT_TASK_ID.
In the composer: Enter submits, Ctrl+J adds a newline, Escape reviews status.
Before submission, p returns to the draft. Each invocation sends at most one task.
--prompt and --observe-task cannot be combined with each other or AGENT_TASK_ID.

Environment:
  AGENT_URL                        Endpoint (default: http://localhost:8000).
  AGENT_BEARER_TOKEN               Caller credential; required for remote access.
  AGENT_AUTH_MODE                  bearer (default), or none for local loopback.
  AGENT_TASK_ID                    Existing task to read; does not resume chat.
  AGENT_SUBMISSION_TIMEOUT_SECONDS Submission timeout, 1..900 (default: 300).
  AGENT_INCLUDE_COMMITTED_EVENT    Include committed event reads: true or false.
  AGENT_THEME                      Terminal theme (default: Tokyo Night).
  NO_COLOR                        Nonempty value disables theme colors.

Remote access requires TLS. Anonymous loopback mode rejects bearer credentials.
Central task streaming and approvals remain unavailable.
Headless exits nonzero on input/auth required, failed, canceled or rejected tasks.
Headless observation is bounded to four hours and never cancels backend work.
Local ACP requires a separately installed compatible Arbora/AU backend.
Local ACP interactive sessions allow sequential acknowledged text turns.
Local ACP has no client tools, permission grants or durable resume.
Legacy --override, --bg, --model and --session are unsupported.
";

fn caller_credential() -> Result<(Option<String>, AuthMode), &'static str> {
    let token = match env::var("AGENT_BEARER_TOKEN") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => return Err("Invalid caller credential"),
    };
    let mode = match env::var("AGENT_AUTH_MODE") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => return Err("AGENT_AUTH_MODE must be valid Unicode"),
    };
    Ok((token, AuthMode::parse(mode.as_deref())?))
}

fn require_credential(
    requested: bool,
    authenticated: bool,
    auth_mode: AuthMode,
    message: &'static str,
) -> Result<(), &'static str> {
    if requested && !authenticated && auth_mode != AuthMode::None {
        return Err(message);
    }
    Ok(())
}

fn launch_targets(
    args: &[OsString],
    authenticated: bool,
    auth_mode: AuthMode,
) -> Result<(bool, Option<String>, Option<TaskSubmission>), &'static str> {
    let task_id = selected_task(env::var("AGENT_TASK_ID"))?;
    let (context_id, args) = cli::continuation(args, task_id.is_some())?;
    let observation = selected_observation(&args, task_id.is_some())?;
    let observe = observation.is_some();
    require_credential(
        observe,
        authenticated,
        auth_mode,
        "Observation requires a caller credential",
    )?;
    let prompt = if observe {
        None
    } else {
        selected_prompt(&args, task_id.is_some())?
    };
    let task_id = observation.or(task_id);
    require_credential(
        prompt.is_some(),
        authenticated,
        auth_mode,
        "Submission requires a caller credential",
    )?;
    Ok((observe, task_id, submission_from(prompt, context_id)?))
}

fn submission_from(
    prompt: Option<String>,
    context_id: Option<String>,
) -> Result<Option<TaskSubmission>, &'static str> {
    prompt
        .as_deref()
        .map(|text| match context_id.as_deref() {
            Some(context) => TaskSubmission::continuing(text, context),
            None => TaskSubmission::new(text),
        })
        .transpose()
}

fn remote_client(
    endpoint: &str,
    token: Option<&str>,
    auth_mode: AuthMode,
) -> Result<ArboraClient, &'static str> {
    let timeout = submission_timeout(env::var("AGENT_SUBMISSION_TIMEOUT_SECONDS"))?;
    let include_event = committed_event_reads(env::var("AGENT_INCLUDE_COMMITTED_EVENT"))?;
    Ok(ArboraClient::with_auth_mode(endpoint, token, auth_mode)?
        .with_submission_timeout(timeout)?
        .with_committed_event_reads(include_event))
}

async fn launch(
    client: ArboraClient,
    headless: bool,
    observe: bool,
    task_id: Option<String>,
    submission: Option<TaskSubmission>,
) -> Result<(), &'static str> {
    if headless {
        headless::run(&client, task_id.as_deref(), submission.as_ref()).await
    } else {
        run(Arc::new(client), task_id, submission, observe).await
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if help_requested(&args) {
        print!("{CLI_HELP}");
        return ExitCode::SUCCESS;
    }
    let result = launch_app(args).await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report_error(error);
            ExitCode::FAILURE
        }
    }
}

fn help_requested(args: &[OsString]) -> bool {
    matches!(args, [flag] if flag == "--help" || flag == "-h")
}

async fn launch_app(args: Vec<OsString>) -> Result<(), &'static str> {
    if let Some(local) = local_runtime::options(&args)? {
        return local_runtime::run(local).await;
    }
    let endpoint = env::var("AGENT_URL").unwrap_or_else(|_| "http://localhost:8000".to_owned());
    let (headless, args) = cli::arguments(&args)?;
    let (token, auth_mode) = caller_credential()?;
    let (observe, task_id, submission) = launch_targets(&args, token.is_some(), auth_mode)?;
    let client = remote_client(&endpoint, token.as_deref(), auth_mode)?;
    launch(client, headless, observe, task_id, submission).await
}

fn report_error(error: &str) {
    #[cfg(unix)]
    {
        write_error(std::io::stderr(), error);
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "{error}");
    }
}

#[cfg(unix)]
fn write_error(output: impl std::os::fd::AsFd, error: &str) {
    if let Ok(flags) = rustix::fs::fcntl_getfl(&output)
        && rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK).is_ok()
    {
        let _ = rustix::io::write(&output, format!("{error}\n").as_bytes());
        let _ = rustix::fs::fcntl_setfl(&output, flags);
    }
}

#[cfg(test)]
mod tests {
    // spec: TUIG-RUNTIME-R011.14
    #[cfg(unix)]
    #[test]
    fn error_reporting_does_not_block_and_restores_flags() {
        let (output, _reader) = std::os::unix::net::UnixStream::pair().unwrap();
        let flags = rustix::fs::fcntl_getfl(&output).unwrap();
        output.set_nonblocking(true).unwrap();
        while rustix::io::write(&output, &[0; 4096]).is_ok() {}
        rustix::fs::fcntl_setfl(&output, flags).unwrap();
        super::write_error(&output, "Cannot draw terminal");
        assert_eq!(rustix::fs::fcntl_getfl(&output).unwrap(), flags);
    }

    use super::{
        CANCELLATION_UNKNOWN, Cancellation, ReadStatus, SUBMISSION_UNKNOWN, completed_status, draw,
        presentation_style, request_completed, selected_prompt, selected_task, snapshot_status,
        submission_completed,
    };
    use ratatui::style::{Color, Style};
    use ratatui::{Terminal, backend::TestBackend};
    use std::env::VarError;
    use std::ffi::OsString;
    use tuig_native::{
        TaskSnapshot,
        correlation::{TaskCorrelation, TaskState},
    };

    // spec: TUIG-RUNTIME-R012.1.3
    #[test]
    fn partial_input_setup_disables_paste_and_mouse() {
        #[derive(Default)]
        struct FailPasteOnce {
            bytes: Vec<u8>,
            failed: bool,
        }
        impl std::io::Write for FailPasteOnce {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.failed && bytes.windows(5).any(|part| part == b"2004h") {
                    self.failed = true;
                    return Err(std::io::Error::other("paste setup failed"));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = FailPasteOnce::default();
        assert!(super::enable_terminal_input(&mut writer).is_err());
        let output = String::from_utf8(writer.bytes).unwrap();
        assert!(output.contains("?1000h"));
        assert!(output.contains("?2004l"));
        assert!(output.contains("?1000l"));
    }

    // spec: TUIG-RUNTIME-R002.1.3.1
    #[test]
    fn prompt_arguments_allow_only_one_text_submission() {
        let arguments = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(selected_prompt(&[], false), Ok(None));
        assert_eq!(selected_prompt(&[], true), Ok(None));
        assert_eq!(
            selected_prompt(&arguments(&["--prompt", "ordinary text"]), false),
            Ok(Some("ordinary text".to_owned()))
        );
        for invalid in [
            vec!["--prompt"],
            vec!["ordinary text"],
            vec!["--override", "text"],
            vec!["--prompt", "text", "--prompt", "again"],
            vec!["--prompt", "text", "extra"],
            vec!["--prompt", "  "],
            vec!["--prompt=other"],
        ] {
            assert!(selected_prompt(&arguments(&invalid), false).is_err());
        }
        assert!(selected_prompt(&arguments(&["--prompt", "text"]), true).is_err());
        assert!(selected_prompt(&arguments(&["--prompt", &"x".repeat(65_537)]), false).is_err());
    }

    // spec: TUIG-RUNTIME-R002.1.3.1
    #[test]
    fn accepted_submission_binds_returned_task_and_enables_cancellation() {
        let mut binding = None;
        let mut cancellation = Cancellation::Unavailable;
        let status = submission_completed(
            Ok(Ok(task_read(
                "returned-task",
                "returned-context",
                Some("returned-run"),
            ))),
            &mut binding,
            &mut cancellation,
        );
        assert!(status.contains("Task ID: returned-task"));
        assert_eq!(binding.unwrap().context_id(), "returned-context");
        assert_eq!(cancellation, Cancellation::Ready);
    }

    // spec: TUIG-RUNTIME-R002.1.3.1
    #[tokio::test]
    async fn uncertain_submission_never_supplies_task_authority() {
        let mut binding = None;
        let mut cancellation = Cancellation::Unavailable;
        for result in [
            Ok(Err("protected response detail")),
            Ok(Ok(ReadStatus::Discovery("unexpected".to_owned()))),
        ] {
            assert_eq!(
                submission_completed(result, &mut binding, &mut cancellation),
                SUBMISSION_UNKNOWN
            );
            assert!(binding.is_none());
            assert_eq!(cancellation, Cancellation::Unavailable);
        }
        let request = tokio::spawn(async {
            std::future::pending::<Result<ReadStatus, &'static str>>().await
        });
        request.abort();
        assert_eq!(
            submission_completed(request.await, &mut binding, &mut cancellation),
            SUBMISSION_UNKNOWN
        );
        assert!(binding.is_none());
    }

    // spec: TUIG-RUNTIME-R002.1.3.1
    #[tokio::test]
    async fn abort_after_submission_completes_retains_known_binding() {
        let request = tokio::spawn(async {
            Ok(task_read(
                "accepted-task",
                "accepted-context",
                Some("accepted-run"),
            ))
        });
        while !request.is_finished() {
            tokio::task::yield_now().await;
        }
        request.abort();
        let mut binding = None;
        let mut cancellation = Cancellation::Unavailable;
        let status = submission_completed(request.await, &mut binding, &mut cancellation);
        let binding = binding.unwrap();
        assert_eq!(binding.task_id(), "accepted-task");
        assert_eq!(binding.context_id(), "accepted-context");
        assert_eq!(binding.run_id(), Some("accepted-run"));
        assert!(status.contains("Task ID: accepted-task"));
        assert!(!status.contains("outcome unknown"));
        assert_eq!(cancellation, Cancellation::Ready);
    }

    fn task_read(task_id: &str, context_id: &str, run_id: Option<&str>) -> ReadStatus {
        let mut task = serde_json::json!({
            "kind": "task", "id": task_id, "contextId": context_id,
            "status": {"state": "working"}
        });
        if let Some(run_id) = run_id {
            task["metadata"] = serde_json::json!({"arbora": {"runId": run_id}});
        }
        ReadStatus::Task(TaskSnapshot {
            terminal_event: Default::default(),
            output: Default::default(),
            usage: Default::default(),
            correlation: TaskCorrelation::from_task(&task).unwrap(),
            state: TaskState::Working,
            plan: None,
        })
    }

    // spec: TUIG-RUNTIME-R012.6.1
    #[test]
    fn turn_usage_renders_missing_values_and_clears_after_read_refusal() {
        let ReadStatus::Task(mut snapshot) =
            task_read("task-one", "context-two", Some("run-three"))
        else {
            panic!("expected task fixture");
        };
        let task = serde_json::json!({"id":"task-one","contextId":"context-two",
            "metadata":{"arbora":{"runId":"run-three","usage":{
            "taskId":"task-one","contextId":"context-two","runId":"run-three",
            "scope":"turn","counts":{"input_tokens":17,"output_tokens":0}}}}});
        snapshot.usage =
            tuig_native::turn_usage::TurnUsage::from_task(&task, &snapshot.correlation);
        let mut binding = None;
        let status = completed_status(Ok(Ok(ReadStatus::Task(snapshot))), &mut binding);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal
            .draw(|frame| draw(frame, &status, Style::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "Input tokens: 17",
            "Output tokens: 0",
            "Total tokens: unavailable",
            "Session usage: unavailable",
            "Cost: unavailable",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        let failed = completed_status(Ok(Err("read refused")), &mut binding);
        terminal
            .draw(|frame| draw(frame, &failed, Style::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("Input tokens: 17"));
        assert!(text.contains("read refused"));
    }

    // spec: TUIG-RUNTIME-R002.1.3.2.2.2.1
    #[test]
    fn completed_reply_renders_literally_and_failed_read_clears_it() {
        let ReadStatus::Task(mut snapshot) =
            task_read("task-one", "context-two", Some("run-three"))
        else {
            panic!("expected task fixture");
        };
        snapshot.state = TaskState::Completed;
        snapshot.output =
            tuig_native::task_output::TaskOutput::Text("[bold]model reply\nsecond line".to_owned());
        let mut binding = None;
        let status = completed_status(Ok(Ok(ReadStatus::Task(snapshot))), &mut binding);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| draw(frame, &status, Style::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("[bold]model reply"));
        assert!(text.contains("second line"));
        assert!(text.contains("Task streaming and approvals remain unavailable"));
        let failed = completed_status(Ok(Err("read refused")), &mut binding);
        terminal
            .draw(|frame| draw(frame, &failed, Style::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("model reply"));
        assert!(text.contains("read refused"));
    }

    // spec: TUIG-RUNTIME-R002.1.4
    #[test]
    fn refresh_rejects_changed_binding_and_retains_omitted_run() {
        let mut binding = None;
        completed_status(
            Ok(Ok(task_read("task-one", "context-two", Some("run-three")))),
            &mut binding,
        );
        let initial = binding.clone();
        for changed in [
            task_read("other-task", "context-two", Some("run-three")),
            task_read("task-one", "other-context", Some("run-three")),
            task_read("task-one", "context-two", Some("other-run")),
        ] {
            let status = completed_status(Ok(Ok(changed)), &mut binding);
            assert_eq!(status, "Task correlation mismatch");
            assert_eq!(binding, initial);
        }
        let status = completed_status(
            Ok(Ok(task_read("task-one", "context-two", None))),
            &mut binding,
        );
        assert!(status.contains("Run ID: run-three"));
        assert_eq!(binding, initial);
    }

    // spec: TUIG-RUNTIME-R002.1.4
    #[test]
    fn refresh_does_not_acquire_a_later_run_binding() {
        let mut binding = None;
        completed_status(
            Ok(Ok(task_read("task-one", "context-two", None))),
            &mut binding,
        );
        let status = completed_status(
            Ok(Ok(task_read("task-one", "context-two", Some("later-run")))),
            &mut binding,
        );
        assert_eq!(status, "Task correlation mismatch");
        assert_eq!(binding.as_ref().unwrap().run_id(), None);
    }

    // spec: TUIG-RUNTIME-R002.1.4
    #[tokio::test]
    async fn denial_and_local_cancellation_preserve_initial_binding() {
        let mut binding = None;
        completed_status(
            Ok(Ok(task_read("task-one", "context-two", Some("run-three")))),
            &mut binding,
        );
        let initial = binding.clone();
        assert_eq!(
            completed_status(
                Ok(Err("Arbora task read refused or unavailable")),
                &mut binding
            ),
            "Arbora task read refused or unavailable"
        );
        assert_eq!(binding, initial);
        let request = tokio::spawn(std::future::pending::<Result<ReadStatus, &'static str>>());
        request.abort();
        assert_eq!(
            completed_status(request.await, &mut binding),
            "Local read canceled. No remote task was canceled."
        );
        assert_eq!(binding, initial);
        assert_eq!(
            completed_status(
                Ok(Ok(task_read(
                    "task-one",
                    "other-context",
                    Some("run-three")
                ))),
                &mut binding,
            ),
            "Task correlation mismatch"
        );
    }

    // spec: TUIG-RUNTIME-R002.7
    #[test]
    fn renders_refusal_without_enabling_task_actions() {
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    "Arbora discovery refused or unavailable",
                    Style::default(),
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Arbora discovery refused"));
        assert!(text.contains("Task streaming and approvals remain unavailable"));
    }

    // spec: TUIG-RUNTIME-R008.1
    #[test]
    fn resize_preserves_wrapped_status_and_keyboard_help() {
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let status = "Local discovery canceled. No remote task was canceled.";
        for width in [100, 40, 100] {
            terminal.backend_mut().resize(width, 16);
            terminal.autoresize().unwrap();
            terminal
                .draw(|frame| draw(frame, status, Style::default()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let lines: Vec<String> = (1..15)
                .map(|y| (1..width - 1).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            let text = lines
                .iter()
                .map(|line| line.trim())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                text.contains(status),
                "status clipped at {width} columns: {text}"
            );
            for help in [
                "r: refresh read",
                "c: cancel local request",
                "q / Ctrl+C: quit",
            ] {
                assert!(
                    text.contains(help),
                    "help clipped at {width} columns: {text}"
                );
            }
        }
    }

    // spec: TUIG-RUNTIME-R008.1
    #[test]
    fn renderer_honors_no_color_and_unknown_theme_fallback() {
        let default_style = presentation_style("tokyo-night", "");
        assert_eq!(presentation_style("unknown-theme", ""), default_style);
        let mut terminal = Terminal::new(TestBackend::new(40, 16)).unwrap();
        for (theme, no_color) in [
            ("tokyo-night", ""),
            ("unknown-theme", ""),
            ("tokyo-night", "1"),
            ("unknown-theme", "0"),
        ] {
            terminal
                .draw(|frame| {
                    draw(
                        frame,
                        "Discovering Arbora...",
                        presentation_style(theme, no_color),
                    )
                })
                .unwrap();
            let cell = &terminal.backend().buffer()[(1, 1)];
            if no_color.is_empty() {
                assert_eq!(cell.fg, Color::Rgb(169, 177, 214));
                assert_eq!(cell.bg, Color::Rgb(26, 27, 38));
            } else {
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content()
                        .iter()
                        .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
                );
            }
        }
    }

    // spec: TUIG-RUNTIME-R002.1.4
    #[test]
    fn task_view_renders_distinct_identifiers_and_missing_run() {
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        for run_id in [None, Some("run-three")] {
            let mut task = serde_json::json!({
                "kind": "task", "id": "task-one", "contextId": "context-two",
                "status": {"state": "working"}
            });
            if let Some(run_id) = run_id {
                task["metadata"] = serde_json::json!({"arbora": {"runId": run_id}});
            }
            let snapshot = TaskSnapshot {
                terminal_event: Default::default(),
                output: Default::default(),
                usage: Default::default(),
                correlation: TaskCorrelation::from_task(&task).unwrap(),
                state: TaskState::Working,
                plan: None,
            };
            terminal
                .draw(|frame| draw(frame, &snapshot_status(&snapshot), Style::default()))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Task ID: task-one"));
            assert!(text.contains("Context ID: context-two"));
            assert!(text.contains(&format!("Run ID: {}", run_id.unwrap_or("unavailable"))));
            assert!(text.contains("State: working"));
            let refused = completed_status(
                Ok(Err("Arbora task read refused or unavailable")),
                &mut Some(snapshot.correlation.clone()),
            );
            terminal
                .draw(|frame| draw(frame, &refused, Style::default()))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Arbora task read refused or unavailable"));
            for protected in ["task-one", "context-two", "run-three", "State: working"] {
                assert!(!text.contains(protected));
            }
        }
    }

    // spec: TUIG-RUNTIME-R001.5
    #[test]
    fn input_required_displays_pending_approval_without_asserting_an_effect() {
        let task = serde_json::json!({
            "kind": "task", "id": "task-one", "contextId": "context-two",
            "status": {"state": "input-required"},
            "metadata": {"arbora": {"runId": "run-three", "approval": {
                "status": "pending", "pendingCallDigest": "c".repeat(64)}}}
        });
        let snapshot = TaskSnapshot {
            terminal_event: Default::default(),
            output: Default::default(),
            usage: Default::default(),
            correlation: TaskCorrelation::from_task(&task).unwrap(),
            state: TaskState::InputRequired,
            plan: None,
        };
        let status = snapshot_status(&snapshot);
        assert!(status.contains("State: input-required"));
        assert!(status.contains("Approval: pending; not granted locally."));
        for effect in [
            "Approval: granted",
            "approved",
            "confirmed",
            "executed",
            "effect applied",
        ] {
            assert!(!status.contains(effect));
        }
        let working = task_read("task-one", "context-two", Some("run-three"));
        if let ReadStatus::Task(snapshot) = working {
            let status = snapshot_status(&snapshot);
            assert!(status.contains("State: working"));
            assert!(!status.contains("Approval: pending"));
        } else {
            panic!("task fixture required");
        }
    }

    // spec: TUIG-RUNTIME-R002.1.4
    #[test]
    fn task_selection_rejects_empty_and_control_identifiers() {
        assert_eq!(selected_task(Err(VarError::NotPresent)), Ok(None));
        assert_eq!(
            selected_task(Ok("task-one".to_owned())),
            Ok(Some("task-one".to_owned()))
        );
        for invalid in ["", " ", "task\nsecret", "task\u{1b}"] {
            assert!(selected_task(Ok(invalid.to_owned())).is_err());
        }
    }

    // spec: TUIG-RUNTIME-R002.1.5
    #[test]
    fn cancellation_requires_confirmation_and_consumes_it_once() {
        let mut cancellation = Cancellation::Unavailable;
        cancellation.arm();
        assert!(!cancellation.confirm());
        cancellation = Cancellation::Ready;
        cancellation.arm();
        cancellation.dismiss();
        assert!(!cancellation.confirm());
        cancellation.arm();
        assert!(cancellation.confirm());
        cancellation.arm();
        assert!(!cancellation.confirm());
        assert_eq!(cancellation, Cancellation::Pending);
    }

    // spec: TUIG-RUNTIME-R002.1.5
    #[tokio::test]
    async fn uncertain_cancellation_requires_successful_read_and_hides_snapshot() {
        let mut binding = None;
        let mut cancellation = Cancellation::Unavailable;
        request_completed(
            Ok(Ok(task_read("task-one", "context-two", Some("run-three")))),
            &mut binding,
            &mut cancellation,
            false,
        );
        let initial = binding.clone();
        let request = tokio::spawn(std::future::pending::<Result<ReadStatus, &'static str>>());
        request.abort();
        let status = request_completed(request.await, &mut binding, &mut cancellation, true);
        assert_eq!(status, CANCELLATION_UNKNOWN);
        assert!(!status.contains("No remote task was canceled"));
        for protected in ["task-one", "context-two", "run-three"] {
            assert!(!status.contains(protected));
        }
        cancellation.arm();
        assert!(!cancellation.confirm());
        cancellation.read_started();
        assert_eq!(
            request_completed(Ok(Err("refused")), &mut binding, &mut cancellation, false),
            CANCELLATION_UNKNOWN
        );
        assert_eq!(binding, initial);
        assert_eq!(cancellation, Cancellation::Unknown);
        request_completed(
            Ok(Ok(task_read("task-one", "context-two", None))),
            &mut binding,
            &mut cancellation,
            false,
        );
        assert_eq!(cancellation, Cancellation::Ready);
        assert_eq!(binding, initial);
    }

    // spec: TUIG-RUNTIME-R002.1.5
    #[test]
    fn only_bound_canceled_response_confirms_remote_cancellation() {
        let mut binding = None;
        let mut cancellation = Cancellation::Unavailable;
        request_completed(
            Ok(Ok(task_read("task-one", "context-two", None))),
            &mut binding,
            &mut cancellation,
            false,
        );
        assert_eq!(
            request_completed(
                Ok(Ok(task_read("task-one", "context-two", None))),
                &mut binding,
                &mut cancellation,
                true
            ),
            CANCELLATION_UNKNOWN
        );
        let ReadStatus::Task(mut canceled) = task_read("task-one", "context-two", None) else {
            panic!("expected task")
        };
        canceled.state = TaskState::Canceled;
        let mut unbound = None;
        assert_eq!(
            request_completed(
                Ok(Ok(ReadStatus::Task(canceled.clone()))),
                &mut unbound,
                &mut cancellation,
                true
            ),
            CANCELLATION_UNKNOWN
        );
        assert!(unbound.is_none());
        let ReadStatus::Task(mut mismatch) = task_read("other-task", "context-two", None) else {
            panic!("expected task")
        };
        mismatch.state = TaskState::Canceled;
        assert_eq!(
            request_completed(
                Ok(Ok(ReadStatus::Task(mismatch))),
                &mut binding,
                &mut cancellation,
                true
            ),
            CANCELLATION_UNKNOWN
        );
        let status = request_completed(
            Ok(Ok(ReadStatus::Task(canceled))),
            &mut binding,
            &mut cancellation,
            true,
        );
        assert!(status.contains("State: canceled"));
        assert_eq!(cancellation, Cancellation::Unavailable);
        cancellation.arm();
        assert!(!cancellation.confirm());
    }

    // spec: TUIG-RUNTIME-R002.1.5
    #[test]
    fn discovery_failed_reads_and_terminal_tasks_do_not_arm_cancellation() {
        let mut binding = None;
        let mut cancellation = Cancellation::Unavailable;
        request_completed(
            Ok(Ok(ReadStatus::Discovery("Agent".to_owned()))),
            &mut binding,
            &mut cancellation,
            false,
        );
        cancellation.arm();
        assert!(!cancellation.confirm());
        request_completed(
            Ok(Ok(task_read("task-one", "context-two", None))),
            &mut binding,
            &mut cancellation,
            false,
        );
        request_completed(Ok(Err("refused")), &mut binding, &mut cancellation, false);
        cancellation.arm();
        assert!(!cancellation.confirm());
        for state in [
            TaskState::Completed,
            TaskState::Canceled,
            TaskState::Failed,
            TaskState::Rejected,
        ] {
            let ReadStatus::Task(mut snapshot) = task_read("task-one", "context-two", None) else {
                panic!("expected task")
            };
            snapshot.state = state;
            request_completed(
                Ok(Ok(ReadStatus::Task(snapshot))),
                &mut binding,
                &mut cancellation,
                false,
            );
            cancellation.arm();
            assert!(!cancellation.confirm());
        }
    }
    #[test]
    fn observation_arguments_are_explicit_and_exclusive() {
        let arguments = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            super::selected_observation(&arguments(&["--observe-task", "task-1"]), false),
            Ok(Some("task-1".to_owned()))
        );
        for values in [
            vec!["--observe-task"],
            vec!["--observe-task", ""],
            vec!["--observe-task", "x", "--prompt", "effect"],
            vec!["--observe-task", "x", "--override"],
            vec!["--observe-task", "bad\nidentity"],
        ] {
            assert!(super::selected_observation(&arguments(&values), false).is_err());
        }
        assert!(super::selected_observation(&arguments(&["--observe-task", "x"]), true).is_err());
        assert!(
            super::selected_observation(&arguments(&["--observe-task", &"x".repeat(513)]), false)
                .is_err()
        );
    }

    #[test]
    fn observation_polling_requires_active_matching_task() {
        let ReadStatus::Task(mut snapshot) =
            task_read("task-one", "context-two", Some("run-three"))
        else {
            panic!("task fixture required")
        };
        let initial = snapshot.correlation.clone();
        for state in [TaskState::Submitted, TaskState::Working] {
            snapshot.state = state;
            assert!(super::observation_continues(&snapshot, Some(&initial)));
        }
        for state in [
            TaskState::InputRequired,
            TaskState::AuthRequired,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Rejected,
            TaskState::Canceled,
        ] {
            snapshot.state = state;
            assert!(!super::observation_continues(&snapshot, Some(&initial)));
        }
        snapshot.state = TaskState::Working;
        let ReadStatus::Task(other) = task_read("task-one", "other-context", Some("run-three"))
        else {
            panic!("task fixture required")
        };
        assert!(!super::observation_continues(
            &snapshot,
            Some(&other.correlation)
        ));
    }

    // spec: TUIG-RUNTIME-R012.1.3
    #[test]
    fn automatic_observation_preserves_confirmation_and_bounds_waiting() {
        let now = tokio::time::Instant::now();
        assert!(!super::observation_expired(None, now));
        assert!(!super::observation_expired(
            Some(now + std::time::Duration::from_secs(1)),
            now
        ));
        assert!(super::observation_expired(Some(now), now));
        let mut active = true;
        let mut status = "Task ID: task-one".to_owned();
        assert!(super::stop_expired_observation(
            Some(now),
            now,
            &mut active,
            &mut status
        ));
        assert!(!super::observation_ready(
            active,
            false,
            Cancellation::Ready
        ));
        assert!(status.contains("Task ID: task-one"));
        assert!(status.contains("Remote execution may continue. Press r to refresh."));
        assert!(super::observation_ready(true, false, Cancellation::Ready));
        assert!(!super::observation_ready(true, true, Cancellation::Ready));
        assert!(!super::observation_ready(false, false, Cancellation::Ready));
        let mut cancellation = Cancellation::Ready;
        cancellation.arm();
        assert!(!super::observation_ready(true, false, cancellation));
        assert_eq!(cancellation, Cancellation::Confirming);
        cancellation.dismiss();
        assert!(super::observation_ready(true, false, cancellation));
    }

    // spec: TUIG-RUNTIME-R012.1.3
    #[test]
    fn automatic_observation_tracks_terminal_results_without_rebinding() {
        for terminal in [TaskState::Completed, TaskState::Failed, TaskState::Canceled] {
            let mut binding = None;
            let mut cancellation = Cancellation::Unavailable;
            super::submission_completed(
                Ok(Ok(task_read("task-one", "context-two", Some("run-three")))),
                &mut binding,
                &mut cancellation,
            );
            let original = binding.clone();
            let ReadStatus::Task(mut snapshot) =
                task_read("task-one", "context-two", Some("run-three"))
            else {
                panic!("task fixture required")
            };
            snapshot.state = TaskState::Working;
            assert!(super::observation_continues(&snapshot, binding.as_ref()));
            snapshot.state = terminal;
            assert!(!super::observation_continues(&snapshot, binding.as_ref()));
            let status = super::request_completed(
                Ok(Ok(ReadStatus::Task(snapshot))),
                &mut binding,
                &mut cancellation,
                false,
            );
            assert!(status.contains("Task ID: task-one"));
            assert_eq!(binding, original);
            assert_eq!(cancellation, Cancellation::Unavailable);
        }
    }

    #[test]
    fn observation_help_does_not_offer_remote_cancellation() {
        let mut terminal = Terminal::new(TestBackend::new(180, 12)).unwrap();
        terminal
            .draw(|frame| {
                super::draw_viewport(
                    frame,
                    "Task ID: task-one",
                    Style::default(),
                    &mut super::Viewport::default(),
                    true,
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("read-only observation"));
        assert!(!text.contains("x: cancel remote task"));
        assert!(text.contains("r: refresh read"));
    }
}

#[cfg(test)]
mod confirmation_view_tests {
    use super::*;
    use crossterm::event::KeyCode;
    use ratatui::{Terminal, backend::TestBackend};

    // spec: TUIG-RUNTIME-R008.2.2
    #[test]
    fn confirmation_is_pinned_and_small_terminals_cannot_confirm() {
        let status =
            "Task ID: task-one\nContext ID: context-two\nRun ID: run-three\nState: working";
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut viewport = Viewport::default();
        terminal
            .draw(|frame| draw_viewport(frame, status, Style::default(), &mut viewport, false))
            .unwrap();
        viewport.handle(KeyCode::End);
        for (width, height, allowed) in [
            (40, 10, true),
            (34, 5, true),
            (33, 5, false),
            (34, 4, false),
        ] {
            terminal.backend_mut().resize(width, height);
            terminal.autoresize().unwrap();
            let mut visible = false;
            terminal
                .draw(|frame| {
                    visible = draw_confirmation(frame, status, Style::default(), Style::default())
                })
                .unwrap();
            assert_eq!(visible, allowed);
            if allowed {
                let buffer = terminal.backend().buffer();
                let first: String = (1..width - 1).map(|x| buffer[(x, 1)].symbol()).collect();
                let second: String = (1..width - 1).map(|x| buffer[(x, 2)].symbol()).collect();
                assert!(first.contains("Cancel selected remote task?"));
                assert!(second.contains("y: confirm | n / Escape: dismiss"));
            }
            let mut cancellation = Cancellation::Confirming;
            assert!(!confirm_visible(&mut cancellation, true, Ok((10, 2))));
            assert!(!confirm_visible(
                &mut cancellation,
                true,
                Err(std::io::Error::other("size unavailable"))
            ));
            assert_eq!(
                confirm_visible(&mut cancellation, visible, Ok((width, height))),
                allowed
            );
            assert_eq!(
                cancellation,
                if allowed {
                    Cancellation::Pending
                } else {
                    Cancellation::Confirming
                }
            );
        }
    }
}
#[cfg(test)]
mod recovery_view_tests;

#[cfg(test)]
mod terminal_point_view_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    use tuig_native::terminal_event::TerminalEvent;

    // spec: TUIG-RUNTIME-R002.1.3.2.2.3
    #[test]
    fn committed_identity_renders_literally_and_failed_read_clears_it() {
        let task = json!({"kind":"task","id":"task","contextId":"ctx","status":{"state":"completed"},
        "metadata":{"arbora":{"runId":"run","terminalEvent":{
            "status":"available","tenant":"tenant","scope":{"tenant":"tenant","scope":{"kind":"graph","graph":"graph"},"incarnation_id":"inc","identity_digest":vec![170;32]},
            "batch_id":"[bold]actual-batch","commit_sequence":7,"ordinal":0,"run_id":"run","kind":"outcome","outcome":"succeeded","result_ref":null,"payload_digest":"b".repeat(64)
        }}}});
        let correlation = TaskCorrelation::from_task(&task).unwrap();
        let snapshot = TaskSnapshot {
            output: Default::default(),
            usage: Default::default(),
            terminal_event: TerminalEvent::from_task(&task, &correlation, TaskState::Completed)
                .unwrap(),
            correlation,
            state: TaskState::Completed,
            plan: None,
        };
        let mut binding = None;
        let status = completed_status(Ok(Ok(ReadStatus::Task(snapshot))), &mut binding);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| draw(frame, &status, Style::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("[bold]actual-batch"));
        assert!(text.contains("Commit sequence: 7"));
        let failed = completed_status(Ok(Err("read refused")), &mut binding);
        terminal
            .draw(|frame| draw(frame, &failed, Style::default()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains("actual-batch"));
        assert!(!text.contains("Commit sequence: 7"));
    }
}

#[cfg(test)]
mod request_settings_tests {
    use super::*;
    // spec: TUIG-RUNTIME-R002.1.3.2.2.2.2
    #[test]
    fn queue_timeout_is_bounded_and_does_not_accept_ambiguous_values() {
        assert_eq!(
            submission_timeout(Err(env::VarError::NotPresent))
                .unwrap()
                .as_secs(),
            300
        );
        for value in ["1", "120", "900"] {
            assert_eq!(
                submission_timeout(Ok(value.into())).unwrap().as_secs(),
                value.parse::<u64>().unwrap()
            );
        }
        for value in [
            "",
            "0",
            "901",
            "-1",
            "1.5",
            " 120",
            "+120",
            "18446744073709551616",
        ] {
            assert!(submission_timeout(Ok(value.into())).is_err());
        }
    }
    // spec: TUIG-RUNTIME-R002.1.3.2.2.2.3
    #[test]
    fn committed_events_require_explicit_true() {
        assert!(!committed_event_reads(Err(env::VarError::NotPresent)).unwrap());
        assert!(!committed_event_reads(Ok("false".into())).unwrap());
        assert!(committed_event_reads(Ok("true".into())).unwrap());
        for value in ["", "1", "yes", "TRUE", " true"] {
            assert!(committed_event_reads(Ok(value.into())).is_err());
        }
    }
}
