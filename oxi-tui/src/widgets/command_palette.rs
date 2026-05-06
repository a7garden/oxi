//! CommandPalette widget — fuzzy-filterable command palette overlay.
//!
//! A centered overlay with a text input and scrollable, fuzzy-filtered command
//! list. Built as a ratatui `StatefulWidget` with separate `CommandPaletteState`
//! for mutable interaction state.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::StatefulWidget,
};
use crate::{Event, KeyCode, KeyEvent, Theme};
use crate::fuzzy::fuzzy_match;

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// A command entry in the palette.
#[derive(Debug, Clone)]
pub struct Command {
    /// Human-readable command name (also used for fuzzy matching).
    pub name: String,
    /// Optional keyboard shortcut shown right-aligned.
    pub shortcut: Option<String>,
    /// Optional category label shown in dimmed text.
    pub category: Option<String>,
}

impl Command {
    /// Create a command with just a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            shortcut: None,
            category: None,
        }
    }

    /// Attach a keyboard shortcut.
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Attach a category label.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Mutable interaction state for the [`CommandPalette`] widget.
#[derive(Debug)]
pub struct CommandPaletteState {
    /// Full list of commands.
    commands: Vec<Command>,
    /// Indices into `commands` after filtering, sorted by fuzzy score (best first).
    filtered_indices: Vec<usize>,
    /// Current search query.
    query: String,
    /// Index into `filtered_indices` for the currently selected item.
    selected: usize,
    /// Vertical scroll offset for the visible list window.
    scroll_offset: usize,
    /// Whether the palette is visible on screen.
    visible: bool,
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            filtered_indices: Vec::new(),
            query: String::new(),
            selected: 0,
            scroll_offset: 0,
            visible: false,
        }
    }
}

impl CommandPaletteState {
    /// Create state pre-populated with a command list.
    pub fn new(commands: Vec<Command>) -> Self {
        let filtered_indices = (0..commands.len()).collect();
        Self {
            commands,
            filtered_indices,
            ..Self::default()
        }
    }

    /// Show the palette (resets query and selection).
    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.apply_filter();
    }

    /// Hide the palette.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Whether the palette is currently visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Replace the full command list.
    pub fn set_commands(&mut self, commands: Vec<Command>) {
        self.commands = commands;
        self.apply_filter();
    }

    /// Apply the current query as a fuzzy filter over commands.
    ///
    /// Results are sorted by descending match score.
    pub fn apply_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.commands.len()).collect();
        } else {
            let mut scored: Vec<(usize, f64)> = self
                .commands
                .iter()
                .enumerate()
                .filter_map(|(i, cmd)| {
                    fuzzy_match(&self.query, &cmd.name).map(|r| (i, r.score))
                })
                .collect();
            scored.sort_by(|a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
            self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
    }

    /// Handle a key event. Returns `true` if the event was consumed.
    pub fn handle_key(&mut self, event: &Event) -> bool {
        if !self.visible {
            return false;
        }

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Escape,
                ..
            }) => {
                self.hide();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Up,
                ..
            }) => {
                self.select_prev();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) => {
                self.select_next();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                // Confirmation is handled by the caller reading `selected_command()`.
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                ..
            }) => {
                self.query.push(*c);
                self.apply_filter();
                self.selected = 0;
                self.scroll_offset = 0;
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                self.query.pop();
                self.apply_filter();
                self.selected = 0;
                self.scroll_offset = 0;
                true
            }
            _ => false,
        }
    }

    /// Get the currently selected command, if any.
    pub fn selected_command(&self) -> Option<&Command> {
        let &idx = self.filtered_indices.get(self.selected)?;
        self.commands.get(idx)
    }

    /// Get the currently selected command index in the original commands list.
    pub fn selected_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Number of items currently shown (after filtering).
    pub fn filtered_count(&self) -> usize {
        self.filtered_indices.len()
    }

    // -- internal helpers --

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll();
        }
    }

    fn select_next(&mut self) {
        if !self.filtered_indices.is_empty() && self.selected < self.filtered_indices.len() - 1 {
            self.selected += 1;
            self.clamp_scroll();
        }
    }

    /// Adjust scroll offset so the selected item is always visible.
    fn clamp_scroll(&mut self) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        // max_visible is checked during render; for state we just ensure a
        // reasonable bound. The render method will further clamp.
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// Command palette overlay widget.
///
/// Renders as a centered modal with rounded borders, an input prompt at the
/// top, and a scrollable fuzzy-filtered command list below.
pub struct CommandPalette<'a> {
    theme: &'a Theme,
    /// Maximum number of visible command rows (default 8).
    max_visible: u16,
}

