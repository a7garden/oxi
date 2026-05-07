//! Input widget — text input field with cursor, placeholder, and completion.

use ratatui::{
    widgets::StatefulWidget,
    buffer::Buffer,
    layout::Rect,
    style::{Style, Modifier},
};
use crate::Theme;

/// Completion entry.
#[derive(Debug, Clone)]
pub struct Completion {
    /// Completion text to insert.
    pub text: String,
    /// Display text for the popup.
    pub display: String,
}

/// State for the Input widget.
#[derive(Debug, Default)]
pub struct InputState {
    /// Current input text.
    pub text: String,
    /// Cursor position (character index).
    pub cursor: usize,
    /// Placeholder shown when input is empty.
    pub placeholder: Option<String>,
    completions: Vec<Completion>,
    completion_index: usize,
    completion_active: bool,
}

impl InputState {
    /// Clear the input.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Insert a character at cursor.
    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self.char_to_byte(self.cursor);
        self.text.insert(byte_pos, c);
        self.cursor += 1;
    }

    /// Insert a string at cursor (for IME composition and paste).
    pub fn insert_str(&mut self, s: &str) {
        let byte_pos = self.char_to_byte(self.cursor);
        self.text.insert_str(byte_pos, s);
        self.cursor += s.chars().count();
    }

    /// Delete character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    /// Delete character after cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let max = self.text.chars().count();
        self.cursor = (self.cursor + 1).min(max);
    }

    /// Move to line start.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move to line end.
    pub fn move_end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.text.len())
    }

    /// Accept current completion if active.
    pub fn accept_completion(&mut self) -> bool {
        if !self.completion_active || self.completions.is_empty() {
            return false;
        }
        let completion = &self.completions[self.completion_index];
        // Find trigger position (first non-space char going backward from cursor)
        let chars: Vec<char> = self.text.chars().collect();
        let mut trigger_pos = self.cursor;
        while trigger_pos > 0 {
            match chars.get(trigger_pos - 1).copied() {
                Some(c) if !c.is_whitespace() => trigger_pos -= 1,
                _ => break,
            }
        }
        // Reconstruct: prefix + completion
        let prefix: String = chars[..trigger_pos].iter().collect();
        self.text = format!("{}{}", prefix, completion.text);
        self.cursor = self.text.chars().count();
        self.completion_active = false;
        true
    }

    /// Move to next completion.
    pub fn next_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = (self.completion_index + 1) % self.completions.len();
        }
    }

    /// Move to previous completion.
    pub fn prev_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = self.completion_index.saturating_sub(1);
        }
    }
}

/// Input widget.
pub struct Input<'a> {
    theme: &'a Theme,
    placeholder: Option<&'a str>,
    prompt_char: char,
}

impl<'a> Input<'a> {
    /// Create a new Input widget with the given theme.
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            placeholder: None,
            prompt_char: '❯',
        }
    }

    /// Replace the theme.
    pub fn with_theme(mut self, theme: &'a Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Set the placeholder text.
    pub fn with_placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    /// Set the prompt character.
    pub fn with_prompt_char(mut self, c: char) -> Self {
        self.prompt_char = c;
        self
    }
}

// Note: No Default impl because Input requires a Theme reference.

impl StatefulWidget for Input<'_> {
    type State = InputState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height < 1 || area.width < 4 {
            return;
        }

        let styles = self.theme.to_styles();
        let y = area.y;

        // Prompt
        buf[(area.x, y)].set_char(self.prompt_char)
            .set_style(styles.primary);
        buf[(area.x + 1, y)].set_char(' ')
            .set_style(styles.normal);

        // Determine what to display
        let display_text = if state.text.is_empty() {
            self.placeholder.unwrap_or("")
        } else {
            &state.text
        };

        let text_fg = if state.text.is_empty() {
            styles.muted
        } else {
            styles.normal
        };

        // Horizontal scrolling
        let content_start = area.x + 2;
        let max_visible = (area.width - 3) as usize;
        let display_len = display_text.chars().count();

        let scroll = if state.cursor >= max_visible {
            state.cursor - max_visible + 1
        } else {
            0
        };

        // Write visible characters
        let visible: String = display_text.chars().skip(scroll).take(max_visible).collect();
        for (i, c) in visible.chars().enumerate() {
            let col = content_start + i as u16;
            if col < area.x + area.width - 1 {
                // Check if cursor
                let char_idx = scroll + i;
                if state.cursor == char_idx && !state.text.is_empty() {
                    buf[(col, y)].set_char(c)
                        .set_style(Style::default()
                            .fg(self.theme.colors.cursor_fg.to_ratatui())
                            .bg(self.theme.colors.cursor_bg.to_ratatui())
                            .add_modifier(Modifier::BOLD));
                } else {
                    buf[(col, y)].set_char(c).set_style(text_fg);
                }
            }
        }

        // Cursor at end of text
        let cursor_screen_pos = if state.cursor >= display_len {
            content_start + visible.len() as u16
        } else {
            content_start + (state.cursor - scroll) as u16
        };

        if state.cursor >= display_len && cursor_screen_pos < area.x + area.width - 1 {
            let cursor_col = if state.text.is_empty() && self.placeholder.is_some() {
                content_start
            } else {
                cursor_screen_pos
            };
            buf[(cursor_col, y)].set_char(' ')
                .set_style(Style::default()
                    .fg(self.theme.colors.cursor_fg.to_ratatui())
                    .bg(self.theme.colors.cursor_bg.to_ratatui()));
        }

        // Clear remainder
        let clear_from = if state.text.is_empty() {
            content_start + (self.placeholder.unwrap_or("").len() as u16).min(area.width - 3)
        } else {
            cursor_screen_pos + 1
        };
        for col in clear_from..area.x + area.width {
            buf[(col, y)].set_char(' ').set_style(text_fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_state_empty() {
        let state = InputState::default();
        assert!(state.text.is_empty());
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn input_state_insert() {
        let mut state = InputState::default();
        state.insert_char('a');
        assert_eq!(state.text, "a");
        state.insert_char('b');
        assert_eq!(state.text, "ab");
        state.insert_char('한');
        assert_eq!(state.text, "ab한");
        assert_eq!(state.cursor, 3); // a + b + 한 = 3 chars
    }

    #[test]
    fn input_state_backspace() {
        let mut state = InputState::default();
        state.text = "ab한".to_string();
        state.cursor = 3;
        state.backspace();
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn input_state_cursor_movement() {
        let mut state = InputState::default();
        state.text = "hello".to_string();
        state.cursor = 5;
        state.move_left();
        assert_eq!(state.cursor, 4);
        state.move_right();
        assert_eq!(state.cursor, 5);
        state.move_home();
        assert_eq!(state.cursor, 0);
        state.move_end();
        assert_eq!(state.cursor, 5);
    }
}