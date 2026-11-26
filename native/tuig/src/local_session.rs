use crate::{
    coding_view::PendingApproval,
    composer::Composer,
    local_runtime::{Options, finish_backend, spawn_backend},
    presentation::{ThemeSelection, Viewport, accepts_key_event},
};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyModifiers,
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};
use std::{cell::Cell, io::Write, rc::Rc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::timeout,
};
use tuig_native::{
    acp::{HostRequest, TextClient},
    coding_approval::{ApprovalPreview, Capability, Choice},
    host_router::HostRouter,
    task_output::terminal_text,
};

const TRANSCRIPT_BYTES: usize = 128 * 1024;
const TURN_LIMIT: usize = 64;

struct TerminalOutput<W: Write> {
    writer: Option<std::io::BufWriter<W>>,
    cleanup: Rc<Cell<bool>>,
}

impl<W: Write> Write for TerminalOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cleanup.get() {
            Ok(bytes.len())
        } else {
            self.writer
                .as_mut()
                .expect("output owned until drop")
                .write(bytes)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.cleanup.get() {
            Ok(())
        } else {
            self.writer
                .as_mut()
                .expect("output owned until drop")
                .flush()
        }
    }
}

impl<W: Write> Drop for TerminalOutput<W> {
    fn drop(&mut self) {
        // Do not replay a partially written frame after leaving the alternate screen.
        if let Some(writer) = self.writer.take() {
            let _ = writer.into_parts();
        }
    }
}

enum Update {
    Ready(&'static str),
    Text(String),
    Approval(PendingApproval),
}

#[derive(Default)]
struct Transcript {
    text: String,
    clipped: bool,
}

impl Transcript {
    fn append(&mut self, text: &str) {
        self.text.push_str(&terminal_text(text));
        if self.text.len() > TRANSCRIPT_BYTES {
            let mut cut = self.text.len() - TRANSCRIPT_BYTES;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
            self.clipped = true;
        }
    }
}

async fn signaled(signal: &mut watch::Receiver<bool>) {
    let _ = signal.wait_for(|value| *value).await;
}

async fn deliver(
    updates: &mpsc::Sender<Update>,
    quit: &mut watch::Receiver<bool>,
    update: Update,
) -> Result<(), &'static str> {
    tokio::select! {
        biased;
        _ = signaled(quit) => Err("Local session closed"),
        result = updates.send(update) => result.map_err(|_| "Local display closed"),
    }
}

async fn turn<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    client: &mut TextClient<R, W>,
    router: &mut HostRouter,
    text: &str,
    updates: &mpsc::Sender<Update>,
    cancel: &mut watch::Receiver<bool>,
    quit: &mut watch::Receiver<bool>,
) -> Result<&'static str, &'static str> {
    let id = submit_prompt(client, text, cancel, quit).await?;
    let reply = wait_turn(client, router, id, updates, cancel, quit).await;
    let value = match reply {
        TurnReply::Value(value) => value,
        TurnReply::Failed(message) => return Err(message),
        TurnReply::TimedOut => {
            drain_cancel(client, id).await;
            return Err("Local turn timed out; session closed without replay.");
        }
        TurnReply::Interrupted => cancel_outcome(client, id).await?,
    };
    match value["stopReason"].as_str() {
        Some("end_turn") => Ok("Turn completed. p: compose next turn"),
        Some("cancelled") => Ok("Turn canceled (acknowledged). p: compose next turn"),
        _ => Err("Local turn outcome unavailable; session closed without replay."),
    }
}

async fn submit_prompt<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    client: &mut tuig_native::acp::TextClient<R, W>,
    text: &str,
    cancel: &mut watch::Receiver<bool>,
    quit: &mut watch::Receiver<bool>,
) -> Result<u64, &'static str> {
    tokio::select! {
        biased;
        _ = signaled(quit) => Err("Local submission interrupted; outcome unknown. No replay."),
        _ = signaled(cancel) => Err("Local submission interrupted; outcome unknown. No replay."),
        result = timeout(Duration::from_secs(5), client.prompt(text)) => match result {
            Ok(id) => id,
            Err(_) => Err("Local submission timed out; outcome unknown. No replay."),
        },
    }
}