impl<'a> CommandPalette<'a> {
    /// Create a new palette referencing the given theme.
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            max_visible: 8,
        }
    }

    /// Set the maximum number of visible command rows.
    pub fn with_max_visible(mut self, n: u16) -> Self {
        self.max_visible = n.max(1);
        self
    }
}

impl StatefulWidget for CommandPalette<'_> {
    type State = CommandPaletteState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if !state.visible {
            return;
        }

        let _styles = self.theme.to_styles();

        // -- colour setup --
        let fg = self.theme.colors.foreground.to_ratatui();
        let primary = self.theme.colors.primary.to_ratatui();
        let muted = self.theme.colors.muted.to_ratatui();
        let border_color = self.theme.colors.border.to_ratatui();
        let overlay_bg = Color::Rgb(30, 30, 44); // dark overlay bg

        // -- dimensions --
        let palette_w = area.width.min(60);
        let list_height = self.max_visible;
        // height: border-top + input-row + separator + list + border-bottom
        let total_height = 1u16 + 1u16 + 1u16 + list_height + 1u16;
        let x = area.x + (area.width.saturating_sub(palette_w)) / 2;
        let y = area.y + 2; // slight offset from top

        // -- dim backdrop --
        let backdrop = Style::default().bg(Color::Rgb(20, 20, 30));
        for row in area.y..area.y + area.height {
            for col in area.x..area.x + area.width {
                buf[(col, row)].set_style(backdrop);
            }
        }

        // Helper: check a coordinate is within the buffer.
        let in_bounds = |col: u16, row: u16| -> bool {
            col < area.x + area.width && row < area.y + area.height
        };

        let bottom_y = y + total_height - 1;

        // -- rounded border --
        let border_style = Style::default().fg(border_color).bg(overlay_bg);
        // corners
        if in_bounds(x, y) {
            buf[(x, y)].set_char('┌').set_style(border_style);
        }
        if in_bounds(x + palette_w - 1, y) {
            buf[(x + palette_w - 1, y)].set_char('┐').set_style(border_style);
        }
        if in_bounds(x, bottom_y) {
            buf[(x, bottom_y)].set_char('└').set_style(border_style);
        }
        if in_bounds(x + palette_w - 1, bottom_y) {
            buf[(x + palette_w - 1, bottom_y)].set_char('┘').set_style(border_style);
        }
        // top & bottom edges
        for col in (x + 1)..(x + palette_w - 1) {
            if in_bounds(col, y) {
                buf[(col, y)].set_char('─').set_style(border_style);
            }
            if in_bounds(col, bottom_y) {
                buf[(col, bottom_y)].set_char('─').set_style(border_style);
            }
        }
        // left & right edges
        for row in (y + 1)..bottom_y {
            if in_bounds(x, row) {
                buf[(x, row)].set_char('│').set_style(border_style);
            }
            if in_bounds(x + palette_w - 1, row) {
                buf[(x + palette_w - 1, row)].set_char('│').set_style(border_style);
            }
        }

        // -- separator after input row --
        let sep_y = y + 2; // border-top(1) + input(1) = separator row
        for col in (x + 1)..(x + palette_w - 1) {
            if in_bounds(col, sep_y) {
                buf[(col, sep_y)].set_char('─').set_style(border_style);
            }
        }

        // -- fill interior background --
        let inner_bg = Style::default().bg(overlay_bg);
        for row in (y + 1)..bottom_y {
            for col in (x + 1)..(x + palette_w - 1) {
                if in_bounds(col, row) {
                    buf[(col, row)].set_char(' ').set_style(inner_bg);
                }
            }
        }

        // -- input row --
        let input_y = y + 1;
        let inner_x = x + 1;
        let inner_w = (palette_w as usize).saturating_sub(2);

        // "> " prompt
        let prompt = "> ";
        let prompt_chars: Vec<char> = prompt.chars().collect();
        for (i, &c) in prompt_chars.iter().enumerate() {
            let col = inner_x + i as u16;
            if (i as usize) < inner_w && in_bounds(col, input_y) {
                buf[(col, input_y)].set_char(c)
                    .set_style(Style::default().fg(primary).bg(overlay_bg));
            }
        }

        // query text
        let prompt_len = prompt_chars.len();
        for (i, c) in state.query.chars().enumerate() {
            let col = inner_x + prompt_len as u16 + i as u16;
            if (prompt_len + i) < inner_w && in_bounds(col, input_y) {
                buf[(col, input_y)].set_char(c)
                    .set_style(Style::default().fg(fg).bg(overlay_bg));
            }
        }

        // cursor block
        let cursor_col = inner_x + prompt_len as u16 + state.query.len() as u16;
        if (prompt_len + state.query.len()) < inner_w && in_bounds(cursor_col, input_y) {
            buf[(cursor_col, input_y)].set_char(' ')
                .set_style(Style::default().fg(Color::Black).bg(primary));
        }

        // -- command list --
        let list_start_y = sep_y + 1;
        let visible_count = list_height as usize;

        // Adjust scroll offset so selected stays in view.
        if state.selected < state.scroll_offset {
            state.scroll_offset = state.selected;
        }
        if state.selected >= state.scroll_offset + visible_count {
            state.scroll_offset = state.selected - visible_count + 1;
        }

        if state.filtered_indices.is_empty() {
            // "No matches" message
            let msg = "No matches";
            for (i, c) in msg.chars().enumerate() {
                let col = inner_x + i as u16;
                if i < inner_w && in_bounds(col, list_start_y) {
                    buf[(col, list_start_y)].set_char(c)
                        .set_style(Style::default().fg(muted).bg(overlay_bg));
                }
            }
            return;
        }

        let start = state.scroll_offset;
        let end = (start + visible_count).min(state.filtered_indices.len());

        for vi in start..end {
            let row = list_start_y + (vi - start) as u16;
            if !in_bounds(inner_x, row) {
                break;
            }
            let cmd_idx = state.filtered_indices[vi];
            let cmd = &state.commands[cmd_idx];
            let is_selected = vi == state.selected;

            let (item_fg, item_bg) = if is_selected {
                (Color::Black, primary)
            } else {
                (fg, overlay_bg)
            };
            let item_style = Style::default().fg(item_fg).bg(item_bg);

            // Category dimmed prefix
            let mut col_offset: usize = 0;
            if let Some(ref cat) = cmd.category {
                let cat_str = format!("{}: ", cat);
                for (i, c) in cat_str.chars().enumerate() {
                    let col = inner_x + i as u16;
                    if col_offset + i < inner_w && in_bounds(col, row) {
                        buf[(col, row)].set_char(c)
                            .set_style(if is_selected {
                                Style::default().fg(Color::Black).bg(primary)
                            } else {
                                Style::default().fg(muted).bg(overlay_bg)
                            });
                    }
                }
                col_offset = cat_str.chars().count();
            }

            // Command name
            let max_name = inner_w.saturating_sub(12); // reserve space for shortcut
            let name_str: String = cmd.name.chars().take(max_name).collect();
            for (i, c) in name_str.chars().enumerate() {
                let col = inner_x + (col_offset + i) as u16;
                if col_offset + i < inner_w && in_bounds(col, row) {
                    buf[(col, row)].set_char(c).set_style(item_style);
                }
            }
            col_offset += name_str.chars().count();

            // Clear remainder to end of shortcut area
            for i in col_offset..inner_w.saturating_sub(0) {
                let col = inner_x + i as u16;
                if in_bounds(col, row) {
                    buf[(col, row)].set_char(' ').set_style(item_style);
                }
            }

            // Shortcut (right-aligned)
            if let Some(ref shortcut) = cmd.shortcut {
                let sc_len = shortcut.chars().count();
                let sc_start = inner_w.saturating_sub(sc_len + 1); // 1 padding
                for (i, c) in shortcut.chars().enumerate() {
                    let col = inner_x + (sc_start + i) as u16;
                    if sc_start + i < inner_w && in_bounds(col, row) {
                        buf[(col, row)].set_char(c)
                            .set_style(if is_selected {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(primary)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(muted).bg(overlay_bg)
                            });
                    }
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

    /// Helper: build a small test theme.
    fn test_theme() -> Theme {
        Theme::dark()
    }

    // -- Fuzzy filter tests --

    #[test]
    fn fuzzy_filter_empty_query_shows_all() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open File"),
            Command::new("Save"),
            Command::new("Quit"),
        ]);
        state.apply_filter();
        assert_eq!(state.filtered_count(), 3);
    }

    #[test]
    fn fuzzy_filter_matches_subsequence() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open File"),
            Command::new("Save"),
            Command::new("Quit"),
        ]);
        state.query = "of".to_string();
        state.apply_filter();
        // "of" should fuzzy-match "Open File" (o..F..)
        assert!(state.filtered_count() >= 1);
        let cmd = state.selected_command().unwrap();
        assert_eq!(cmd.name, "Open File");
    }

    #[test]
    fn fuzzy_filter_no_results() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open File"),
            Command::new("Save"),
        ]);
        state.query = "zzz".to_string();
        state.apply_filter();
        assert_eq!(state.filtered_count(), 0);
        assert!(state.selected_command().is_none());
    }

    #[test]
    fn fuzzy_filter_case_insensitive() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open File"),
        ]);
        state.query = "OPEN".to_string();
        state.apply_filter();
        assert_eq!(state.filtered_count(), 1);
    }

    // -- Selection navigation tests --

    #[test]
    fn selection_starts_at_zero() {
        let state = CommandPaletteState::new(vec![
            Command::new("A"),
            Command::new("B"),
        ]);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn selection_navigate_down() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("A"),
            Command::new("B"),
            Command::new("C"),
        ]);
        state.select_next();
        assert_eq!(state.selected, 1);
        state.select_next();
        assert_eq!(state.selected, 2);
        state.select_next(); // should stay at last
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn selection_navigate_up() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("A"),
            Command::new("B"),
        ]);
        state.selected = 1;
        state.select_prev();
        assert_eq!(state.selected, 0);
        state.select_prev(); // should stay at 0
        assert_eq!(state.selected, 0);
    }

    // -- Visibility toggle tests --

    #[test]
    fn visibility_toggle() {
        let mut state = CommandPaletteState::new(vec![]);
        assert!(!state.is_visible());
        state.show();
        assert!(state.is_visible());
        state.hide();
        assert!(!state.is_visible());
    }

    #[test]
    fn show_resets_query_and_selection() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("A"),
            Command::new("B"),
        ]);
        state.query = "test".to_string();
        state.selected = 1;
        state.show();
        assert!(state.query.is_empty());
        assert_eq!(state.selected, 0);
    }

    // -- set_commands tests --

    #[test]
    fn set_commands_replaces_list() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Old"),
        ]);
        state.set_commands(vec![
            Command::new("New A"),
            Command::new("New B"),
        ]);
        assert_eq!(state.filtered_count(), 2);
    }

    // -- handle_key tests --

    #[test]
    fn handle_key_escape_hides() {
        let mut state = CommandPaletteState::new(vec![]);
        state.show();
        assert!(state.is_visible());
        let consumed = state.handle_key(&Event::Key(KeyEvent {
            code: KeyCode::Escape,
            modifiers: crate::KeyModifiers::new(),
        }));
        assert!(consumed);
        assert!(!state.is_visible());
    }

    #[test]
    fn handle_key_char_appends_to_query() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open"),
        ]);
        state.show();
        state.handle_key(&Event::Key(KeyEvent {
            code: KeyCode::Char('o'),
            modifiers: crate::KeyModifiers::new(),
        }));
        assert_eq!(state.query, "o");
        state.handle_key(&Event::Key(KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: crate::KeyModifiers::new(),
        }));
        assert_eq!(state.query, "op");
    }

    #[test]
    fn handle_key_backspace_removes_char() {
        let mut state = CommandPaletteState::new(vec![]);
        state.show();
        state.query = "ab".to_string();
        state.handle_key(&Event::Key(KeyEvent {
            code: KeyCode::Backspace,
            modifiers: crate::KeyModifiers::new(),
        }));
        assert_eq!(state.query, "a");
    }

    #[test]
    fn handle_key_not_consumed_when_hidden() {
        let mut state = CommandPaletteState::new(vec![]);
        // not visible
        let consumed = state.handle_key(&Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: crate::KeyModifiers::new(),
        }));
        assert!(!consumed);
    }

    // -- selected_command tests --

    #[test]
    fn selected_command_returns_correct_item() {
        let mut state = CommandPaletteState::new(vec![
            Command::new("Alpha"),
            Command::new("Beta"),
            Command::new("Gamma"),
        ]);
        state.selected = 1;
        let cmd = state.selected_command().unwrap();
        assert_eq!(cmd.name, "Beta");
    }

    // -- render smoke test (no panic) --

    #[test]
    fn render_does_not_panic_when_visible() {
        let theme = test_theme();
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open File").with_shortcut("Ctrl+O").with_category("File"),
            Command::new("Save").with_shortcut("Ctrl+S"),
            Command::new("Quit").with_shortcut("Ctrl+Q").with_category("General"),
        ]);
        state.show();

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let widget = CommandPalette::new(&theme);
        widget.render(area, &mut buf, &mut state);
        // Just assert no panic occurred.
    }

    #[test]
    fn render_no_matches_does_not_panic() {
        let theme = test_theme();
        let mut state = CommandPaletteState::new(vec![
            Command::new("Open File"),
        ]);
        state.show();
        state.query = "zzz".to_string();
        state.apply_filter();

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let widget = CommandPalette::new(&theme);
        widget.render(area, &mut buf, &mut state);
    }

    #[test]
    fn render_tiny_area_does_not_panic() {
        let theme = test_theme();
        let mut state = CommandPaletteState::new(vec![Command::new("Test")]);
        state.show();

        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let widget = CommandPalette::new(&theme);
        widget.render(area, &mut buf, &mut state);
    }
}
