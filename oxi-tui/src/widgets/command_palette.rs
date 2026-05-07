//! CommandPalette widget — fuzzy-filterable command palette overlay.
//!
//! A centered overlay with a text input and scrollable, fuzzy-filtered command
//! list. Built as a ratatui `StatefulWidget` with separate `CommandPaletteState`
//! for mutable interaction state.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, StatefulWidget, Widget, Wrap,
    },
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

        // Clamp popup to buffer bounds
        let max_h = area.height.saturating_sub(y.saturating_sub(area.y));
        let clamped_h = total_height.min(max_h);
        if clamped_h < 3 || palette_w < 3 {
            // Too small to render anything meaningful.
            return;
        }

        let popup_area = Rect {
            x,
            y,
            width: palette_w,
            height: clamped_h,
        };

        // -- dim backdrop (full screen) --
        Clear.render(area, buf);
        let backdrop = Block::default().style(Style::default().bg(Color::Rgb(20, 20, 30)));
        backdrop.render(area, buf);

        // -- outer rounded border --
        let border_block = Block::default()
            .border_type(BorderType::Rounded)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(overlay_bg));
        let inner = border_block.inner(popup_area);
        border_block.render(popup_area, buf);

        // inner dimensions
        let inner_x = inner.x;
        let inner_w = inner.width as usize;
        let input_y = inner.y;
        let sep_y = input_y.saturating_add(1);
        let list_start_y = sep_y.saturating_add(1);

        // -- input row --
        let prompt_style = Style::default().fg(primary).bg(overlay_bg);
        let query_style = Style::default().fg(fg).bg(overlay_bg);

        let input_line = Line::from(vec![
            Span::styled("> ", prompt_style),
            Span::styled(&state.query, query_style),
        ]);
        let input_area = Rect {
            x: inner_x,
            y: input_y,
            width: inner.width,
            height: 1,
        };
        Paragraph::new(input_line)
            .style(Style::default().bg(overlay_bg))
            .render(input_area, buf);

        // cursor block highlight (manual — only 1–2 cells)
        let prompt_len = 2; // "> " is 2 chars
        let cursor_col = inner_x + prompt_len + state.query.len() as u16;
        if (prompt_len as usize + state.query.len()) < inner_w {
            buf[(cursor_col, input_y)]
                .set_char(' ')
                .set_style(Style::default().fg(Color::Black).bg(primary));
        }

        // -- separator line (only if fits inside popup) --
        if sep_y < inner.y + inner.height {
            let separator = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(border_color).bg(overlay_bg))
                .style(Style::default().bg(overlay_bg));
            let sep_area = Rect {
                x: inner_x,
                y: sep_y,
                width: inner.width,
                height: 1,
            };
            separator.render(sep_area, buf);
        }

        // -- scroll clamp --
        let available_list_h = inner.height.saturating_sub(2) as usize; // subtract input + separator rows
        let visible_count = available_list_h.max(1);
        if state.selected < state.scroll_offset {
            state.scroll_offset = state.selected;
        }
        if state.selected >= state.scroll_offset + visible_count {
            state.scroll_offset = state.selected - visible_count + 1;
        }

        // -- command list (only if fits) --
        if list_start_y >= inner.y + inner.height {
            return;
        }
        let actual_list_h = inner
            .height
            .saturating_sub((list_start_y - inner.y) as u16);
        let list_area = Rect {
            x: inner_x,
            y: list_start_y,
            width: inner.width,
            height: actual_list_h,
        };

        if state.filtered_indices.is_empty() {
            let msg = Paragraph::new(Line::from(Span::styled(
                "No matches",
                Style::default().fg(muted).bg(overlay_bg),
            )))
            .style(Style::default().bg(overlay_bg));
            msg.render(list_area, buf);
            return;
        }

        let start = state.scroll_offset;
        let end = (start + visible_count).min(state.filtered_indices.len());

        let lines: Vec<Line> = (start..end)
            .map(|vi| {
                let cmd_idx = state.filtered_indices[vi];
                let cmd = &state.commands[cmd_idx];
                let is_selected = vi == state.selected;

                let (item_fg, item_bg) = if is_selected {
                    (Color::Black, primary)
                } else {
                    (fg, overlay_bg)
                };
                let item_style = Style::default().fg(item_fg).bg(item_bg);

                // Build the left side: optional category prefix + command name
                let mut spans: Vec<Span> = Vec::new();

                if let Some(ref cat) = cmd.category {
                    let cat_style = if is_selected {
                        Style::default().fg(Color::Black).bg(primary)
                    } else {
                        Style::default().fg(muted).bg(overlay_bg)
                    };
                    spans.push(Span::styled(format!("{}: ", cat), cat_style));
                }

                // Truncate name to fit (reserve ~12 chars for shortcut column)
                let cat_len = cmd
                    .category
                    .as_ref()
                    .map(|c| c.chars().count() + 2)
                    .unwrap_or(0);
                let max_name = inner_w.saturating_sub(12);
                let name_str: String = cmd.name.chars().take(max_name.saturating_sub(cat_len)).collect();
                spans.push(Span::styled(name_str, item_style));

                // Pad to fill width, leaving room for right-aligned shortcut
                let current_len: usize = spans.iter().map(|s| s.width()).sum();
                let shortcut_col = if cmd.shortcut.is_some() {
                    inner_w.saturating_sub(12)
                } else {
                    inner_w
                };
                let padding = shortcut_col.saturating_sub(current_len);
                if padding > 0 {
                    spans.push(Span::styled(" ".repeat(padding), item_style));
                }

                // Right-aligned shortcut
                if let Some(ref shortcut) = cmd.shortcut {
                    let sc_style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(primary)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(muted).bg(overlay_bg)
                    };
                    spans.push(Span::styled(format!("{:>width$}", shortcut, width = 12.min(inner_w.saturating_sub(shortcut_col))), sc_style));
                }

                Line::from(spans)
            })
            .collect();

        let list = Paragraph::new(lines)
            .style(Style::default().bg(overlay_bg))
            .wrap(Wrap { trim: false });
        list.render(list_area, buf);

        // -- selected row highlight (fill bg for the entire row) --
        // The Paragraph widget renders text but may not fill the full row bg.
        // Apply inverted bg to the selected row cells manually.
        if state.selected >= start && state.selected < end {
            let row = list_start_y + (state.selected - start) as u16;
            let highlight_style = Style::default().fg(Color::Black).bg(primary);
            for col in inner_x..inner_x + inner.width {
                if col < area.x + area.width && row < area.y + area.height {
                    let cell = &mut buf[(col, row)];
                    // Only override bg for cells that aren't already styled by
                    // the paragraph (empty space at end of row).
                    if cell.symbol() == " " {
                        cell.set_style(highlight_style);
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