async fn wait_turn<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    client: &mut TextClient<R, W>,
    router: &mut HostRouter,
    id: u64,
    updates: &mpsc::Sender<Update>,
    cancel: &mut watch::Receiver<bool>,
    quit: &mut watch::Receiver<bool>,
) -> TurnReply {
    let mut ask = async |preview: ApprovalPreview| {
        let (reply, decision) = oneshot::channel();
        let pending = Update::Approval(PendingApproval::new(preview, reply));
        if updates.send(pending).await.is_err() {
            return Choice::Deny;
        }
        decision.await.unwrap_or(Choice::Deny)
    };
    let host = async |request: HostRequest| Some(router.handle(&request, &mut ask).await);
    tokio::select! {
        biased;
        _ = signaled(quit) => TurnReply::Interrupted,
        _ = signaled(cancel) => TurnReply::Interrupted,
        result = timeout(Duration::from_secs(330), client.response_hosted(id, |chunk: &str| {
            let update = Update::Text(terminal_text(chunk));
            let updates = updates.clone();
            async move { updates.send(update).await.map_err(|_| "Local display closed") }
        }, host)) => match result {
            Ok(Ok(value)) => TurnReply::Value(value),
            Ok(Err(message)) => TurnReply::Failed(message),
            Err(_) => TurnReply::TimedOut,
        },
    }
}

enum TurnReply {
    Value(serde_json::Value),
    Failed(&'static str),
    TimedOut,
    Interrupted,
}

/// Cancel the in-flight turn and return the backend's final reply for it.
async fn cancel_outcome<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    client: &mut tuig_native::acp::TextClient<R, W>,
    id: u64,
) -> Result<serde_json::Value, &'static str> {
    timeout(Duration::from_secs(5), async {
        client.cancel().await?;
        client.response(id, async |_| Ok(())).await
    })
    .await
    .map_err(|_| "Cancellation outcome unknown; session closed without replay.")?
}

/// Drain a cancel whose outcome is no longer needed, ignoring its result.
async fn drain_cancel<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    client: &mut tuig_native::acp::TextClient<R, W>,
    id: u64,
) {
    let _ = cancel_outcome(client, id).await;
}

async fn session(
    options: Options,
    mut commands: mpsc::Receiver<String>,
    updates: mpsc::Sender<Update>,
    mut cancel: watch::Receiver<bool>,
    mut quit: watch::Receiver<bool>,
) -> Result<(), &'static str> {
    let (mut child, client, cwd) = spawn_backend(&options.model, options.root.as_deref())?;
    let mut client = client.with_sequential_turns().with_host_capabilities();
    let result = async {
        tokio::select! {
            biased;
            _ = signaled(&mut quit) => return Ok(()),
            result = timeout(Duration::from_secs(30), client.initialize(&cwd)) => result.map_err(|_| "Local session initialization timed out")??,
        }
        let session_id = client.session_id().ok_or("Local session identifier unavailable")?.to_owned();
        let mut router = HostRouter::new(&session_id, &cwd, Capability::ALL.into());
        deliver(&updates, &mut quit, Update::Ready("Local session ready. p: compose a turn")).await?;
        for _ in 0..TURN_LIMIT {
            let text = tokio::select! {
                biased;
                _ = signaled(&mut quit) => return Ok(()),
                result = client.idle() => return result,
                command = commands.recv() => match command { Some(text) => text, None => return Ok(()) },
            };
            let status = turn(&mut client, &mut router, &text, &updates, &mut cancel, &mut quit).await?;
            if *quit.borrow() { return Ok(()); }
            deliver(&updates, &mut quit, Update::Ready(status)).await?;
        }
        Err("Local session turn limit reached. Start a new process; history is not resumed.")
    }.await;
    finish_backend(client, &mut child, result).await
}

