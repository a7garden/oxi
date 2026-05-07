//! Input widget — text input field with cursor, placeholder, and completion.
//!
//! Supports full Unicode including CJK double-width characters.

use ratatui::{
    widgets::{StatefulWidget, Paragraph},
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use ratatui::widgets::Widget;
use crate::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub display: String,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct InputState {
    pub text: String,
    pub cursor: usize,
    pub placeholder: Option<String>,
    completions: Vec<Completion>,
    completion_index: usize,
    completion_active: bool,
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

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.text.len())
    }

    fn display_width_up_to(&self, char_idx: usize) -> usize {
        let s: String = self.text.chars().take(char_idx).collect();
        UnicodeWidthStr::width(s.as_str())
    }

    pub fn accept_completion(&mut self) -> bool {
        if !self.completion_active || self.completions.is_empty() {
            return false;
        }
        let completion = &self.completions[self.completion_index];
        let chars: Vec<char> = self.text.chars().collect();
        let mut trigger_pos = self.cursor;
        while trigger_pos > 0 {
            match chars.get(trigger_pos - 1).copied() {
                Some(c) if !c.is_whitespace() => trigger_pos -= 1,
                _ => break,
            }
        }
        let prefix: String = chars[..trigger_pos].iter().collect();
        self.text = format!("{}{}", prefix, completion.text);
        self.cursor = self.text.chars().count();
        self.completion_active = false;
        true
    }

    pub fn next_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = (self.completion_index + 1) % self.completions.len();
        }
    }

    pub fn prev_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = self.completion_index.saturating_sub(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

pub struct Input<'a> {
    theme: &'a Theme,
    placeholder: Option<&'a str>,
    prompt_char: char,
}

impl<'a> Input<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme, placeholder: None, prompt_char: '❯' }
    }

    pub fn with_theme(mut self, theme: &'a Theme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

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

        // ── Prompt (manual buf — 2 cells max, not worth Paragraph) ──
        let prompt_width = self.prompt_char.width().unwrap_or(1) as u16;
        buf[(area.x, y)]
            .set_char(self.prompt_char)
            .set_style(styles.primary);
        if prompt_width > 1 {
            buf[(area.x + 1, y)]
                .set_char(' ')
                .set_style(styles.primary);
        }
        let content_start = area.x + prompt_width + 1;
        buf[(content_start - 1, y)]
            .set_char(' ')
            .set_style(styles.normal);

        // ── Display text ──
        let text_fg = if state.text.is_empty() {
            styles.muted
        } else {
            styles.normal
        };

        let max_cols = (area.width - prompt_width - 2) as usize;
        let cursor_col = if state.text.is_empty() {
            0
        } else {
            state.display_width_up_to(state.cursor)
        };
        let scroll_col = if cursor_col >= max_cols {
            cursor_col - max_cols + 1
        } else {
            0
        };

        // ── Build text area ──
        let text_area = Rect {
            x: content_start,
            y,
            width: max_cols as u16,
            height: 1,
        };

        if state.text.is_empty() {
            let display_text = self.placeholder.unwrap_or("");
            let visible: String = display_text.chars().take(max_cols).collect();
            let line = Line::from(Span::styled(visible, text_fg));
            Paragraph::new(line).render(text_area, buf);

            // Empty cursor block at content_start
            buf[(content_start, y)]
                .set_char(' ')
                .set_style(
                    Style::default()
                        .fg(self.theme.colors.cursor_fg.to_ratatui())
                        .bg(self.theme.colors.cursor_bg.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                );
            return;
        }

        // ── Find cursor position in visible string ──
        let total_chars = state.text.chars().count();
        let mut visible_chars: Vec<char> = Vec::new();
        let mut cursor_in_visible: Option<usize> = None;
        let mut cursor_width: u16 = 0;
        let mut col_acc = 0usize;

        // Skip scrolled-off chars
        for (ci, c) in state.text.chars().enumerate() {
            let cw = c.width().unwrap_or(0);
            if col_acc < scroll_col {
                col_acc += cw;
            } else {
                break;
            }
        }

        // Collect visible chars, track cursor
        for ci in col_acc..total_chars {
            let c = state.text.chars().nth(ci).unwrap();
            let cw = c.width().unwrap_or(1);
            let screen_col = col_acc.saturating_sub(scroll_col);

            if screen_col + cw > max_cols {
                break;
            }

            if state.cursor == ci {
                cursor_in_visible = Some(visible_chars.len());
                cursor_width = cw as u16;
            }

            visible_chars.push(c);
            if cw > 1 {
                visible_chars.push('\u{0}');
            }
            col_acc += cw;
        }

        // Cursor at end
        if state.cursor >= total_chars && cursor_in_visible.is_none() {
            let end_screen_col = col_acc.saturating_sub(scroll_col);
            if end_screen_col <= max_cols {
                cursor_in_visible = Some(visible_chars.len());
                cursor_width = 1;
            }
        }

        let visible_str: String = visible_chars.iter().collect();

        // Build Line: pre-cursor + cursor-char + post-cursor
        if let Some(civ) = cursor_in_visible {
            let pre: String = visible_chars[..civ].iter().collect();
            let cursor_ch: String = if civ < visible_chars.len() {
                visible_chars[civ].to_string()
            } else {
                " ".to_string()
            };
            let post: String = if civ + 1 < visible_chars.len() {
                visible_chars[civ + 1..].iter().collect()
            } else {
                String::new()
            };

            let line = Line::from(vec![
                Span::styled(&pre, text_fg),
                Span::styled(&cursor_ch, text_fg),
                Span::styled(&post, text_fg),
            ]);
            Paragraph::new(line).render(text_area, buf);

            // ── Manual buf: cursor highlight (justified: fg/bg invert) ──
            let cursor_style = Style::default()
                .fg(self.theme.colors.cursor_fg.to_ratatui())
                .bg(self.theme.colors.cursor_bg.to_ratatui())
                .add_modifier(Modifier::BOLD);

            // Find screen column of cursor char
            let mut screen_offset = 0usize;
            for i in 0..civ {
                screen_offset += visible_chars[i].width().unwrap_or(1);
            }
            let screen_col = content_start + screen_offset as u16;

            if screen_col < area.x + area.width {
                buf[(screen_col, y)].set_char(cursor_ch.chars().next().unwrap_or(' ')).set_style(cursor_style);
                if cursor_width > 1 && screen_col + 1 < area.x + area.width {
                    buf[(screen_col + 1, y)].set_char(' ').set_style(cursor_style);
                }
            }
        } else {
            let line = Line::from(Span::styled(visible_str, text_fg));
            Paragraph::new(line).render(text_area, buf);
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
        state.insert_char('한');
        assert_eq!(state.text, "ab한");
        assert_eq!(state.cursor, 3);
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
        assert_eq!(state.display_width_up_to(1), 1);
        assert_eq!(state.display_width_up_to(2), 2);
        assert_eq!(state.display_width_up_to(3), 4);
        assert_eq!(state.display_width_up_to(4), 6);
    }
}