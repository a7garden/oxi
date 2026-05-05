//! Editor component - multi-line text editor with autocomplete.
//!
//! Provides a text editing area with support for file path completion
//! via @ mentions and Tab completion for paths.

use crate::autocomplete::FuzzyMatcher;
use crate::cell::Cell;
use crate::component::Component;
use crate::components::{Completion, FileCompleter};
use crate::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crate::surface::Surface;
use crate::undo_stack::UndoStack;
use crate::Rect;
use crate::Size;
use crate::Theme;
use std::path::Path;
use unicode_width::UnicodeWidthChar;

/// Editor content with cursor tracking.
#[derive(Debug, Clone)]
struct Line {
    content: String,
    cursor: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
        }
    }

    fn from(s: &str) -> Self {
        let len = s.len();
        Self {
            content: s.to_string(),
            cursor: len,
        }
    }

    fn insert(&mut self, pos: usize, c: char) {
        if pos <= self.content.len() {
            self.content.insert(pos, c);
            if self.cursor >= pos {
                self.cursor = (self.cursor + c.len_utf8()).min(self.content.len());
            }
        }
    }

    fn remove(&mut self, pos: usize) -> Option<char> {
        if pos < self.content.len() {
            let c = self.content.remove(pos);
            if self.cursor > pos {
                self.cursor = self.cursor.saturating_sub(c.len_utf8());
            }
            Some(c)
        } else {
            None
        }
    }

    fn len(&self) -> usize {
        self.content.len()
    }
}

/// Mention suggestion for @ mentions.
#[derive(Debug, Clone)]
pub struct Mention {
    pub name: String,
    pub path: String,
    pub is_file: bool,
}

/// Editor options.
#[derive(Debug, Clone)]
pub struct EditorOptions {
    /// Prompt text shown before content.
    pub prompt: Option<String>,
    /// Prompt color.
    pub prompt_color: Option<crate::Color>,
    /// Text color.
    pub text_color: Option<crate::Color>,
    /// Background color.
    pub bg_color: Option<crate::Color>,
    /// Enable file path completion.
    pub enable_file_completion: bool,
    /// Enable @ mentions.
    pub enable_mention_completion: bool,
    /// Maximum history lines to keep.
    pub max_history: usize,
    /// Show line numbers.
    pub show_line_numbers: bool,
    /// Theme reference for default colors.
    pub theme: Option<Theme>,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            prompt: None,
            prompt_color: None,
            text_color: None,
            bg_color: None,
            enable_file_completion: true,
            enable_mention_completion: true,
            max_history: 100,
            show_line_numbers: false,
            theme: None,
        }
    }
}

impl EditorOptions {
    /// Create options pre-filled from a theme.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            prompt_color: Some(theme.colors.primary),
            text_color: Some(theme.colors.foreground),
            bg_color: Some(theme.colors.background),
            theme: Some(theme.clone()),
            ..EditorOptions::default()
        }
    }
}

/// A multi-line editor component with autocomplete support.
pub struct Editor {
    lines: Vec<Line>,
    current_line: usize,
    scroll_offset: usize,
    options: EditorOptions,
    focused: bool,
    dirty: bool,
    // Completion state
    file_completer: Option<FileCompleter>,
    mention_candidates: Vec<Mention>,
    completions: Vec<Completion>,
    completion_index: usize,
    completion_active: bool,
    trigger_start: usize,
    mention_matcher: FuzzyMatcher,
    // Undo/redo state
    undo_stack: UndoStack<String>,
}

impl Editor {
    /// Create a new editor.
    pub fn new() -> Self {
        Self {
            lines: vec![Line::new()],
            current_line: 0,
            scroll_offset: 0,
            options: EditorOptions::default(),
            focused: false,
            dirty: true,
            file_completer: None,
            mention_candidates: Vec::new(),
            completions: Vec::new(),
            completion_index: 0,
            completion_active: false,
            trigger_start: 0,
            mention_matcher: FuzzyMatcher::new(),
            undo_stack: UndoStack::new(),
        }
    }

    /// Create with options.
    pub fn with_options(options: EditorOptions) -> Self {
        Self {
            lines: vec![Line::new()],
            current_line: 0,
            scroll_offset: 0,
            options,
            focused: false,
            dirty: true,
            file_completer: None,
            mention_candidates: Vec::new(),
            completions: Vec::new(),
            completion_index: 0,
            completion_active: false,
            trigger_start: 0,
            mention_matcher: FuzzyMatcher::new(),
            undo_stack: UndoStack::new(),
        }
    }