fn draw_status(
    frame: &mut ratatui::Frame,
    status: &str,
    transcript: &Transcript,
    viewport: &mut Viewport,
    style: Style,
    border: Style,
    follow: bool,
) {
    let clipped = if transcript.clipped {
        "\nDisplay clipped; backend history unchanged."
    } else {
        ""
    };
    let header = Paragraph::new(format!("{status}\nLocal process history only; quitting ends this session.\np: compose | c: cancel | F6: theme | q / Ctrl+C: quit{clipped}"))
        .style(style).wrap(Wrap { trim: false });
    let block = Block::bordered()
        .title("Local ACP session")
        .border_style(border)
        .title_style(border);
    let inner = block.inner(frame.area());
    frame.render_widget(block, frame.area());
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let header_height = header
        .line_count(inner.width)
        .min(usize::from(inner.height.saturating_sub(1))) as u16;
    let [header_area, body] = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(header_height),
        ratatui::layout::Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(header, header_area);
    let paragraph = Paragraph::new(transcript.text.as_str())
        .style(style)
        .wrap(Wrap { trim: false });
    viewport.measure(paragraph.line_count(body.width), body.height);
    if follow {
        viewport.handle(KeyCode::End);
    }
    frame.render_widget(paragraph.scroll((viewport.offset(), 0)), body);
}

pub async fn run(options: Options, themes: ThemeSelection) -> Result<(), &'static str> {
    tokio::task::LocalSet::new()
        .run_until(run_ui(options, themes))
        .await
}

fn restore_terminal(output: &mut impl Write) {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        output,
        DisableBracketedPaste,
        DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}

struct TerminalCleanup<W: Write> {
    output: W,
    cleanup: Rc<Cell<bool>>,
}

impl<W: Write> Drop for TerminalCleanup<W> {
    fn drop(&mut self) {
        self.cleanup.set(true);
        restore_terminal(&mut self.output);
    }
}

struct LocalTerminal<'a> {
    terminal: ratatui::Terminal<CrosstermBackend<TerminalOutput<std::fs::File>>>,
    cleanup: TerminalCleanup<&'a mut std::fs::File>,
}

/// Open the alternate screen and terminal input for the local session.
fn setup_terminal<'a>(output: &'a mut std::fs::File) -> Result<LocalTerminal<'a>, &'static str> {
    let cleanup = Rc::new(Cell::new(false));
    let initialized = (|| {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(output, crossterm::terminal::EnterAlternateScreen)?;
        ratatui::Terminal::new(CrosstermBackend::new(TerminalOutput {
            writer: Some(std::io::BufWriter::with_capacity(
                64 * 1024,
                output.try_clone()?,
            )),
            cleanup: Rc::clone(&cleanup),
        }))
    })();
    let terminal = match initialized {
        Ok(terminal) => terminal,
        Err(_) => {
            restore_terminal(output);
            return Err("Cannot initialize local terminal");
        }
    };
    let cleanup = TerminalCleanup { output, cleanup };
    if crate::enable_terminal_input(cleanup.output).is_err() {
        return Err("Cannot enable terminal input");
    }
    Ok(LocalTerminal { terminal, cleanup })
}

/// The two outbound control channels of the interactive local session.
struct ControlChannels<'a> {
    commands: &'a mpsc::Sender<String>,
    cancel: &'a watch::Sender<bool>,
}

/// Display state for the interactive local session.
struct UiState {
    composer: Composer,
    initial: Option<String>,
    transcript: Transcript,
    viewport: Viewport,
    follow: bool,
    ready: bool,
    active: bool,
    starting: bool,
    status: String,
    outcome: Result<(), &'static str>,
    worker_done: bool,
    approval: Option<PendingApproval>,
}

impl UiState {
    fn new() -> Self {
        Self {
            composer: Composer::local(),
            initial: None,
            transcript: Transcript::default(),
            viewport: Viewport::default(),
            follow: true,
            ready: false,
            active: false,
            starting: true,
            status: "Starting local ACP session...".to_owned(),
            outcome: Ok(()),
            worker_done: false,
            approval: None,
        }
    }

