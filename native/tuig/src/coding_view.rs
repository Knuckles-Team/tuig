//! The pending-approval pane of the local session (TUIG-RUNTIME-R011.8).
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, widgets::Clear};
use tokio::sync::oneshot;
use tuig_native::coding_approval::{ApprovalPreview, Choice};

/// One request waiting for the person; dropping it answers deny.
pub struct PendingApproval {
    preview: ApprovalPreview,
    reply: oneshot::Sender<Choice>,
}

impl PendingApproval {
    pub fn new(preview: ApprovalPreview, reply: oneshot::Sender<Choice>) -> Self {
        Self { preview, reply }
    }

    /// Draw the exact preview over the lower part of the session.
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let screen = frame.area();
        let height = (screen.height * 3 / 4).max(screen.height.min(6));
        let area = Rect::new(
            screen.x,
            screen.y + screen.height - height,
            screen.width,
            height,
        );
        frame.render_widget(Clear, area);
        self.preview.render(frame, area);
    }

    /// `y` approves and `n` denies; any other key leaves the request pending.
    pub fn answer(self, key: KeyEvent) -> Result<Choice, Self> {
        let choice = match key.code {
            KeyCode::Char('y') => Choice::Approve,
            KeyCode::Char('n') | KeyCode::Esc => Choice::Deny,
            _ => return Err(self),
        };
        let _ = self.reply.send(choice);
        Ok(choice)
    }
}
