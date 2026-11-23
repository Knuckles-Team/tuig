//! The mounted task-history view: only the current page and its navigation.
use crate::presentation::Viewport;
use crossterm::event::{KeyCode, MouseEvent};
use ratatui::{
    Frame,
    style::Style,
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};
use std::sync::Arc;
use tuig_native::{
    ArboraClient,
    correlation::TaskState,
    task_history::{HistoryStatus, TaskHistory},
};

#[derive(Default)]
pub struct TaskHistoryView {
    history: TaskHistory,
    viewport: Viewport,
}

fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Submitted => "submitted",
        TaskState::Working => "working",
        TaskState::InputRequired => "input-required",
        TaskState::AuthRequired => "auth-required",
        TaskState::Completed => "completed",
        TaskState::Canceled => "canceled",
        TaskState::Failed => "failed",
        TaskState::Rejected => "rejected",
    }
}

impl TaskHistoryView {
    pub fn is_open(&self) -> bool {
        self.history.is_open()
    }
    pub fn pending(&self) -> bool {
        self.history.pending()
    }
    pub fn open(&mut self, client: Arc<ArboraClient>) {
        self.viewport = Viewport::default();
        self.history.open(client);
    }
    pub fn close(&mut self) {
        self.history.close();
    }
    pub async fn complete(&mut self) {
        self.history.complete().await;
    }

    /// P rereads the previous page; N reads the next; R resets the path.
    pub fn key(&mut self, key: KeyCode, client: Arc<ArboraClient>) -> bool {
        if !self.is_open() {
            return false;
        }
        match key {
            KeyCode::Esc => self.close(),
            KeyCode::Char('p') => self.history.previous(client),
            KeyCode::Char('n') => self.history.next(client),
            KeyCode::Char('r') => self.history.reset(client),
            _ => {
                self.viewport.handle(key);
            }
        }
        true
    }

    pub fn mouse(&mut self, event: MouseEvent) -> bool {
        self.is_open() && self.viewport.mouse(event)
    }

    fn message(&self) -> String {
        match (self.history.status(), self.history.page()) {
            (HistoryStatus::Reading, _) => "Reading task page...".to_owned(),
            (HistoryStatus::Refused, _) => "Task history unavailable.".to_owned(),
            (HistoryStatus::Page, None) => "Task history unavailable.".to_owned(),
            (HistoryStatus::Page, Some(page)) if page.tasks.is_empty() => {
                "No tasks on this page.".to_owned()
            }
            (HistoryStatus::Page, Some(page)) => page
                .tasks
                .first()
                .map(|task| task.reply.clone().unwrap_or_else(|| "No reply.".to_owned()))
                .unwrap_or_else(|| "No tasks on this page.".to_owned()),
        }
    }

    fn help(&self) -> String {
        if self.history.status().ne(&HistoryStatus::Page) || self.history.next_cursor().is_some() {
            "P: previous | N: next page | R: reset | Esc: close".to_owned()
        } else {
            "P: previous | End of task list | R: reset | Esc: close".to_owned()
        }
    }

    fn rows(&self) -> Vec<Line<'static>> {
        self.history
            .page()
            .map(|page| {
                page.tasks
                    .iter()
                    .map(|task| {
                        Line::from(format!(
                            "{} · {}",
                            state_label(task.state),
                            task.correlation.task_id()
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn draw(&mut self, frame: &mut Frame, style: Style, border_style: Style) {
        if !self.is_open() {
            return;
        }
        let block = Block::bordered()
            .border_style(border_style)
            .title_style(border_style)
            .title("Task history (read-only)");
        let inner = block.inner(frame.area());
        let mut lines = self.rows();
        lines.push(Line::from(""));
        lines.push(Line::from(self.message()));
        lines.push(Line::from(""));
        lines.push(Line::from(self.help()));
        let paragraph = Paragraph::new(lines)
            .style(style)
            .wrap(Wrap { trim: false });
        self.viewport
            .measure(paragraph.line_count(inner.width), inner.height);
        frame.render_widget(
            paragraph.scroll((self.viewport.offset(), 0)).block(block),
            frame.area(),
        );
    }
}

impl Drop for TaskHistoryView {
    fn drop(&mut self) {
        self.close();
    }
}
