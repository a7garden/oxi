//! Input widget — single-line text input with cursor, placeholder, and scrolling.
//!
//! Supports full Unicode including CJK double-width characters.
//! Uses ASCII-safe prompt character '>' instead of Unicode arrows.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, StatefulWidget, Widget},
};
use crate::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct InputState {
    pub text: String,
    pub cursor: usize,
    pub placeholder: Option<String>,
}

impl InputState {
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self.char_to_byte(self.cursor);
        self.text.insert(byte_pos, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        let byte_pos = self.char_to_byte(self.cursor);
        self.text.insert_str(byte_pos, s);
        self.cursor += s.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let byte_pos = self.char_to_byte(self.cursor);
            self.text.remove(byte_pos);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        let max = self.text.chars().count();
        self.cursor = (self.cursor + 1).min(max);
    }

    pub fn move_home(&mut self) { self.cursor = 0; }
    pub fn move_end(&mut self) { self.cursor = self.text.chars().count(); }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.text.len())
    }

    fn display_width_up_to(&self, char_idx: usize) -> usize {
        let s: String = self.text.chars().take(char_idx).collect();
        UnicodeWidthStr::width(s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

pub struct Input<'a> {
    theme: &'a Theme,
    placeholder: Option<&'a str>,
}

impl<'a> Input<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme, placeholder: None }
    }

    pub fn with_placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }
}

impl StatefulWidget for Input<'_> {
    type State = InputState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height < 1 || area.width < 4 { return; }

        let styles = self.theme.to_styles();
        let y = area.y;

        // Prompt: ">" (1 cell, always safe)
        let _prompt_str = ">";
        buf[(area.x, y)].set_char('>').set_style(styles.primary);
        buf[(area.x + 1, y)].set_char(' ').set_style(styles.normal);

        let content_start = area.x + 2;
        let max_cols = (area.width as usize).saturating_sub(3);

        let text_fg = if state.text.is_empty() { styles.muted } else { styles.normal };

        let text_area = Rect {
            x: content_start,
            y,
            width: max_cols as u16,
            height: 1,
        };

        if state.text.is_empty() {
            // Show placeholder
            let display_text = self.placeholder.unwrap_or("");
            let visible: String = display_text.chars().take(max_cols).collect();
            Paragraph::new(Line::from(Span::styled(visible, text_fg))).render(text_area, buf);

            // Cursor at start
            buf[(content_start, y)]
                .set_char(' ')
                .set_style(Style::default()
                    .fg(self.theme.colors.cursor_fg.to_ratatui())
                    .bg(self.theme.colors.cursor_bg.to_ratatui())
                    .add_modifier(Modifier::BOLD));
            return;
        }

        // Calculate scroll offset for cursor visibility
        let cursor_col = state.display_width_up_to(state.cursor);
        let scroll_col = if cursor_col >= max_cols {
            cursor_col - max_cols + 1
        } else { 0 };

        // Build visible portion
        let chars: Vec<char> = state.text.chars().collect();
        let total_chars = chars.len();
        let char_widths: Vec<usize> = chars.iter().map(|c| c.width().unwrap_or(1)).collect();
        let mut prefix_width = vec![0usize; total_chars + 1];
        for i in 0..total_chars {
            prefix_width[i + 1] = prefix_width[i] + char_widths[i];
        }

        // Find start char index
        let mut start_ci = 0;
        for i in 0..total_chars {
            if prefix_width[i + 1] > scroll_col {
                start_ci = i;
                break;
            }
            start_ci = i + 1;
        }

        let mut visible_str = String::new();
        let mut cursor_screen_col: Option<usize> = None;
        let mut cursor_char = ' ';
        let mut cursor_w: u16 = 1;

        for ci in start_ci..total_chars {
            let cw = char_widths[ci];
            let disp_col = prefix_width[ci].saturating_sub(scroll_col);
            if disp_col + cw > max_cols { break; }

            if state.cursor == ci {
                cursor_screen_col = Some(disp_col);
                cursor_char = chars[ci];
                cursor_w = cw as u16;
            }
            visible_str.push(chars[ci]);
        }

        // Cursor at end
        if state.cursor >= total_chars {
            let end_col = prefix_width[total_chars].saturating_sub(scroll_col);
            if end_col <= max_cols + 1 {
                cursor_screen_col = Some(end_col.min(max_cols));
                cursor_char = ' ';
                cursor_w = 1;
            }
        }

        // Render text
        Paragraph::new(Line::from(Span::styled(&visible_str, text_fg))).render(text_area, buf);

        // Draw cursor
        if let Some(col) = cursor_screen_col {
            let cursor_style = Style::default()
                .fg(self.theme.colors.cursor_fg.to_ratatui())
                .bg(self.theme.colors.cursor_bg.to_ratatui())
                .add_modifier(Modifier::BOLD);

            let screen_x = content_start + col as u16;
            if screen_x < area.x + area.width {
                buf[(screen_x, y)].set_char(cursor_char).set_style(cursor_style);
                if cursor_w > 1 && screen_x + 1 < area.x + area.width {
                    buf[(screen_x + 1, y)].set_char(' ').set_style(cursor_style);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        state.insert_char('\u{d55c}'); // 한
        assert_eq!(state.text, "ab\u{d55c}");
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn input_state_insert_str() {
        let mut state = InputState::default();
        state.insert_str("\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}"); // 안녕하세요
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn input_state_backspace() {
        let mut state = InputState::default();
        state.text = "ab\u{d55c}".to_string(); // ab한
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

    #[test]
    fn input_state_display_width() {
        let mut state = InputState::default();
        state.text = "ab\u{d55c}\u{ae00}".to_string(); // ab한글
        assert_eq!(state.display_width_up_to(0), 0);
        assert_eq!(state.display_width_up_to(1), 1);
        assert_eq!(state.display_width_up_to(2), 2);
        assert_eq!(state.display_width_up_to(3), 4);
        assert_eq!(state.display_width_up_to(4), 6);
    }
}