    fn apply_update(
        &mut self,
        update: Update,
        channels: &ControlChannels,
    ) -> Result<(), &'static str> {
        match update {
            Update::Text(text) => {
                self.transcript.append(&text);
            }
            Update::Ready(message) => self.apply_ready(message, channels)?,
            Update::Approval(pending) => {
                self.status = "Approval pending. y: approve | n: deny".to_owned();
                self.approval = Some(pending);
            }
        }
        Ok(())
    }

    fn apply_ready(
        &mut self,
        message: &str,
        channels: &ControlChannels,
    ) -> Result<(), &'static str> {
        channels.cancel.send_replace(false);
        self.ready = true;
        self.active = false;
        self.status = message.to_owned();
        self.transcript.append(&format!("\n{message}\n"));
        if !self.starting {
            return Ok(());
        }
        self.starting = false;
        match self.initial.take() {
            Some(initial) => self.queue_turn(&initial, channels.commands),
            None => {
                self.composer.open = true;
                Ok(())
            }
        }
    }

    fn queue_turn(
        &mut self,
        text: &str,
        commands: &mpsc::Sender<String>,
    ) -> Result<(), &'static str> {
        commands
            .try_send(text.to_owned())
            .map_err(|_| "Local prompt queue unavailable")?;
        self.transcript
            .append(&format!("\nYou: {text}\nAssistant: "));
        self.mark_turn_running();
        Ok(())
    }

    fn mark_turn_running(&mut self) {
        self.ready = false;
        self.active = true;
        self.status = "Local turn running... c: cancel".to_owned();
    }

    fn worker_exited(&mut self, outcome: Result<(), &'static str>) {
        self.worker_done = true;
        self.ready = false;
        self.active = false;
        self.composer.open = false;
        self.outcome = outcome;
        self.status = format!("Session closed: {}", outcome.err().unwrap_or("closed"));
    }

    /// Handle one terminal event; returns true when the event ends the loop.
    fn handle_event(
        &mut self,
        event: Event,
        themes: &mut ThemeSelection,
        channels: &ControlChannels,
    ) -> bool {
        match event {
            Event::Paste(text) if self.composer.open => {
                self.composer.paste(&text);
                false
            }
            Event::Mouse(mouse) if !self.composer.open => {
                if self.viewport.mouse(mouse) {
                    self.follow = false;
                }
                false
            }
            Event::Key(key) if accepts_key_event(key) => self.handle_key(key, themes, channels),
            _ => false,
        }
    }

    fn handle_key(
        &mut self,
        key: KeyEvent,
        themes: &mut ThemeSelection,
        channels: &ControlChannels,
    ) -> bool {
        if themes.key(key) {
            return false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.approval.is_some() {
            self.answer_approval(key);
            return false;
        }
        if self.composer.open {
            self.composer_key(key, channels.commands);
            return false;
        }
        self.global_key(key, channels.cancel)
    }

    fn answer_approval(&mut self, key: KeyEvent) {
        if let Some(choice) = self
            .approval
            .as_mut()
            .and_then(|pending| pending.answer(key))
        {
            self.approval = None;
            self.status = format!("Request answered: {choice:?}. Local turn running... c: cancel");
        }
    }

    fn composer_key(&mut self, key: KeyEvent, commands: &mpsc::Sender<String>) {
        if let Some(text) = self.composer.key(key)
            && self.ready
            && commands.try_send(text.clone()).is_ok()
        {
            self.transcript
                .append(&format!("\nYou: {text}\nAssistant: "));
            self.follow = true;
            self.composer.clear();
            self.composer.open = false;
            self.mark_turn_running();
        }
    }

    fn global_key(&mut self, key: KeyEvent, cancel: &watch::Sender<bool>) -> bool {
        if key.code == KeyCode::Char('q') {
            return true;
        }
        match key.code {
            KeyCode::Char('p') if self.ready => self.composer.open = true,
            KeyCode::Char('c') if self.active => {
                cancel.send_replace(true);
                self.status = "Canceling local turn; waiting for acknowledgement...".to_owned();
            }
            code => {
                if self.viewport.handle(code) {
                    self.follow = code == KeyCode::End;
                }
            }
        }
        false
    }
}

