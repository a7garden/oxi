//! Cancellable loader overlay with spinner animation.
//!
//! Shows a braille-spinner + message while a background operation is in
//! progress. The user can press Esc at any time to abort.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use super::{centered_layout, OverlayAction, OverlayComponent};
use oxi_tui::Theme;

// ---------------------------------------------------------------------------
// Spinner frames (braille dance)
// ---------------------------------------------------------------------------

const SPINNER_FRAMES: &[&str] = &[
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
];

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

pub struct CancellableLoader {
    message: String,
    aborted: bool,
    frame: usize,
}

impl std::fmt::Debug for CancellableLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellableLoader")
            .field("message", &self.message)
            .field("aborted", &self.aborted)
            .finish()
    }
}

impl CancellableLoader {
    /// Create a new loader with the given status message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            aborted: false,
            frame: 0,
        }
    }

    /// Advance the spinner by one tick.
    pub fn tick(&mut self) {
        self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
    }

    /// Whether the user pressed Esc to abort.
    pub fn aborted(&self) -> bool {
        self.aborted
    }

    /// Update the status message.
    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = msg.into();
    }

    /// Current status message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl OverlayComponent for CancellableLoader {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match key.code {
            KeyCode::Esc => {
                self.aborted = true;
                OverlayAction::Close
            }
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.40, 0.05);
        // Ensure at least 3 rows tall for borders + content.
        let popup = Rect {
            height: popup.height.max(3),
            ..popup
        };

        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let spinner_frame = SPINNER_FRAMES[self.frame];

        let content = Line::from(vec![
            Span::styled(
                format!("{} ", spinner_frame),
                styles.accent,
            ),
            Span::styled(self.message.clone(), styles.normal),
            Span::styled(" (Esc cancel)", styles.muted),
        ]);

        frame.render_widget(
            Paragraph::new(content),
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: inner.height,
            },
        );
    }

    fn hint(&self) -> &str {
        " Esc cancel"
    }
}
