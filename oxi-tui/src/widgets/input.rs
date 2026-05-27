//! Input widget — multi-line text input with cursor, placeholder, and scrolling.
//!
//! Built on `ratatui-textarea` for:
//! - Full Unicode including CJK double-width characters
//! - Emacs-like shortcuts (Ctrl+Left/Right for word movement, Ctrl+A/E)
//! - Undo/Redo support (Ctrl+Z / Ctrl+Shift+Z)
//! - Better IME handling via bracketed paste mode
//!
//! Behavior:
//! - Enter submits text
//! - Shift+Enter inserts newline (multiline mode)

use crate::Theme;
use ratatui::prelude::*;
use ratatui_textarea::{Input as TextAreaInput, Key, TextArea};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Input state wrapping ratatui-textarea's TextArea.
///
/// This provides all textarea features:
/// - Insert/delete characters, word-by-word deletion (Ctrl+Backspace)
/// - Cursor movement (Left/Right, Ctrl+Left/Right, Home/End)
/// - Undo/Redo (Ctrl+Z / Ctrl+Shift+Z)
/// - Selection support (Shift+Arrow)
/// - Multi-line text with automatic line wrapping
/// - Shift+Enter inserts newline
#[derive(Debug)]
pub struct InputState {
    /// The textarea holds all state (text, cursor, history, etc.)
    textarea: TextArea<'static>,
}

impl Default for InputState {
    fn default() -> Self {
        let mut textarea = TextArea::default();
        // No visual line numbers for input
        textarea.remove_line_number();
        // Disable cursor line highlight for cleaner look
        textarea.set_cursor_line_style(Style::default());
        Self { textarea }
    }
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current text content
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Set text directly (replaces all content)
    pub fn set_text(&mut self, text: String) {
        self.textarea.clear();
        if !text.is_empty() {
            self.textarea.insert_str(&text);
        }
    }

    /// Get lines as a vector
    pub fn lines(&self) -> Vec<String> {
        self.textarea.lines().to_vec()
    }

    pub fn clear(&mut self) {
        self.textarea.clear();
    }

    /// Set placeholder text
    pub fn set_placeholder(&mut self, placeholder: Option<String>) {
        if let Some(p) = placeholder {
            self.textarea.set_placeholder_text(&p);
        } else {
            self.textarea.set_placeholder_text("");
        }
    }

    // ── Backward-compatible API (used by oxi-cli handlers) ──

    /// Insert a character at cursor position
    pub fn insert_char(&mut self, c: char) {
        self.handle_char(c);
    }

    /// Insert a string at cursor position
    pub fn insert_str(&mut self, s: &str) {
        self.textarea.insert_str(s);
    }

    /// Delete character before cursor (Backspace)
    pub fn backspace(&mut self) {
        self.textarea.input(TextAreaInput {
            key: Key::Backspace,
            ..Default::default()
        });
    }

    /// Delete character after cursor (Delete)
    pub fn delete(&mut self) {
        self.textarea.input(TextAreaInput {
            key: Key::Delete,
            ..Default::default()
        });
    }

    /// Move cursor left
    pub fn move_left(&mut self) {
        self.textarea
            .move_cursor(ratatui_textarea::CursorMove::Back);
    }

    /// Move cursor right
    pub fn move_right(&mut self) {
        self.textarea
            .move_cursor(ratatui_textarea::CursorMove::Forward);
    }

    /// Move cursor to start of line
    pub fn move_home(&mut self) {
        self.textarea
            .move_cursor(ratatui_textarea::CursorMove::Head);
    }

    /// Move cursor to end of line
    pub fn move_end(&mut self) {
        self.textarea.move_cursor(ratatui_textarea::CursorMove::End);
    }

    /// Move cursor by word (Ctrl+Left/Right)
    pub fn move_word_left(&mut self) {
        self.textarea
            .move_cursor(ratatui_textarea::CursorMove::WordBack);
    }

    /// Move cursor by word (Ctrl+Left/Right)
    pub fn move_word_right(&mut self) {
        self.textarea
            .move_cursor(ratatui_textarea::CursorMove::WordForward);
    }

    // ── Input handling ──

    /// Handle a key event, returning true if it was consumed (Enter/Tab).
    pub fn handle_key(&mut self, key: Key) -> bool {
        match key {
            Key::Enter => true, // Enter is reserved for submit
            Key::Tab => true,   // Tab is used for slash completion
            _ => {
                self.textarea.input(TextAreaInput {
                    key,
                    ..Default::default()
                });
                false
            }
        }
    }

