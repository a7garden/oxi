//! Completion popup widget — inline autocomplete that appears above the editor input.
//!
//! Renders a `ratatui::widgets::List` inside a bordered popup, positioned
//! directly above the input area. Uses `ListState` for selection tracking.
//!
//! Usage:
//! ```ignore
//! let mut state = CompletionState::new();
//! state.set_items(vec![CompletionItem { ... }]);
//! state.show();
//! let mut popup = CompletionPopup::new(&mut state, &theme);
//! popup.render(frame, input_area);
//! ```

use crate::Theme;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Clear, List, ListItem};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Kind of completion item, used for icon selection.
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionKind {
    /// Slash command (`/help`, `/model`, …).
    Command,
    /// File path.
    File,
    /// Directory path.
    Directory,
    /// Model name.
    Model,
}

/// A single completion item displayed in the popup.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Human-readable label shown in the list.
    pub label: String,
    /// Optional description shown after the label.
    pub description: Option<String>,
    /// Text to insert when this item is accepted.
    pub insert_text: String,
    /// Category of the item (controls the icon).
    pub kind: CompletionKind,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Tracks completion items, list selection, and visibility.
#[derive(Debug, Default)]
pub struct CompletionState {
    /// Current completion candidates.
    pub items: Vec<CompletionItem>,
    /// ratatui list-state for highlighting the selected row.
    pub list_state: ratatui::widgets::ListState,
    /// Whether the popup is currently shown.
    pub visible: bool,
}

impl CompletionState {
    /// Create a new, hidden completion state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the item list and reset selection to the first row.
    pub fn set_items(&mut self, items: Vec<CompletionItem>) {
        self.items = items;
        if self.items.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    /// Return the currently selected item, if any.
    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.list_state.selected().and_then(|i| self.items.get(i))
    }

    /// Move the selection down by one (wraps to top).
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len();
        let current = self.list_state.selected().unwrap_or(0);
        let next = (current + 1) % len;
        self.list_state.select(Some(next));
    }

    /// Move the selection up by one (wraps to bottom).
    pub fn select_previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len();
        let current = self.list_state.selected().unwrap_or(0);
        let prev = if current == 0 { len - 1 } else { current - 1 };
        self.list_state.select(Some(prev));
    }

    /// Show the popup.
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Hide the popup.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Toggle popup visibility.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Returns `true` if the popup is currently visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// Renders a completion popup above the input area.
///
/// The popup uses `ratatui::widgets::List` with selection highlighting
/// and is cleared/redrawn each frame via the `Clear` widget.
pub struct CompletionPopup<'a> {
    /// Shared completion state (items + selection).
    pub state: &'a mut CompletionState,
    /// Theme used for colors.
    pub theme: &'a Theme,
    /// Maximum number of items visible before scrolling.
    pub max_visible: usize,
}

impl<'a> CompletionPopup<'a> {
    /// Create a new popup bound to the given state and theme.
    pub fn new(state: &'a mut CompletionState, theme: &'a Theme) -> Self {
        Self {
            state,
            theme,
            max_visible: 5,
        }
    }

    /// Render the popup above the given input area.
    ///
    /// Returns the area occupied by the popup, or `None` if the popup
    /// is not visible or has no items.
    pub fn render(&mut self, frame: &mut ratatui::Frame, input_area: Rect) -> Option<Rect> {
        if !self.state.visible || self.state.items.is_empty() {
            return None;
        }

        // Determine how many rows we can show.
        let item_count = self.state.items.len().min(self.max_visible);
        let available_above = input_area.y as usize;
        let popup_height = item_count.min(available_above).min(u16::MAX as usize) as u16;

        if popup_height == 0 {
            return None;
        }

        // Position the popup directly above the input.
        let popup_area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(popup_height),
            width: input_area.width,
            height: popup_height,
        };

        // Clear the area first so previous frame content doesn't bleed through.
        frame.render_widget(Clear, popup_area);

