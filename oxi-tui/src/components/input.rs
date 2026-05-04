//! Input component - text input field with autocomplete support.

use crate::components::{Completion, FileCompleter};
use crate::{
    Cell, Color, Component, Event, KeyCode, KeyEvent, KeyModifiers, Rect, Size, Surface, Theme,
};
use std::path::PathBuf;

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
    /// Enable file path autocomplete.
    pub enable_file_completion: bool,
    /// Enable @ mention completion.
    pub enable_mention_completion: bool,
    /// Theme reference for default colors.
    pub theme: Option<Theme>,
}

impl Default for InputOptions {
    fn default() -> Self {
        Self {
            placeholder: None,
            fg_color: None,
            bg_color: None,
            max_length: None,
            enable_file_completion: true,
            enable_mention_completion: true,
            theme: None,
        }
    }
}

impl InputOptions {
    /// Create options pre-filled from a theme.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            fg_color: Some(theme.colors.foreground),
            bg_color: Some(theme.colors.background),
            theme: Some(theme.clone()),
            ..InputOptions::default()
        }
    }
}

/// Autocomplete provider trait.
pub trait AutocompleteProvider: Send {
    /// Get completions for the given context.
    fn completions(&mut self, context: &str, cursor_pos: usize) -> Vec<Completion>;
}

/// An input field with autocomplete support.
pub struct Input {
    value: String,
    placeholder: String,
    cursor_pos: usize,
    options: InputOptions,
    focused: bool,
    dirty: bool,
    // Autocomplete state
    completer: Option<FileCompleter>,
    mention_completer: Option<Box<dyn AutocompleteProvider>>,
    completions: Vec<Completion>,
    completion_index: usize,
    completion_active: bool,
    trigger_char: Option<char>,
}

impl Input {
    /// Create a new input field.
    pub fn new() -> Self {
        Self {
            value: String::new(),
            placeholder: String::from(""),
            cursor_pos: 0,
            options: InputOptions::default(),
            focused: false,
            dirty: true,
            completer: None,
            mention_completer: None,
            completions: Vec::new(),
            completion_index: 0,
            completion_active: false,
            trigger_char: None,
        }
    }

    /// Create with placeholder text.
    pub fn with_placeholder(placeholder: &str) -> Self {
        Self {
            value: String::new(),
            placeholder: placeholder.to_string(),
            cursor_pos: 0,
            options: InputOptions::default(),
            focused: false,
            dirty: true,
            completer: None,
            mention_completer: None,
            completions: Vec::new(),
            completion_index: 0,
            completion_active: false,
            trigger_char: None,
        }
    }