    /// Handle a char input event.
    pub fn handle_char(&mut self, c: char) {
        self.textarea.input(TextAreaInput {
            key: Key::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
        });
    }

    /// Handle a full Input event.
    /// Returns true if Enter pressed (should submit).
    pub fn handle_input(&mut self, input: TextAreaInput) -> bool {
        if input.key == Key::Enter && !input.shift {
            true // Enter without shift = submit
        } else {
            self.textarea.input(input);
            false
        }
    }

    /// Get mutable access to the underlying textarea
    pub fn textarea_mut(&mut self) -> &mut TextArea<'static> {
        &mut self.textarea
    }

    /// Undo last change
    pub fn undo(&mut self) {
        self.textarea.undo();
    }

    /// Redo last undone change
    pub fn redo(&mut self) {
        self.textarea.redo();
    }

    /// Delete from cursor to the start of the line (Ctrl+U).
    pub fn delete_to_line_start(&mut self) {
        self.textarea.delete_line_by_head();
    }

    /// Delete from cursor to the end of the line (Ctrl+K).
    pub fn delete_to_line_end(&mut self) {
        self.textarea.delete_line_by_end();
    }

    /// Delete word before cursor (Ctrl+W / Ctrl+Backspace).
    pub fn delete_word_backward(&mut self) {
        self.textarea.delete_word();
    }

    /// Delete word after cursor (Ctrl+Delete).
    pub fn delete_word_forward(&mut self) {
        self.textarea.delete_next_word();
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// Input widget for the main prompt.
///
/// This widget wraps the textarea and adds a prompt character ("> ").
/// The textarea is rendered as a StatefulWidget using TextArea::widget().
pub struct Input<'a> {
    theme: &'a Theme,
    placeholder: Option<&'a str>,
}

impl<'a> Input<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            placeholder: None,
        }
    }

    pub fn with_placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }
}

impl ratatui::widgets::StatefulWidget for Input<'_> {
    type State = InputState;

    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer, state: &mut Self::State) {
        if area.height < 1 || area.width < 4 {
            return;
        }

        let y = area.y;

        // Configure the textarea with oxi styling
        let textarea = state.textarea_mut();
        textarea.set_style(Style::default().fg(self.theme.colors.foreground.to_ratatui()));
        textarea.set_cursor_style(
            Style::default()
                .fg(self.theme.colors.cursor_fg.to_ratatui())
                .bg(self.theme.colors.cursor_bg.to_ratatui()),
        );
        textarea.set_cursor_line_style(Style::default());
        textarea.remove_line_number();

        // Placeholder style — text is managed by InputState::set_placeholder()
        textarea.set_placeholder_style(Style::default().fg(self.theme.colors.muted.to_ratatui()));

        // Render the textarea widget.
        // No prompt symbol is shown (the `> ` prefix was removed),
        // so use minimal 1-char horizontal padding to maximize input width.
        let content_area = Rect {
            x: area.x + 1,
            y,
            width: area.width.saturating_sub(2), // 1 left + 1 right padding
            height: area.height,
        };

        // Clone textarea for rendering (TextArea implements Clone)
        let textarea_clone = textarea.clone();
        textarea_clone.render(content_area, buf);
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
        assert!(state.text().is_empty());
    }

    #[test]
    fn input_state_insert() {
        let mut state = InputState::default();
        state.handle_char('a');
        assert_eq!(state.text(), "a");
        state.handle_char('b');
        assert_eq!(state.text(), "ab");
        state.handle_char('\u{0041}'); // 'A'
        assert_eq!(state.text(), "abA");
    }

    #[test]
    fn input_state_insert_str() {
        let mut state = InputState::default();
        state.insert_str("HelloWorld");
        assert_eq!(state.text(), "HelloWorld");
    }

    #[test]
    fn input_state_multiline() {
        let mut state = InputState::default();
        state.handle_char('a');
        state.handle_input(TextAreaInput {
            key: Key::Enter,
            shift: true, // Shift+Enter = newline
            ..Default::default()
        });
        state.handle_char('b');
        assert_eq!(state.text(), "a\nb");
    }

    #[test]
    fn input_state_clear() {
        let mut state = InputState::default();
        state.insert_str("hello");
        state.clear();
        assert!(state.text().is_empty());
    }

    #[test]
    fn input_state_undo_redo() {
        let mut state = InputState::default();
        state.insert_str("hello");
        assert_eq!(state.text(), "hello");
        state.undo();
        assert_eq!(state.text(), "");
        state.redo();
        assert_eq!(state.text(), "hello");
    }
}
