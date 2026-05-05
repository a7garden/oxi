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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_loader_new() {
        let loader = Loader::new();
        assert!(loader.message.is_none());
        assert_eq!(loader.frame, 0);
        assert!(!loader.cancelled);
        assert!(loader.dirty);
    }

    #[test]
    fn test_loader_with_message() {
        let loader = Loader::new().with_message("Loading...");
        assert_eq!(loader.message.as_deref(), Some("Loading..."));
    }

    #[test]
    fn test_loader_with_color() {
        let loader = Loader::new().with_color(Color::Cyan);
        assert_eq!(loader.fg_color, Color::Cyan);
    }

    #[test]
    fn test_loader_set_message() {
        let mut loader = Loader::new();
        loader.set_message("Processing");
        assert_eq!(loader.message.as_deref(), Some("Processing"));
    }

    #[test]
    fn test_loader_tick() {
        let mut loader = Loader::new();
        assert_eq!(loader.frame, 0);
        loader.tick();
        assert_eq!(loader.frame, 1);
        // Tick wraps around
        for _ in 0..SPINNER_FRAMES.len() - 1 {
            loader.tick();
        }
        assert_eq!(loader.frame, 0);
    }

    #[test]
    fn test_loader_tick_does_not_advance_when_cancelled() {
        let mut loader = Loader::new();
        loader.cancel();
        assert_eq!(loader.frame, 0);
        loader.tick();
        assert_eq!(loader.frame, 0); // no advancement when cancelled
    }

    #[test]
    fn test_loader_cancel() {
        let mut loader = Loader::new();
        loader.cancel();
        assert!(loader.is_cancelled());
        assert_eq!(loader.message.as_deref(), Some("Cancelled"));
    }

    #[test]
    fn test_loader_reset() {
        let mut loader = Loader::new();
        loader.tick();
        loader.tick();
        loader.cancel();
        loader.reset();
        assert!(!loader.is_cancelled());
        assert_eq!(loader.frame, 0);
    }

    #[test]
    fn test_loader_set_done() {
        let mut loader = Loader::new();
        loader.set_done("Complete!");
        assert!(loader.is_cancelled());
        assert_eq!(loader.message.as_deref(), Some("Complete!"));
    }

    #[test]
    fn test_loader_default() {
        let loader = Loader::default();
        assert!(loader.message.is_none());
    }

    #[test]
    fn test_loader_name() {
        let loader = Loader::new();
        assert_eq!(loader.name(), "Loader");
    }

    #[test]
    fn test_loader_dirty_flag() {
        let mut loader = Loader::new();
        assert!(loader.is_dirty());
        loader.clear_dirty();
        assert!(!loader.is_dirty());
        loader.request_render();
        assert!(loader.is_dirty());
    }

    #[test]
    fn test_loader_min_size_no_message() {
        let loader = Loader::new();
        let min = loader.min_size();
        assert_eq!(min.width, 3); // spinner + space + no message
        assert_eq!(min.height, 1);
    }

    #[test]
    fn test_loader_min_size_with_message() {
        let loader = Loader::new().with_message("Loading...");
        let min = loader.min_size();
        // spinner (1) + space (1) + "Loading..." (10) = 12, plus extra for spinner + space = 3 + msg_width
        assert_eq!(min.width, 13);
        assert_eq!(min.height, 1);
    }

    #[test]
    fn test_loader_handle_event_unfocused() {
        let mut loader = Loader::new();
        let event = Event::Key(KeyEvent::new(KeyCode::Escape));
        assert!(!loader.handle_event(&event));
    }

    #[test]
    fn test_loader_handle_event_escape() {
        let mut loader = Loader::new();
        loader.on_focus();
        let event = Event::Key(KeyEvent::new(KeyCode::Escape));
        assert!(loader.handle_event(&event));
        assert!(loader.is_cancelled());
    }

    #[test]
    fn test_loader_handle_event_ctrl_c() {
        let mut loader = Loader::new();
        loader.on_focus();
        let event = Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('c'),
            KeyModifiers::new().with_ctrl(),
        ));
        assert!(loader.handle_event(&event));
        assert!(loader.is_cancelled());
    }

    #[test]
    fn test_loader_handle_event_other_key() {
        let mut loader = Loader::new();
        loader.on_focus();
        let event = Event::Key(KeyEvent::new(KeyCode::Char('a')));
        assert!(!loader.handle_event(&event));
    }

    #[test]
    fn test_loader_focus() {
        let mut loader = Loader::new();
        assert!(!loader.is_focused());
        loader.on_focus();
        assert!(loader.is_focused());
        loader.on_unfocus();
        assert!(!loader.is_focused());
    }

    #[test]
    fn test_loader_render_active() {
        let mut loader = Loader::new().with_message("Loading...");
        let mut surface = Surface::new(80, 1);
        let area = Rect::new(0, 0, 80, 1);
        loader.render(&mut surface, area);
        // Should have rendered a spinner char in the first cell
        let cell = surface.get(0, 0).unwrap();
        assert_eq!(cell.char, SPINNER_FRAMES[0]);
        // Should have rendered message chars
        let msg_cell = surface.get(0, 2).unwrap();
        assert_eq!(msg_cell.char, 'L');
    }

    #[test]
    fn test_loader_render_cancelled() {
        let mut loader = Loader::new().with_message("Loading...");
        loader.cancel();
        let mut surface = Surface::new(80, 1);
        let area = Rect::new(0, 0, 80, 1);
        loader.render(&mut surface, area);
        // Should show checkmark
        let cell = surface.get(0, 0).unwrap();
        assert_eq!(cell.char, '✓');
        assert_eq!(cell.fg, Color::Green);
    }

    #[test]
    fn test_loader_render_truncates_message() {
        let long_msg = "A".repeat(200);
        let mut loader = Loader::new().with_message(long_msg);
        let mut surface = Surface::new(20, 1);
        let area = Rect::new(0, 0, 20, 1);
        loader.render(&mut surface, area);
        // Should not panic; message should be truncated to fit area
    }
}
