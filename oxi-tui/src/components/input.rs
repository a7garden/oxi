//! Input component for single-line text input with cursor support.

use std::string::String;

/// A single-line text input component with cursor positioning and keyboard handling.
pub struct Input {
    /// The current input buffer content.
    buffer: String,
    /// The cursor position within the buffer (0 = before first char).
    cursor: usize,
    /// Maximum length of the input (0 = unlimited).
    max_length: usize,
    /// Placeholder text shown when empty.
    placeholder: String,
    /// Whether the input is focused/active.
    focused: bool,
}

impl Input {
    /// Creates a new Input component.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            max_length: 0,
            placeholder: String::new(),
            focused: false,
        }
    }

    /// Sets the initial value of the input.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.buffer = value.into();
        self.cursor = self.buffer.len();
        self
    }

    /// Sets the maximum length of the input.
    pub fn max_length(mut self, max: usize) -> Self {
        self.max_length = max;
        self
    }

    /// Sets the placeholder text.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Sets whether the input is focused.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Gets the current buffer content.
    pub fn get_value(&self) -> &str {
        &self.buffer
    }

    /// Gets the cursor position.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Gets whether the input is focused.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Inserts a character at the current cursor position.
    pub fn insert_char(&mut self, c: char) {
        if self.max_length > 0 && self.buffer.len() >= self.max_length {
            return;
        }
        self.buffer.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Inserts a string at the current cursor position.
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if self.max_length > 0 && self.buffer.len() >= self.max_length {
                return;
            }
            self.insert_char(c);
        }
    }

    /// Deletes the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
    }

    /// Deletes the character after the cursor (delete).
    pub fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    /// Moves the cursor one position to the left.
    pub fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Moves the cursor one position to the right.
    pub fn move_cursor_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
    }

    /// Moves the cursor to the start of the line.
    pub fn move_cursor_to_start(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end of the line.
    pub fn move_cursor_to_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    /// Clears the input buffer.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Handles a key event.
    /// Returns Some(String) if Enter was pressed with the submitted value.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        match key {
            KeyEvent::Char(c) => {
                self.insert_char(c);
                None
            }
            KeyEvent::Backspace => {
                self.backspace();
                None
            }
            KeyEvent::Delete => {
                self.delete();
                None
            }
            KeyEvent::Left => {
                self.move_cursor_left();
                None
            }
            KeyEvent::Right => {
                self.move_cursor_right();
                None
            }
            KeyEvent::Home => {
                self.move_cursor_to_start();
                None
            }
            KeyEvent::End => {
                self.move_cursor_to_end();
                None
            }
            KeyEvent::Enter => {
                let value = self.buffer.clone();
                self.clear();
                Some(value)
            }
            KeyEvent::Ctrl('a') => {
                self.move_cursor_to_start();
                None
            }
            KeyEvent::Ctrl('e') => {
                self.move_cursor_to_end();
                None
            }
            KeyEvent::Ctrl('k') => {
                // Kill to end of line
                let remaining = self.buffer.len() - self.cursor;
                for _ in 0..remaining {
                    self.buffer.pop();
                }
                None
            }
            KeyEvent::Ctrl('u') => {
                // Kill to start of line
                self.buffer.drain(0..self.cursor);
                self.cursor = 0;
                None
            }
            _ => None,
        }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

/// A key event for input handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// A printable character.
    Char(char),
    /// Backspace key.
    Backspace,
    /// Delete key.
    Delete,
    /// Left arrow key.
    Left,
    /// Right arrow key.
    Right,
    /// Up arrow key.
    Up,
    /// Down arrow key.
    Down,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Enter key.
    Enter,
    /// Escape key.
    Escape,
    /// Tab key.
    Tab,
    /// Ctrl + key combination.
    Ctrl(char),
    /// Unknown key.
    Unknown,
}

/// Renders the input component to a single line with cursor.
pub trait Render {
    fn render(&self, width: usize) -> Vec<String>;
}

impl Render for Input {
    fn render(&self, width: usize) -> Vec<String> {
        use unicode_width::UnicodeWidthStr;

        let display_text = if self.buffer.is_empty() {
            &self.placeholder
        } else {
            &self.buffer
        };

        // Calculate visual width of text before cursor
        let text_before_cursor = self.buffer[..self.cursor.min(self.buffer.len())].to_string();
        let cursor_offset = UnicodeWidthStr::width(text_before_cursor.as_str());

        let placeholder_colored = if self.buffer.is_empty() {
            format!("{}{}{}", &self.placeholder, " ".repeat(width.saturating_sub(UnicodeWidthStr::width(self.placeholder.as_str()))), " ".repeat(width))
        } else {
            format!("{}{}", display_text, " ".repeat(width.saturating_sub(UnicodeWidthStr::width(display_text.as_str()))))
        };

        let line = if self.focused {
            // Show cursor
            format!(
                "{}{}{}",
                &placeholder_colored[..cursor_offset.min(width)],
                " ",
                &placeholder_colored[(cursor_offset + 1).min(width)..]
            )
        } else {
            placeholder_colored
        };

        vec![line]
    }
}
