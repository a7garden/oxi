//! Input widget — text input field with cursor, placeholder, and completion.
//!
//! Supports full Unicode including CJK double-width characters (Korean, Chinese, Japanese).

use ratatui::{
    widgets::StatefulWidget,
    buffer::Buffer,
    layout::Rect,
    style::{Style, Modifier},
};
use crate::Theme;
use unicode_width::UnicodeWidthStr;
use unicode_width::UnicodeWidthChar;

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

    /// Calculate the display width (in terminal columns) of text up to char index.
    fn display_width_up_to(&self, char_idx: usize) -> usize {
        let s: String = self.text.chars().take(char_idx).collect();
        UnicodeWidthStr::width(s.as_str())
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

impl StatefulWidget for Input<'_> {
    type State = InputState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height < 1 || area.width < 4 {
            return;
        }

        let styles = self.theme.to_styles();
        let y = area.y;

        // Prompt (❯ is double-width, takes 2 cells)
        let prompt_width = self.prompt_char.width().unwrap_or(1) as u16;
        buf[(area.x, y)].set_char(self.prompt_char)
            .set_style(styles.primary);
        // Fill continuation cell if prompt is wide
        if prompt_width > 1 {
            buf[(area.x + 1, y)].set_char(' ')
                .set_style(styles.primary);
        }
        let content_start = area.x + prompt_width + 1; // prompt + space
        buf[(area.x + prompt_width, y)].set_char(' ')
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

        // Calculate display widths using Unicode-aware width
        let max_cols = (area.width - prompt_width - 2) as usize; // available column width
        let _text_display_width = UnicodeWidthStr::width(display_text);

        // Calculate cursor column position
        let cursor_col = if state.text.is_empty() {
            0
        } else {
            state.display_width_up_to(state.cursor)
        };

        // Horizontal scrolling: ensure cursor is visible within the viewport
        let scroll_col = if cursor_col >= max_cols {
            // Scroll so cursor is near the right edge
            cursor_col - max_cols + 1
        } else {
            0
        };

        // Render characters using column-based positioning
        let mut col = 0u16; // current column offset from content_start
        let mut char_iter = display_text.chars().enumerate().peekable();
        let mut chars_before_cursor = 0usize;
        let mut cursor_rendered = false;

        // Skip characters that are scrolled off
        let mut skipped_width = 0usize;
        while let Some((char_idx, c)) = char_iter.peek().cloned() {
            let cw = c.width().unwrap_or(0);
            if skipped_width + cw <= scroll_col {
                skipped_width += cw;
                chars_before_cursor = char_idx + 1;
                char_iter.next();
            } else {
                break;
            }
        }

        // Render visible characters
        for (char_idx, c) in char_iter {
            let cw = c.width().unwrap_or(1) as u16;
            let screen_col = content_start + col;

            if screen_col + cw > area.x + area.width - 1 {
                break; // No more room
            }

            let is_cursor = state.cursor == char_idx && !state.text.is_empty();

            if is_cursor {
                buf[(screen_col, y)].set_char(c)
                    .set_style(Style::default()
                        .fg(self.theme.colors.cursor_fg.to_ratatui())
                        .bg(self.theme.colors.cursor_bg.to_ratatui())
                        .add_modifier(Modifier::BOLD));
                // For wide chars, set continuation cell
                if cw > 1 {
                    buf[(screen_col + 1, y)].set_char(' ')
                        .set_style(Style::default()
                            .fg(self.theme.colors.cursor_fg.to_ratatui())
                            .bg(self.theme.colors.cursor_bg.to_ratatui()));
                }
                cursor_rendered = true;
            } else {
                buf[(screen_col, y)].set_char(c).set_style(text_fg);
                // Wide char continuation is handled by ratatui's set_char
            }

            col += cw;
        }

        // Cursor at end of text (empty cursor)
        let end_col = content_start + col;
        if state.cursor >= state.text.chars().count() && end_col < area.x + area.width - 1 {
            let cursor_col_pos = if state.text.is_empty() && self.placeholder.is_some() {
                content_start
            } else {
                end_col
            };
            buf[(cursor_col_pos, y)].set_char(' ')
                .set_style(Style::default()
                    .fg(self.theme.colors.cursor_fg.to_ratatui())
                    .bg(self.theme.colors.cursor_bg.to_ratatui()));
        }

        // Clear remainder
        let clear_from = if state.text.is_empty() {
            let ph_width = UnicodeWidthStr::width(self.placeholder.unwrap_or(""));
            content_start + (ph_width as u16).min(area.width - prompt_width - 2)
        } else {
            end_col + 1
        };
        for c in clear_from..area.x + area.width {
            buf[(c, y)].set_char(' ').set_style(text_fg);
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
    fn input_state_insert_str() {
        let mut state = InputState::default();
        state.insert_str("안녕하세요");
        assert_eq!(state.text, "안녕하세요");
        assert_eq!(state.cursor, 5);
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
    fn input_state_backspace_korean() {
        let mut state = InputState::default();
        state.text = "안녕하세요".to_string();
        state.cursor = 5;
        state.backspace();
        assert_eq!(state.text, "안녕하세");
        assert_eq!(state.cursor, 4);
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

    #[test]
    fn input_state_display_width() {
        let mut state = InputState::default();
        state.text = "ab한글".to_string();
        // a(1) + b(1) + 한(2) + 글(2) = 6 columns
        assert_eq!(state.display_width_up_to(0), 0);
        assert_eq!(state.display_width_up_to(1), 1); // "a"
        assert_eq!(state.display_width_up_to(2), 2); // "ab"
        assert_eq!(state.display_width_up_to(3), 4); // "ab한"
        assert_eq!(state.display_width_up_to(4), 6); // "ab한글"
    }
}