fn draw_local(frame: &mut ratatui::Frame, ui: &mut UiState, themes: &ThemeSelection) {
    if ui.composer.open {
        ui.composer
            .draw(frame, themes.style(), themes.border_style());
    } else {
        draw_status(
            frame,
            &ui.status,
            &ui.transcript,
            &mut ui.viewport,
            themes.style(),
            themes.border_style(),
            ui.follow,
        );
    }
    if let Some(pending) = &ui.approval {
        pending.draw(frame);
    }
}

async fn run_event_loop(
    terminal: &mut ratatui::Terminal<CrosstermBackend<TerminalOutput<std::fs::File>>>,
    ui: &mut UiState,
    themes: &mut ThemeSelection,
    updates: &mut mpsc::Receiver<Update>,
    events: &mut EventStream,
    channels: &ControlChannels<'_>,
    worker: &mut tokio::task::JoinHandle<Result<(), &'static str>>,
) -> Result<(), &'static str> {
    let mut redraw = tokio::time::interval(Duration::from_millis(33));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            biased;
            _ = &mut interrupt => return Ok(()),
            _ = redraw.tick() => {
                terminal.draw(|frame| draw_local(frame, ui, themes))
                    .map_err(|_| "Cannot draw terminal")?;
            }
            update = updates.recv(), if !ui.worker_done => {
                if let Some(update) = update {
                    ui.apply_update(update, channels)?;
                } else {
                    ui.worker_exited(
                        (&mut *worker)
                            .await
                            .unwrap_or(Err("Local runtime interrupted; no replay.")),
                    );
                }
            }
            event = events.next() => {
                let Some(Ok(event)) = event else { return Err("Terminal input unavailable"); };
                if ui.handle_event(event, themes, channels) {
                    return Ok(());
                }
            }
        }
    }
}

/// Reap the session worker once the display loop has ended.
async fn settle_worker(
    worker: tokio::task::JoinHandle<Result<(), &'static str>>,
    worker_done: bool,
    settled: Result<(), &'static str>,
) -> Result<(), &'static str> {
    if worker_done {
        settled
    } else {
        worker
            .await
            .unwrap_or(Err("Local runtime cleanup interrupted"))
    }
}

async fn run_ui(mut options: Options, mut themes: ThemeSelection) -> Result<(), &'static str> {
    let initial = options.prompt.take();
    let (commands, command_rx) = mpsc::channel(1);
    let (update_tx, mut updates) = mpsc::channel(16);
    let (cancel, cancel_rx) = watch::channel(false);
    let (quit, quit_rx) = watch::channel(false);
    let _output = crate::local_runtime::Output::new()?;
    let mut output = rustix::io::fcntl_dupfd_cloexec(std::io::stdout(), 3)
        .map(std::fs::File::from)
        .map_err(|_| "Cannot open terminal output")?;
    let LocalTerminal {
        mut terminal,
        cleanup: terminal_cleanup,
    } = setup_terminal(&mut output)?;
    let mut worker =
        tokio::task::spawn_local(session(options, command_rx, update_tx, cancel_rx, quit_rx));
    let mut ui = UiState::new();
    ui.initial = initial;
    let mut events = EventStream::new();
    let result = run_event_loop(
        &mut terminal,
        &mut ui,
        &mut themes,
        &mut updates,
        &mut events,
        &ControlChannels {
            commands: &commands,
            cancel: &cancel,
        },
        &mut worker,
    )
    .await;
    quit.send_replace(true);
    drop(commands);
    drop(updates);
    // Ratatui's Drop logs failed cursor restoration to stderr, which can itself
    // panic when stderr shares this full terminal. Cleanup writes are best effort.
    drop(terminal_cleanup);
    result.and(settle_worker(worker, ui.worker_done, ui.outcome).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: TUIG-RUNTIME-R011.14
    #[test]
    fn terminal_cleanup_runs_during_unwinding() {
        let cleanup = Rc::new(Cell::new(false));
        let mut output = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = TerminalCleanup {
                output: &mut output,
                cleanup: Rc::clone(&cleanup),
            };
            panic!("interrupted UI");
        }));
        assert!(result.is_err());
        assert!(cleanup.get());
        assert!(output.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
    }

    // spec: TUIG-RUNTIME-R011.14
    #[test]
    fn terminal_output_only_ignores_errors_during_cleanup() {
        struct Blocked;
        impl Write for Blocked {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
        }
        let mut output = TerminalOutput {
            writer: Some(std::io::BufWriter::with_capacity(1, Blocked)),
            cleanup: Rc::new(Cell::new(false)),
        };
        assert!(output.write_all(b"frame").is_err());
        assert!(output.flush().is_err());
        output.cleanup.set(true);
        assert!(output.write_all(b"cursor restoration").is_ok());
        assert!(output.flush().is_ok());
    }

    // spec: TUIG-RUNTIME-R011.14
    #[test]
    fn transcript_is_bounded_unicode_safe_and_inert() {
        let mut transcript = Transcript::default();
        transcript.append(&"界".repeat(TRANSCRIPT_BYTES));
        transcript.append("\u{1b}]52;c;bad\u{7}\u{202e}safe");
        assert!(transcript.text.len() <= TRANSCRIPT_BYTES);
        assert!(transcript.clipped);
        assert!(transcript.text.ends_with("safe"));
        assert!(!transcript.text.contains(['\u{1b}', '\u{7}', '\u{202e}']));
    }

    // spec: TUIG-RUNTIME-R011.14
    #[tokio::test]
    async fn quit_interrupts_full_display_queue() {
        let (sender, _receiver) = mpsc::channel(1);
        sender.send(Update::Text("full".to_owned())).await.unwrap();
        let (quit, mut receiver) = watch::channel(false);
        quit.send_replace(true);
        assert!(
            timeout(
                Duration::from_millis(100),
                deliver(&sender, &mut receiver, Update::Ready("blocked"))
            )
            .await
            .unwrap()
            .is_err()
        );
    }
}