        // Build list items with kind icons.
        let items: Vec<ListItem> = self
            .state
            .items
            .iter()
            .take(self.max_visible)
            .map(|item| {
                let kind_icon = match item.kind {
                    CompletionKind::Command => "/",
                    CompletionKind::File => "[f]",
                    CompletionKind::Directory => "[d]",
                    CompletionKind::Model => "[m]",
                };
                let label = Span::styled(format!("{} ", item.label), Style::default().bold());
                let desc = item.description.as_ref().map(|d| {
                    Span::styled(
                        format!(" {}", d),
                        Style::default().fg(self.theme.colors.muted),
                    )
                });
                let mut spans = vec![
                    Span::styled(kind_icon, Style::default()),
                    Span::raw(" "),
                    label,
                ];
                if let Some(d) = desc {
                    spans.push(d);
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .block(Block::bordered().style(Style::default().fg(self.theme.colors.border)))
            .highlight_style(
                Style::default()
                    .fg(self.theme.colors.background)
                    .bg(self.theme.colors.primary),
            )
            .highlight_symbol("→ ");

        frame.render_stateful_widget(list, popup_area, &mut self.state.list_state);

        Some(popup_area)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_items() -> Vec<CompletionItem> {
        vec![
            CompletionItem {
                label: "/help".into(),
                description: Some("Show help".into()),
                insert_text: "/help".into(),
                kind: CompletionKind::Command,
            },
            CompletionItem {
                label: "src/main.rs".into(),
                description: None,
                insert_text: "src/main.rs".into(),
                kind: CompletionKind::File,
            },
            CompletionItem {
                label: "target/".into(),
                description: Some("directory".into()),
                insert_text: "target/".into(),
                kind: CompletionKind::Directory,
            },
        ]
    }

    #[test]
    fn new_state_is_hidden() {
        let state = CompletionState::new();
        assert!(!state.is_visible());
        assert!(state.items.is_empty());
    }

    #[test]
    fn show_hide_toggle() {
        let mut state = CompletionState::new();
        assert!(!state.is_visible());
        state.show();
        assert!(state.is_visible());
        state.hide();
        assert!(!state.is_visible());
        state.toggle();
        assert!(state.is_visible());
        state.toggle();
        assert!(!state.is_visible());
    }

    #[test]
    fn set_items_resets_selection() {
        let mut state = CompletionState::new();
        state.set_items(sample_items());
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.list_state.selected(), Some(0));
        assert_eq!(state.selected_item().unwrap().label, "/help");
    }

    #[test]
    fn set_items_clear_selects_none() {
        let mut state = CompletionState::new();
        state.set_items(sample_items());
        state.set_items(vec![]);
        assert!(state.items.is_empty());
        assert!(state.list_state.selected().is_none());
        assert!(state.selected_item().is_none());
    }

    #[test]
    fn select_next_wraps() {
        let mut state = CompletionState::new();
        state.set_items(sample_items());
        assert_eq!(state.list_state.selected(), Some(0));
        state.select_next();
        assert_eq!(state.list_state.selected(), Some(1));
        state.select_next();
        assert_eq!(state.list_state.selected(), Some(2));
        state.select_next(); // wraps to 0
        assert_eq!(state.list_state.selected(), Some(0));
    }

    #[test]
    fn select_previous_wraps() {
        let mut state = CompletionState::new();
        state.set_items(sample_items());
        assert_eq!(state.list_state.selected(), Some(0));
        state.select_previous(); // wraps to last
        assert_eq!(state.list_state.selected(), Some(2));
        state.select_previous();
        assert_eq!(state.list_state.selected(), Some(1));
    }

    #[test]
    fn select_navigation_empty_items() {
        let mut state = CompletionState::new();
        state.select_next(); // no panic
        state.select_previous(); // no panic
        assert!(state.list_state.selected().is_none());
    }
}