    /// Get current content as a single string.
    pub fn content(&self) -> String {
        self.lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i == self.lines.len() - 1 {
                    l.content.clone()
                } else {
                    format!("{}\n", l.content)
                }
            })
            .collect()
    }

    /// Set content.
    pub fn set_content(&mut self, text: &str) {
        self.lines = text.lines().map(Line::from).collect();
        if self.lines.is_empty() {
            self.lines.push(Line::new());
        }
        self.current_line = self.lines.len() - 1;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// Clear content.
    pub fn clear(&mut self) {
        self.lines = vec![Line::new()];
        self.current_line = 0;
        self.scroll_offset = 0;
        self.clear_completions();
        self.undo_stack.clear();
        self.dirty = true;
    }

    /// Enable file path completion.
    pub fn with_file_completion<P: AsRef<Path>>(mut self, base_dir: P) -> Self {
        self.file_completer = Some(FileCompleter::new(base_dir));
        self.options.enable_file_completion = true;
        self
    }

    /// Set the theme; colors from the theme are used unless explicitly overridden.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.options.theme = Some(theme.clone());
        if self.options.prompt_color.is_none() {
            self.options.prompt_color = Some(theme.colors.primary);
        }
        if self.options.text_color.is_none() {
            self.options.text_color = Some(theme.colors.foreground);
        }
        if self.options.bg_color.is_none() {
            self.options.bg_color = Some(theme.colors.background);
        }
        self.dirty = true;
    }

    /// Add mention candidates (files, users, etc).
    pub fn set_mention_candidates(&mut self, candidates: Vec<Mention>) {
        self.mention_candidates = candidates;
    }

    /// Clear current completions.
    fn clear_completions(&mut self) {
        self.completions.clear();
        self.completion_index = 0;
        self.completion_active = false;
        self.trigger_start = 0;
    }

    /// Get current cursor position for a line.
    fn line_cursor(line: &Line) -> usize {
        line.cursor.min(line.content.len())
    }

    /// Get current line.
    fn current(&self) -> &Line {
        &self.lines[self.current_line]
    }

    /// Get mutable current line.
    fn current_mut(&mut self) -> &mut Line {
        &mut self.lines[self.current_line]
    }

    /// Get current cursor position relative to line start.
    fn cursor(&self) -> usize {
        Self::line_cursor(self.current())
    }

    /// Get full content up to and including cursor.
    #[allow(dead_code)]
    fn content_until_cursor(&self) -> String {
        let mut result = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i == self.current_line {
                result.push_str(&line.content[..Self::line_cursor(line)]);
                break;
            } else {
                result.push('\n');
                result.push_str(&line.content);
            }
        }
        result
    }

    /// Try to trigger completion.
    fn try_trigger_completion(&mut self) {
        if !self.options.enable_file_completion && !self.options.enable_mention_completion {
            return;
        }

        let line_content = self.current().content.clone();
        let cursor_pos = Self::line_cursor(self.current());

        // Check for @ mention trigger
        if self.options.enable_mention_completion {
            if let Some((trigger_pos, pattern)) =
                Self::find_mention_trigger(&line_content, cursor_pos)
            {
                self.trigger_start = trigger_pos;
                let pattern = pattern.to_string();
                self.request_mention_completions(&pattern[1..]); // Remove @ prefix
                return;
            }
        }

        // Check for file path trigger
        if self.options.enable_file_completion {
            if let Some((trigger_pos, pattern)) = Self::find_path_trigger(&line_content, cursor_pos)
            {
                self.trigger_start = trigger_pos;
                let pattern = pattern.to_string();
                self.request_file_completions(&pattern);
                return;
            }
        }

        self.clear_completions();
    }

    /// Find @ mention trigger position and pattern.
    fn find_mention_trigger(line: &str, cursor: usize) -> Option<(usize, &str)> {
        // Only look at content up to cursor
        let line_up_to_cursor = &line[..cursor.min(line.len())];

        // Find the last @ that's not preceded by alphanumeric
        let mut last_at = None;

        for (i, c) in line_up_to_cursor.char_indices().rev() {
            if c == '@' {
                // Check that there's nothing between this @ and cursor
                let after = &line_up_to_cursor[i + 1..];
                if !after.contains(' ') && !after.contains('\n') {
                    last_at = Some(i);
                    break;
                }
            }
            // Stop at whitespace (but allow @ to be at start)
            if c.is_whitespace() && i > 0 {
                break;
            }
        }

        last_at.map(|pos| (pos, &line[pos..cursor.min(line.len())]))
    }

    /// Find path trigger (after /, ~, or ./).
    fn find_path_trigger(line: &str, cursor: usize) -> Option<(usize, &str)> {
        let trigger_chars = ['/', '~'];

        let mut last_trigger_pos = 0;

        for (i, c) in line.char_indices() {
            if i >= cursor {
                break;
            }
            if trigger_chars.contains(&c) {
                last_trigger_pos = i;
            }
        }

        if last_trigger_pos < cursor {
            Some((
                last_trigger_pos,
                &line[last_trigger_pos..cursor.min(line.len())],
            ))
        } else {
            None
        }
    }

    /// Request file completions.
    fn request_file_completions(&mut self, prefix: &str) {
        if let Some(ref completer) = self.file_completer {
            self.completions = completer.completions(prefix);
        } else {
            self.completions.clear();
        }
        self.completion_index = 0;
        self.completion_active = !self.completions.is_empty();
    }

    /// Request mention completions using fuzzy matching.
    fn request_mention_completions(&mut self, pattern: &str) {
        let pattern_lower = pattern.to_lowercase();

        let mut results: Vec<Completion> = self
            .mention_candidates
            .iter()
            .filter_map(|m| {
                let matches = m.name.to_lowercase().starts_with(&pattern_lower)
                    || self.mention_matcher.matches(pattern, &m.name).is_some();

                if matches {
                    Some(Completion {
                        text: format!("@{}", m.name),
                        display: format!(
                            "@{} ({})",
                            m.name,
                            if m.is_file { "file" } else { "user" }
                        ),
                        is_dir: m.is_file,
                        score: 50, // Base score for mention matches
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by score
        results.sort_by(|a, b| b.score.cmp(&a.score));

        self.completions = results;
        self.completion_index = 0;
        self.completion_active = !self.completions.is_empty();
    }

    /// Accept current completion.
    fn accept_completion(&mut self) -> bool {
        if !self.completion_active || self.completions.is_empty() {
            return false;
        }

        let trigger_start = self.trigger_start;
        let cursor = self.cursor();

        // Get the completion text
        let completion_text = self.completions[self.completion_index].text.clone();

        // Replace the trigger range with completion text using byte-range replacement
        let line = self.current_mut();
        line.content.replace_range(trigger_start..cursor, &completion_text);
        line.cursor = trigger_start + completion_text.len();

        self.clear_completions();
        self.dirty = true;
        true
    }

    /// Navigate to next completion.
    fn next_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = (self.completion_index + 1) % self.completions.len();
            self.dirty = true;
        }
    }

    /// Navigate to previous completion.
    fn prev_completion(&mut self) {
        if !self.completions.is_empty() {
            if self.completion_index == 0 {
                self.completion_index = self.completions.len() - 1;
            } else {
                self.completion_index -= 1;
            }
            self.dirty = true;
        }
    }

    /// Get current completions.
    pub fn get_completions(&self) -> &[Completion] {
        &self.completions
    }

    /// Check if completion is active.
    pub fn is_completion_active(&self) -> bool {
        self.completion_active
    }

    /// Get current completion index.
    pub fn completion_index(&self) -> usize {
        self.completion_index
    }

    /// Snapshot current content for undo.
    fn snapshot(&mut self) {
        self.undo_stack.push(self.content());
    }

    /// Undo the last edit.
    fn undo(&mut self) -> bool {
        if let Some(state) = self.undo_stack.undo() {
            let content = state.clone();
            self.restore_state(&content);
            true
        } else {
            false
        }
    }

    /// Redo the last undone edit.
    fn redo(&mut self) -> bool {
        if let Some(state) = self.undo_stack.redo() {
            let content = state.clone();
            self.restore_state(&content);
            true
        } else {
            false
        }
    }

    /// Restore editor to a snapshot state.
    fn restore_state(&mut self, content: &str) {
        // Save current cursor line for best-effort restore
        let prev_line = self.current_line;
        self.set_content(content);
        // Try to restore cursor line
        self.current_line = prev_line.min(self.lines.len().saturating_sub(1));
        self.dirty = true;
    }

    /// Check if undo is available.
    pub fn can_undo(&self) -> bool {
        self.undo_stack.can_undo()
    }

    /// Check if redo is available.
    pub fn can_redo(&self) -> bool {
        self.undo_stack.can_redo()
    }

    /// Insert character at cursor position.
    fn insert_char(&mut self, c: char) {
        let cursor = Self::line_cursor(self.current());
        self.current_mut().insert(cursor, c);
        self.dirty = true;
    }

    /// Delete character before cursor.
    fn delete_back(&mut self) -> bool {
        let cursor = Self::line_cursor(self.current());
        if cursor > 0 {
            // Find byte position of previous char boundary
            let prev_byte = self.current().content[..cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.current_mut().remove(prev_byte);
            self.dirty = true;
            true
        } else if self.current_line > 0 {
            // Merge with previous line
            let prev_len = self.lines[self.current_line - 1].len();
            let current = self.lines.remove(self.current_line);
            self.current_line -= 1;
            self.lines[self.current_line]
                .content
                .push_str(&current.content);
            self.lines[self.current_line].cursor = prev_len;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Delete character at cursor.
    fn delete_forward(&mut self) -> bool {
        let cursor = Self::line_cursor(&self.lines[self.current_line]);

        // Check if we can delete within current line
        if cursor < self.lines[self.current_line].len() {
            self.current_mut().remove(cursor);
            self.dirty = true;
            true
        } else if self.current_line < self.lines.len() - 1 {
            // Merge with next line - need to do this without holding the mutable reference
            let next = self.lines.remove(self.current_line + 1);
            self.current_mut().content.push_str(&next.content);
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Move cursor left by one character.
    fn move_left(&mut self) -> bool {
        let line = self.current_mut();
        if line.cursor > 0 {
            // Find byte offset of previous character
            let prev_byte = line.content[..line.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            line.cursor = prev_byte;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Move cursor right by one character.
    fn move_right(&mut self) -> bool {
        let line = self.current_mut();
        if line.cursor < line.content.len() {
            // Find byte offset of next character
            line.cursor = line.content[line.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| line.cursor + i)
                .unwrap_or(line.content.len());
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Move cursor left by one word.
    fn move_word_left(&mut self) -> bool {
        let line = self.current_mut();
        if line.cursor == 0 {
            return false;
        }

        // Walk backwards: skip non-word chars, then skip word chars
        let s = &line.content[..line.cursor];
        let chars: Vec<(usize, char)> = s.char_indices().collect();

        let mut i = chars.len();
        // Skip trailing non-word chars
        while i > 0 && !is_word_char(chars[i - 1].1) {
            i -= 1;
        }
        // Skip word chars
        while i > 0 && is_word_char(chars[i - 1].1) {
            i -= 1;
        }

        let new_cursor = if i > 0 { chars[i - 1].0 + chars[i - 1].1.len_utf8() } else { 0 };
        // If we're at the same position, try to move before the non-word chars
        if new_cursor == line.cursor && i > 0 {
            // fallback: just move left one char
            let prev_byte = chars.last().map(|(idx, c)| *idx).unwrap_or(0);
            line.cursor = prev_byte;
        } else {
            line.cursor = new_cursor;
        }
        self.dirty = true;
        true
    }

    /// Move cursor right by one word.
    fn move_word_right(&mut self) -> bool {
        let line = self.current_mut();
        if line.cursor >= line.content.len() {
            return false;
        }

        let s = &line.content[line.cursor..];
        let mut offset = 0usize;
        let chars: Vec<(usize, char)> = s.char_indices().collect();
        let mut i = 0;

        // Skip word chars
        while i < chars.len() && is_word_char(chars[i].1) {
            i += 1;
        }
        // Skip non-word chars
        while i < chars.len() && !is_word_char(chars[i].1) {
            i += 1;
        }

        if i < chars.len() {
            offset = chars[i].0;
        } else {
            offset = s.len();
        }

        line.cursor = line.cursor + offset;
        self.dirty = true;
        true
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Editor {
    fn name(&self) -> &str {
        "Editor"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.completion_active
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }

        // Handle completion navigation
        if self.completion_active {
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Tab, ..
                }) => {
                    self.next_completion();
                    return true;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Up,
                    modifiers: KeyModifiers { shift: true, .. },
                    ..
                }) => {
                    self.prev_completion();
                    return true;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Down,
                    modifiers: KeyModifiers { shift: true, .. },
                    ..
                }) => {
                    self.next_completion();
                    return true;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    return self.accept_completion();
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Escape,
                    ..
                }) => {
                    self.clear_completions();
                    self.dirty = true;
                    return true;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Backspace,
                    ..
                })
                | Event::Key(KeyEvent {
                    code: KeyCode::Char(_),
                    ..
                }) => {
                    self.clear_completions();
                    // Continue to normal handling
                }
                _ => {}
            }
        }

        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Char(c) => {
                    if key.modifiers.ctrl {
                        return false;
                    }
                    self.snapshot();
                    self.insert_char(c);
                    self.try_trigger_completion();
                    true
                }
                KeyCode::Backspace => {
                    if !self.delete_back() {
                        return true;
                    }
                    self.snapshot();
                    self.try_trigger_completion();
                    true
                }
                KeyCode::Delete => {
                    self.snapshot();
                    self.delete_forward();
                    true
                }
                KeyCode::Enter => {
                    if self.completion_active {
                        return self.accept_completion();
                    }
                    self.snapshot();
                    // Split line at cursor
                    let line = self.current_mut();
                    let cursor = Self::line_cursor(line);
                    let after = line.content[cursor..].to_string();
                    line.content.truncate(cursor);
                    line.cursor = 0;

                    // Insert new line
                    let new_line = Line::from(&after);
                    self.current_line += 1;
                    self.lines.insert(self.current_line, new_line);

                    self.dirty = true;
                    true
                }
                KeyCode::Left => {
                    if key.modifiers.ctrl {
                        self.move_word_left();
                    } else {
                        self.move_left();
                    }
                    true
                }
                KeyCode::Right => {
                    if key.modifiers.ctrl {
                        self.move_word_right();
                    } else {
                        self.move_right();
                    }
                    true
                }
                KeyCode::Char('z') if key.modifiers.ctrl => {
                    self.undo();
                    true
                }
                KeyCode::Char('y') if key.modifiers.ctrl => {
                    self.redo();
                    true
                }
                KeyCode::Up => {
                    if self.current_line > 0 {
                        self.current_line -= 1;
                        self.ensure_cursor_visible();
                        self.dirty = true;
                    }
                    true
                }
                KeyCode::Down => {
                    if self.current_line < self.lines.len() - 1 {
                        self.current_line += 1;
                        self.ensure_cursor_visible();
                        self.dirty = true;
                    }
                    true
                }
                KeyCode::Home => {
                    self.current_mut().cursor = 0;
                    self.dirty = true;
                    true
                }
                KeyCode::End => {
                    let line = self.current_mut();
                    line.cursor = line.content.len();
                    self.dirty = true;
                    true
                }
                KeyCode::Tab => {
                    if !self.completions.is_empty() {
                        self.accept_completion();
                    } else {
                        self.try_trigger_completion();
                    }
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let max_width = area.width as usize;
        let max_height = area.height as usize;

        // Render prompt if set
        let content_start_col = if let Some(ref prompt) = self.options.prompt {
            let prompt_width = prompt.len().min(max_width);
            for (i, c) in prompt.chars().enumerate() {
                let mut cell = Cell::new(c);
                if let Some(color) = self.options.prompt_color {
                    cell.fg = color;
                }
                surface.set(area.y, area.x + i as u16, cell);
            }
            prompt_width
        } else {
            0
        };

        // Calculate line number width if showing line numbers
        let line_num_width = if self.options.show_line_numbers {
            (self.lines.len().to_string().len() + 2).max(1)
        } else {
            0
        };

        // Render lines
        let visible_lines = max_height.saturating_sub(self.scroll_offset);
        for row in 0..visible_lines.min(self.lines.len()) {
            let line_idx = row + self.scroll_offset;
            let line = &self.lines[line_idx];

            let y = area.y + row as u16;
            let mut x = area.x + content_start_col as u16;

            // Render line number
            if self.options.show_line_numbers {
                let line_num = (line_idx + 1).to_string();
                for (i, c) in line_num.chars().enumerate() {
                    let mut cell = Cell::new(c);
                    cell.fg = crate::Color::Indexed(8); // Gray
                    surface.set(y, x + i as u16, cell);
                }
                x += line_num_width as u16;

                // Separator
                let mut sep = Cell::new(' ');
                sep.fg = crate::Color::Indexed(8);
                surface.set(y, x, sep);
                x += 1;
            }

            // Render line content
            let content = &line.content;
            let cursor_in_line = Self::line_cursor(line);

            for (byte_idx, c) in content.char_indices() {
                if x >= area.x + area.width {
                    break;
                }

                let mut cell = Cell::new(c);
                if let Some(color) = self.options.text_color {
                    cell.fg = color;
                }

                // Highlight cursor position
                if byte_idx == cursor_in_line && self.focused {
                    cell.fg = crate::Color::Indexed(0);
                    cell.bg = crate::Color::Indexed(15);
                }

                let char_width = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
                surface.set(y, x, cell);
                x += char_width;
            }

            // Render cursor if at end of line
            if cursor_in_line == content.len() && self.focused
                && x < area.x + area.width {
                    let mut cell = Cell::new(' ');
                    cell.fg = crate::Color::Indexed(0);
                    cell.bg = crate::Color::Indexed(15);
                    surface.set(y, x, cell);
                    x += 1;
                }

            // Highlight trigger if completion active
            if self.completion_active && line_idx == self.current_line {
                // Could add visual indicator here
            }

            // Clear remainder of line
            while x < area.x + area.width {
                let mut cell = Cell::new(' ');
                if let Some(bg) = self.options.bg_color {
                    cell.bg = bg;
                }
                surface.set(y, x, cell);
                x += 1;
            }
        }

        // Clear remaining rows
        for row in visible_lines.min(self.lines.len())..max_height {
            let y = area.y + row as u16;
            for col in area.x..area.x + area.width {
                let mut cell = Cell::new(' ');
                if let Some(bg) = self.options.bg_color {
                    cell.bg = bg;
                }
                surface.set(y, col, cell);
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 20,
            height: 3,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        Some(Size {
            width: 80,
            height: 10,
        })
    }

    fn on_focus(&mut self) {
        self.focused = true;
        self.dirty = true;
    }

    fn on_unfocus(&mut self) {
        self.focused = false;
        self.clear_completions();
        self.dirty = true;
    }

    fn is_focused(&self) -> bool {
        self.focused
    }
}

/// Check if a character is a word character for word-wise movement.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// Private helper
impl Editor {
    fn ensure_cursor_visible(&mut self) {
        // Clamp cursor to line length and ensure it's on a valid char boundary
        let line = &mut self.lines[self.current_line];
        line.cursor = line.cursor.min(line.content.len());

        // If cursor is not on a char boundary, snap to the nearest one before it
        while !line.content.is_char_boundary(line.cursor) && line.cursor > 0 {
            line.cursor -= 1;
        }

        // Could add scroll logic here if needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_editor_creation() {
        let editor = Editor::new();
        assert_eq!(editor.lines.len(), 1);
        assert_eq!(editor.content(), "");
    }

    #[test]
    fn test_set_content() {
        let mut editor = Editor::new();
        editor.set_content("line1\nline2");
        assert!(editor.content().contains("line1"));
        assert!(editor.content().contains("line2"));
    }

    #[test]
    fn test_clear() {
        let mut editor = Editor::new();
        editor.set_content("test");
        editor.clear();
        assert_eq!(editor.content(), "");
    }

    #[test]
    fn test_mention_candidates() {
        let mut editor = Editor::new();
        let mentions = vec![
            Mention {
                name: "alice".to_string(),
                path: "".to_string(),
                is_file: false,
            },
            Mention {
                name: "bob".to_string(),
                path: "".to_string(),
                is_file: false,
            },
        ];
        editor.set_mention_candidates(mentions);
        // Can't easily test completion without simulating events
    }

    // ===== Unicode / multi-byte character tests =====

    #[test]
    fn test_line_insert_korean() {
        // Korean characters are 3 bytes each in UTF-8
        let mut line = Line::new();
        line.insert(0, '한');
        assert_eq!(line.content, "한");
        assert_eq!(line.cursor, "한".len()); // 3 bytes
        assert_eq!(line.cursor, 3);

        line.insert(line.cursor, '글');
        assert_eq!(line.content, "한글");
        assert_eq!(line.cursor, "한글".len()); // 6 bytes
    }

    #[test]
    fn test_line_insert_emoji() {
        // Emoji are 4 bytes in UTF-8
        let mut line = Line::new();
        line.insert(0, '🎉');
        assert_eq!(line.content, "🎉");
        assert_eq!(line.cursor, 4);

        line.insert(line.cursor, '🚀');
        assert_eq!(line.content, "🎉🚀");
        assert_eq!(line.cursor, 8);
    }

    #[test]
    fn test_line_from_multibyte() {
        let line = Line::from("한글");
        assert_eq!(line.content, "한글");
        assert_eq!(line.cursor, "한글".len()); // 6 bytes
    }

    #[test]
    fn test_line_remove_korean() {
        let mut line = Line::from("한글");
        // Remove the second char (at byte offset 3)
        let c = line.remove(3);
        assert_eq!(c, Some('글'));
        assert_eq!(line.content, "한");
        assert_eq!(line.cursor, 3); // cursor was at end (6), moved back by 3
    }

    #[test]
    fn test_line_remove_emoji() {
        let mut line = Line::from("🎉🚀");
        let c = line.remove(4);
        assert_eq!(c, Some('🚀'));
        assert_eq!(line.content, "🎉");
        assert_eq!(line.cursor, 4); // was at 8, moved back by 4
    }

    #[test]
    fn test_editor_insert_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.insert_char('한');
        editor.insert_char('글');
        assert_eq!(editor.content(), "한글");
        assert_eq!(editor.cursor(), "한글".len());
    }

    #[test]
    fn test_editor_insert_mixed_ascii_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.insert_char('h');
        editor.insert_char('i');
        editor.insert_char('한');
        editor.insert_char('글');
        assert_eq!(editor.content(), "hi한글");
        // cursor should be at end: 2 ("hi") + 6 ("한글") = 8 bytes
        assert_eq!(editor.cursor(), 8);
    }

    #[test]
    fn test_editor_move_left_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("한글");
        // cursor at end (6 bytes)
        assert_eq!(editor.cursor(), 6);

        // Move left past '글' (3 bytes)
        assert!(editor.move_left());
        assert_eq!(editor.cursor(), 3); // before '글'

        // Move left past '한' (3 bytes)
        assert!(editor.move_left());
        assert_eq!(editor.cursor(), 0); // at start

        // Can't move further left
        assert!(!editor.move_left());
    }

    #[test]
    fn test_editor_move_right_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("한글");
        // Move to start
        editor.current_mut().cursor = 0;
        assert_eq!(editor.cursor(), 0);

        // Move right past '한'
        assert!(editor.move_right());
        assert_eq!(editor.cursor(), 3);

        // Move right past '글'
        assert!(editor.move_right());
        assert_eq!(editor.cursor(), 6);

        // Can't move further right
        assert!(!editor.move_right());
    }

    #[test]
    fn test_editor_move_left_emoji() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("🎉🚀");
        assert_eq!(editor.cursor(), 8);

        assert!(editor.move_left());
        assert_eq!(editor.cursor(), 4); // before '🚀'

        assert!(editor.move_left());
        assert_eq!(editor.cursor(), 0); // before '🎉'
    }

    #[test]
    fn test_editor_delete_back_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("한글");
        assert_eq!(editor.cursor(), 6);

        // Delete '글' (backspace from end)
        assert!(editor.delete_back());
        assert_eq!(editor.content(), "한");
        assert_eq!(editor.cursor(), 3);

        // Delete '한'
        assert!(editor.delete_back());
        assert_eq!(editor.content(), "");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn test_editor_delete_back_mixed() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("a한b");
        // byte lengths: 'a'=1, '한'=3, 'b'=1 => total 5
        assert_eq!(editor.cursor(), 5);

        // Delete 'b'
        assert!(editor.delete_back());
        assert_eq!(editor.content(), "a한");
        assert_eq!(editor.cursor(), 4);

        // Delete '한' (3 bytes)
        assert!(editor.delete_back());
        assert_eq!(editor.content(), "a");
        assert_eq!(editor.cursor(), 1);

        // Delete 'a'
        assert!(editor.delete_back());
        assert_eq!(editor.content(), "");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn test_editor_delete_forward_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("한글");
        // Move cursor to start
        editor.current_mut().cursor = 0;

        // Delete '한' at cursor
        assert!(editor.delete_forward());
        assert_eq!(editor.content(), "글");
        assert_eq!(editor.cursor(), 0);

        // Delete '글'
        assert!(editor.delete_forward());
        assert_eq!(editor.content(), "");
    }

    #[test]
    fn test_editor_split_line_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("한글");
        // Move cursor to between '한' and '글' (byte offset 3)
        editor.current_mut().cursor = 3;

        // Simulate Enter key
        let line = editor.current_mut();
        let cursor = Editor::line_cursor(line);
        let after = line.content[cursor..].to_string();
        line.content.truncate(cursor);
        line.cursor = 0;
        let new_line = Line::from(&after);
        editor.current_line += 1;
        editor.lines.insert(editor.current_line, new_line);

        assert_eq!(editor.lines.len(), 2);
        assert_eq!(editor.lines[0].content, "한");
        assert_eq!(editor.lines[1].content, "글");
    }

    #[test]
    fn test_editor_home_end_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("한글");

        // Home
        editor.current_mut().cursor = 0;
        assert_eq!(editor.cursor(), 0);

        // End
        let line = editor.current_mut();
        line.cursor = line.content.len();
        assert_eq!(editor.cursor(), 6);
    }

    #[test]
    fn test_content_until_cursor_korean() {
        let mut editor = Editor::new();
        editor.set_content("한글");
        // Move cursor to between the two chars
        editor.current_mut().cursor = 3;
        assert_eq!(editor.content_until_cursor(), "한");
    }

    #[test]
    fn test_editor_roundtrip_korean() {
        let mut editor = Editor::new();
        editor.on_focus();
        // Insert Korean chars one by one
        for c in "안녕하세요".chars() {
            editor.insert_char(c);
        }
        assert_eq!(editor.content(), "안녕하세요");

        // Move to start
        editor.current_mut().cursor = 0;
        // Move right through all chars
        let char_count = "안녕하세요".chars().count();
        for _ in 0..char_count {
            assert!(editor.move_right());
        }
        assert_eq!(editor.cursor(), "안녕하세요".len());

        // Delete all chars from end
        for _ in 0..char_count {
            assert!(editor.delete_back());
        }
        assert_eq!(editor.content(), "");
    }

    // ===== Undo/Redo tests =====

    #[test]
    fn test_undo_basic() {
        let mut editor = Editor::new();
        editor.on_focus();
        assert!(!editor.can_undo());
        assert!(!editor.can_redo());

        // Simulate typing via event handler
        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('a'))));
        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('b'))));
        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('c'))));
        assert_eq!(editor.content(), "abc");

        // Undo
        let did_undo = editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('z'),
            KeyModifiers::new().with_ctrl(),
        )));
        assert!(did_undo);
        assert!(editor.can_redo());
        // After undo, content should revert to previous snapshot
    }

    #[test]
    fn test_undo_redo_cycle() {
        let mut editor = Editor::new();
        editor.on_focus();

        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('h'))));
        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('i'))));
        assert_eq!(editor.content(), "hi");

        // Undo twice
        editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('z'),
            KeyModifiers::new().with_ctrl(),
        )));
        editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('z'),
            KeyModifiers::new().with_ctrl(),
        )));

        // Redo twice
        editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('y'),
            KeyModifiers::new().with_ctrl(),
        )));
        editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('y'),
            KeyModifiers::new().with_ctrl(),
        )));
        assert_eq!(editor.content(), "hi");
    }

    #[test]
    fn test_undo_on_empty_editor() {
        let mut editor = Editor::new();
        editor.on_focus();
        let result = editor.undo();
        assert!(!result);
    }

    #[test]
    fn test_redo_on_empty_editor() {
        let mut editor = Editor::new();
        editor.on_focus();
        let result = editor.redo();
        assert!(!result);
    }

    #[test]
    fn test_undo_after_backspace() {
        let mut editor = Editor::new();
        editor.on_focus();

        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('a'))));
        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('b'))));
        editor.handle_event(&Event::Key(KeyEvent::new(KeyCode::Backspace)));
        assert_eq!(editor.content(), "a");

        // Undo should restore "ab"
        editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Char('z'),
            KeyModifiers::new().with_ctrl(),
        )));
        assert!(editor.can_redo());
    }

    // ===== Word-wise movement tests =====

    #[test]
    fn test_move_word_left_basic() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("hello world");
        // cursor at end
        assert_eq!(editor.cursor(), 11);

        editor.move_word_left();
        // Should skip "world" and land at start of "world"
        assert_eq!(editor.cursor(), 6); // at 'w'

        editor.move_word_left();
        // Should skip space and land at start of "hello"
        assert_eq!(editor.cursor(), 0); // at 'h'
    }

    #[test]
    fn test_move_word_right_basic() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("hello world");
        // Move cursor to start
        editor.current_mut().cursor = 0;

        editor.move_word_right();
        // Should skip "hello" and space, land at start of "world"
        assert_eq!(editor.cursor(), 6);

        editor.move_word_right();
        // Should skip "world" to end
        assert_eq!(editor.cursor(), 11);
    }

    #[test]
    fn test_move_word_left_at_start() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("hello");
        editor.current_mut().cursor = 0;
        assert!(!editor.move_word_left());
    }

    #[test]
    fn test_move_word_right_at_end() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("hello");
        assert!(!editor.move_word_right());
    }

    #[test]
    fn test_ctrl_left_right_events() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("foo bar baz");
        // cursor at end

        // Ctrl+Left
        let handled = editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Left,
            KeyModifiers::new().with_ctrl(),
        )));
        assert!(handled);
        assert_eq!(editor.cursor(), 8); // start of "baz"

        // Ctrl+Right
        let handled = editor.handle_event(&Event::Key(KeyEvent::with_modifiers(
            KeyCode::Right,
            KeyModifiers::new().with_ctrl(),
        )));
        assert!(handled);
        assert_eq!(editor.cursor(), 11); // end
    }

    #[test]
    fn test_move_word_with_underscores() {
        let mut editor = Editor::new();
        editor.on_focus();
        editor.set_content("foo_bar baz");
        // cursor at end
        assert_eq!(editor.cursor(), 11);

        editor.move_word_left();
        // Should skip "baz" to start of "baz"
        assert_eq!(editor.cursor(), 8);

        editor.move_word_left();
        // Should skip space and "foo_bar" (underscores are word chars)
        assert_eq!(editor.cursor(), 0);
    }
}
