//! Loading indicator component with spinner animation.

use crate::{Cell, Color, Component, Event, Rect, Size, Surface};

/// Spinner frames for animation.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// A loading indicator with optional message and cancellation.
pub struct Loader {
    message: Option<String>,
    frame: usize,
    cancelled: bool,
    dirty: bool,
    focused: bool,
    fg_color: Color,
}

impl Loader {
    pub fn new() -> Self {
        Self {
            message: None,
            frame: 0,
            cancelled: false,
            dirty: true,
            focused: false,
            fg_color: Color::Default,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.fg_color = color;
        self
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
        self.dirty = true;
    }

    /// Advance the spinner frame. Call this on a tick.
    pub fn tick(&mut self) {
        if !self.cancelled {
            self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
            self.dirty = true;
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.message = Some("Cancelled".to_string());
        self.dirty = true;
    }

    pub fn reset(&mut self) {
        self.cancelled = false;
        self.frame = 0;
        self.dirty = true;
    }

    pub fn set_done(&mut self, msg: impl Into<String>) {
        self.cancelled = true;
        self.message = Some(msg.into());
        self.dirty = true;
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Loader {
    fn name(&self) -> &str {
        "Loader"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }

        if let crate::Event::Key(key) = event {
            match key.code {
                crate::KeyCode::Char('c') if key.modifiers.ctrl => {
                    self.cancel();
                    true
                }
                crate::KeyCode::Escape => {
                    self.cancel();
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let mut col = area.x;

        // Spinner or done indicator
        let indicator = if self.cancelled {
            '✓'
        } else {
            SPINNER_FRAMES[self.frame]
        };

        let fg = if self.cancelled {
            Color::Green
        } else {
            self.fg_color
        };

        if col < area.x + area.width {
            surface.set(area.y, col, Cell::new(indicator).with_fg(fg));
            col += 1;
        }
        if col < area.x + area.width {
            surface.set(area.y, col, Cell::new(' '));
            col += 1;
        }

        // Message
        if let Some(ref msg) = self.message {
            let available = (area.x + area.width).saturating_sub(col) as usize;
            let text: String = msg.chars().take(available).collect();
            for (i, c) in text.chars().enumerate() {
                let c2 = col + i as u16;
                if c2 < area.x + area.width {
                    surface.set(area.y, c2, Cell::new(c).with_fg(fg));
                }
            }
            col += text.len() as u16;
        }

        // Clear remainder
        for c in col..area.x + area.width {
            surface.set(area.y, c, Cell::new(' '));
        }
    }

    fn min_size(&self) -> Size {
        let msg_width = self.message.as_ref().map_or(0, |m| m.len()) as u16;
        Size {
            width: 3 + msg_width,
            height: 1,
        }
    }

    fn on_focus(&mut self) {
        self.focused = true;
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        self.dirty = true;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}