#[cfg(test)]
mod backpressure_tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split};
    use tuig_native::acp::TextClient;

    // spec: TUIG-RUNTIME-R011.14
    #[tokio::test]
    async fn cancel_and_quit_drain_the_same_id_when_display_is_full() {
        for quitting in [false, true] {
            let (local, remote) = tokio::io::duplex(4096);
            let (read, write) = split(local);
            let (rr, mut rw) = split(remote);
            let mut reader = BufReader::new(rr);
            let mut client = TextClient::new(read, write).with_sequential_turns();
            let (updates, _display) = mpsc::channel(1);
            updates
                .send(Update::Text("already full".to_owned()))
                .await
                .unwrap();
            let (cancel, mut cancel_rx) = watch::channel(false);
            let (quit, mut quit_rx) = watch::channel(false);
            let backend = async {
                for id in [1, 2] {
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let response = if id == 1 {
                        json!({"protocolVersion":1})
                    } else {
                        json!({"sessionId":"session"})
                    };
                    rw.write_all(
                        format!("{}\n", json!({"jsonrpc":"2.0","id":id,"result":response}))
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                }
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let prompt: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(prompt["id"], 3);
                rw.write_all(format!("{}\n", json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"blocked"}}}})).as_bytes()).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                if quitting {
                    quit.send_replace(true);
                } else {
                    cancel.send_replace(true);
                }
                line.clear();
                reader.read_line(&mut line).await.unwrap();
                let cancellation: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(cancellation["method"], "session/cancel");
                rw.write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"stopReason\":\"cancelled\"}}\n",
                )
                .await
                .unwrap();
            };
            let frontend = async {
                client.initialize("/workspace").await.unwrap();
                let mut router = HostRouter::new("session", "/workspace", Capability::ALL.into());
                let status = timeout(
                    Duration::from_secs(1),
                    turn(
                        &mut client,
                        &mut router,
                        "prompt",
                        &updates,
                        &mut cancel_rx,
                        &mut quit_rx,
                    ),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(status.contains("acknowledged"));
                assert_eq!(client.prompt("next").await.unwrap(), 4);
            };
            tokio::join!(backend, frontend);
        }
    }
}
