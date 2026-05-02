//! Input component - text input field.

use crate::{Cell, Color, Component, Event, KeyCode, Rect, Surface, Size};

/// Input field configuration.
#[derive(Debug, Clone)]
pub struct InputOptions {
    /// Placeholder text.
    pub placeholder: Option<String>,
    /// Text color when not focused.
    pub fg_color: Option<Color>,
    /// Background color.
    pub bg_color: Option<Color>,
    /// Maximum input length.
    pub max_length: Option<usize>,
}

impl Default for InputOptions {
    fn default() -> Self {
        Self {
            placeholder: None,
            fg_color: None,
            bg_color: None,
            max_length: None,
        }
    }
}

/// A text input component.
pub struct Input {
    value: String,
    placeholder: String,
    cursor_pos: usize,
    options: InputOptions,
    focused: bool,
    dirty: bool,
}

impl Input {
    pub fn new() -> Self {
        Self {
            value: String::new(),
            placeholder: String::from(""),
            cursor_pos: 0,
            options: InputOptions::default(),
            focused: false,
            dirty: true,
        }
    }

    pub fn with_placeholder(placeholder: &str) -> Self {
        Self {
            value: String::new(),
            placeholder: placeholder.to_string(),
            cursor_pos: 0,
            options: InputOptions::default(),
            focused: false,
            dirty: true,
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
        self.cursor_pos = self.value.len().min(self.cursor_pos);
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor_pos = 0;
        self.dirty = true;
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Input {
    fn name(&self) -> &str {
        "Input"
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

        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Char(c) => {
                    // Check max length
                    if let Some(max) = self.options.max_length {
                        if self.value.len() >= max {
                            return true;
                        }
                    }
                    // Insert character at cursor
                    self.value.insert(self.cursor_pos, c);
                    self.cursor_pos += 1;
                    self.dirty = true;
                    true
                }
                KeyCode::Backspace => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        self.value.remove(self.cursor_pos);
                        self.dirty = true;
                    }
                    true
                }
                KeyCode::Delete => {
                    if self.cursor_pos < self.value.len() {
                        self.value.remove(self.cursor_pos);
                        self.dirty = true;
                    }
                    true
                }
                KeyCode::Left => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        self.dirty = true;
                    }
                    true
                }
                KeyCode::Right => {
                    if self.cursor_pos < self.value.len() {
                        self.cursor_pos += 1;
                        self.dirty = true;
                    }
                    true
                }
                KeyCode::Home => {
                    self.cursor_pos = 0;
                    self.dirty = true;
                    true
                }
                KeyCode::End => {
                    self.cursor_pos = self.value.len();
                    self.dirty = true;
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        // Get display text (placeholder or value)
        let display = if self.value.is_empty() {
            &self.placeholder
        } else {
            &self.value
        };

        // Calculate visible portion
        let max_width = area.width as usize;
        let start_offset = if self.cursor_pos >= max_width {
            self.cursor_pos - max_width + 1
        } else {
            0
        };

        let visible = &display[start_offset..display.len().min(start_offset + max_width)];

        // Render text
        let mut x = area.x;
        for c in visible.chars() {
            let mut cell = Cell::new(c);
            if let Some(fg) = self.options.fg_color {
                cell.fg = fg;
            }
            surface.set(area.y, x, cell);
            x += 1;
        }

        // Render cursor if focused
        if self.focused && area.x + ((self.cursor_pos - start_offset) as u16) < area.x + area.width {
            let cursor_col = area.x + (self.cursor_pos - start_offset) as u16;
            let mut cursor_cell = surface.get(area.y, cursor_col).cloned().unwrap_or_default();
            cursor_cell.fg = Color::Indexed(0); // Black on white
            cursor_cell.bg = Color::Indexed(15);
            surface.set(area.y, cursor_col, cursor_cell);
        }

        // Clear remainder of area
        for col in x..area.x + area.width {
            let mut cell = Cell::new(' ');
            if let Some(bg) = self.options.bg_color {
                cell.bg = bg;
            }
            surface.set(area.y, col, cell);
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 10, // Minimum input width
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
}