    /// Set the theme; colors from the theme are used unless explicitly overridden.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.options.theme = Some(theme.clone());
        if self.options.fg_color.is_none() {
            self.options.fg_color = Some(theme.colors.foreground);
        }
        if self.options.bg_color.is_none() {
            self.options.bg_color = Some(theme.colors.background);
        }
        self.dirty = true;
    }

    /// Get current value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Set the value.
    pub fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
        let char_count = self.value.chars().count();
        self.cursor_pos = char_count.min(self.cursor_pos);
        self.dirty = true;
    }

    /// Clear the input.
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor_pos = 0;
        self.clear_completions();
        self.dirty = true;
    }

    /// Set options.
    pub fn with_options(mut self, options: InputOptions) -> Self {
        self.options = options;
        self
    }

    /// Enable file path completion with a base directory.
    pub fn with_file_completion(
        mut self,
        base_dir: impl Into<PathBuf> + AsRef<std::path::Path>,
    ) -> Self {
        self.completer = Some(FileCompleter::new(base_dir.as_ref()));
        self.options.enable_file_completion = true;
        self
    }

    /// Set a custom autocomplete provider for mentions.
    pub fn with_mention_provider(mut self, provider: impl AutocompleteProvider + 'static) -> Self {
        self.mention_completer = Some(Box::new(provider));
        self.options.enable_mention_completion = true;
        self
    }

    /// Convert char index to byte index for safe String slicing.
    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    /// Get byte length of char at the given char index.
    fn char_len_at(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(_, c)| c.len_utf8())
            .unwrap_or(0)
    }

    /// Clear the current completions.
    fn clear_completions(&mut self) {
        self.completions.clear();
        self.completion_index = 0;
        self.completion_active = false;
        self.trigger_char = None;
    }

    /// Try to trigger completion based on current cursor context.
    fn try_trigger_completion(&mut self) {
        if !self.options.enable_file_completion && !self.options.enable_mention_completion {
            return;
        }

        // Find the trigger character context (last @ or / from cursor position going backwards)
        let (trigger_pos, trigger_char) = match self.find_trigger() {
            Some((pos, ch)) => (pos, ch),
            None => return,
        };

        let prefix = self.value[self.char_to_byte(trigger_pos)..self.char_to_byte(self.cursor_pos)].to_string();

        match trigger_char {
            '@' if self.options.enable_mention_completion => {
                self.trigger_char = Some('@');
                self.request_completions(&prefix, trigger_char);
            }
            '/' | '~' | '.' if self.options.enable_file_completion => {
                self.trigger_char = Some(trigger_char);
                self.request_completions(&prefix, trigger_char);
            }
            _ => {}
        }
    }

    /// Find the trigger character before cursor (searches backwards).
    fn find_trigger(&self) -> Option<(usize, char)> {
        let chars: Vec<(usize, char)> = self.value.chars().enumerate().collect();

        // Look for the last @ or / before cursor
        let mut trigger_pos = None;
        let mut trigger_char = None;

        for (i, c) in chars.iter().take(self.cursor_pos).rev() {
            if c == &'@' || c == &'/' || c == &'~' || c == &'.' {
                // Make sure there's nothing after the trigger (within our prefix)
                if let Some(next) = chars.get(i + 1) {
                    if next.0 < self.cursor_pos && next.1 != ' ' && next.1 != '/' {
                        continue; // Not a valid trigger position
                    }
                }
                trigger_pos = Some(*i);
                trigger_char = Some(*c);
                break;
            }
            // Stop at whitespace
            if c.is_whitespace() {
                break;
            }
        }

        trigger_pos.zip(trigger_char)
    }

    /// Request completions from the appropriate provider.
    fn request_completions(&mut self, prefix: &str, trigger: char) {
        match trigger {
            '@' => {
                if let Some(ref mut provider) = self.mention_completer {
                    self.completions = provider.completions(prefix, self.cursor_pos);
                } else {
                    // Default: no completions for @ without a provider
                    self.completions.clear();
                }
            }
            '/' | '~' | '.' => {
                if let Some(ref completer) = self.completer {
                    self.completions = completer.completions(prefix);
                }
            }
            _ => {}
        }

        self.completion_index = 0;
        self.completion_active = !self.completions.is_empty();
    }

    /// Accept the current completion.
    fn accept_completion(&mut self) -> bool {
        if !self.completion_active || self.completions.is_empty() {
            return false;
        }

        let completion = &self.completions[self.completion_index];

        // Find the start position of the prefix being completed
        let (trigger_pos, _) = self.find_trigger().unwrap_or((0, '/'));

        // Replace the prefix with the completion (char-index aware)
        let prefix_start = self.char_to_byte(trigger_pos);
        let cursor_byte = self.char_to_byte(self.cursor_pos);
        let suffix = self.value[cursor_byte..].to_string();

        self.value = format!(
            "{}{}{}",
            &self.value[..prefix_start],
            completion.text.clone(),
            suffix
        );

        // Position cursor after the completed text
        let new_cursor = trigger_pos + completion.text.chars().count();
        self.cursor_pos = new_cursor.min(self.value.chars().count());

        self.clear_completions();
        self.dirty = true;
        true
    }

    /// Move to next completion.
    fn next_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = (self.completion_index + 1) % self.completions.len();
            self.dirty = true;
        }
    }

    /// Move to previous completion.
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

    /// Get current completions for external display (e.g., popup).
    pub fn get_completions(&self) -> &[Completion] {
        &self.completions
    }

    /// Get current completion index.
    pub fn completion_index(&self) -> usize {
        self.completion_index
    }

    /// Check if completion is active.
    pub fn is_completion_active(&self) -> bool {
        self.completion_active
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
                }) => {
                    self.clear_completions();
                    // Let the normal backspace handling occur
                }
                _ => {
                    // Any other key clears completions
                    self.clear_completions();
                }
            }
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
                    // Insert character at cursor (byte-index safe)
                    self.value.insert(self.char_to_byte(self.cursor_pos), c);
                    self.cursor_pos += 1;
                    self.dirty = true;
                    // Try to trigger completion
                    self.try_trigger_completion();
                    true
                }
                KeyCode::Backspace => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        let byte_pos = self.char_to_byte(self.cursor_pos);
                        self.value.remove(byte_pos);
                        self.dirty = true;
                        self.try_trigger_completion();
                    }
                    true
                }
                KeyCode::Delete => {
                    if self.cursor_pos < self.value.len() {
                        let byte_pos = self.char_to_byte(self.cursor_pos);
                        self.value.remove(byte_pos);
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
                    self.cursor_pos = self.value.chars().count();
                    self.dirty = true;
                    true
                }
                KeyCode::Tab => {
                    // Try to complete if we have completions, otherwise trigger
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
        // Determine text and foreground color
        let fg = if self.value.is_empty() {
            // Placeholder - dim color
            Color::Indexed(8) // dark gray
        } else {
            // Actual text - use theme foreground or bright white
            self.options.fg_color.unwrap_or(Color::Indexed(252))
        };

        let display = if self.value.is_empty() {
            &self.placeholder
        } else {
            &self.value
        };

        // Calculate visible portion (char-index aware)
        let max_width = area.width as usize;
        let start_offset = if self.cursor_pos >= max_width {
            self.cursor_pos - max_width + 1
        } else {
            0
        };

        let start_byte = display
            .char_indices()
            .nth(start_offset)
            .map(|(i, _)| i)
            .unwrap_or(display.len());
        let end_byte = display
            .char_indices()
            .nth(start_offset + max_width)
            .map(|(i, _)| i)
            .unwrap_or(display.len());
        let visible = &display[start_byte..end_byte];

        // Render text
        let mut x = area.x;
        for c in visible.chars() {
            let mut cell = Cell::new(c);
            cell.fg = fg;
            if let Some(bg) = self.options.bg_color {
                cell.bg = bg;
            }
            surface.set(area.y, x, cell);
            x += 1;
        }

        // Render cursor if focused
        if self.focused {
            let cursor_offset = (self.cursor_pos - start_offset) as u16;
            let cursor_col = area.x + cursor_offset;
            if cursor_col < area.x + area.width {
                // Block cursor - swap fg/bg
                if let Some(cell) = surface.get_mut(area.y, cursor_col) {
                    cell.fg = Color::Indexed(0); // Black text
                    cell.bg = Color::Indexed(255); // Bright white background
                    cell.attrs.bold = true;
                }
            } else if self.cursor_pos >= start_offset + max_width {
                // Cursor is past the visible area - show on the last column
                let last_col = area.x + area.width - 1;
                if let Some(cell) = surface.get_mut(area.y, last_col) {
                    cell.fg = Color::Indexed(0);
                    cell.bg = Color::Indexed(255);
                    cell.attrs.bold = true;
                }
            }
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
        self.clear_completions();
        self.dirty = true;
    }
}
