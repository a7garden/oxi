//! Interactive fork selector overlay.
//!
//! Replaces the two-step `/fork` text flow with a single list-based overlay
//! where the user can browse messages and fork from any entry.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use super::{centered_layout, OverlayAction, OverlayComponent};
use crate::app::agent_session::AgentSessionHandle;
use oxi_tui::Theme;

// ---------------------------------------------------------------------------
// Shared AppState wrapper (matches factories.rs convention)
// ---------------------------------------------------------------------------

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

// ---------------------------------------------------------------------------
// Fork entry
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct ForkEntry {
    entry_id: String,
    index: usize,
    preview: String,
    timestamp: String,
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

pub struct ForkSelectOverlay {
    list_state: ListState,
    entries: Vec<ForkEntry>,
    #[allow(dead_code)]
    session_handle: AgentSessionHandle,
    #[allow(dead_code)]
    app_state: SharedAppState,
}

impl std::fmt::Debug for ForkSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkSelectOverlay")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl ForkSelectOverlay {
    /// Create a new fork-select overlay.
    ///
    /// `entries` — tuples of `(entry_id, preview_text)` from the session branch.
    pub fn new(
        entries: Vec<(String, String)>,
        session_handle: AgentSessionHandle,
        app_state: SharedAppState,
    ) -> Self {
        let fork_entries: Vec<ForkEntry> = entries
            .into_iter()
            .enumerate()
            .map(|(i, (entry_id, preview))| ForkEntry {
                entry_id,
                index: i + 1,
                preview,
                timestamp: String::new(),
            })
            .collect();

        let mut list_state = ListState::default();
        if !fork_entries.is_empty() {
            list_state.select(Some(0));
        }

        Self {
            list_state,
            entries: fork_entries,
            session_handle,
            app_state,
        }
    }
}

impl OverlayComponent for ForkSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        let len = self.entries.len();
        match key.code {
            KeyCode::Up if len > 0 => {
                let cur = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some(cur.saturating_sub(1)));
            }
            KeyCode::Down if len > 0 => {
                let cur = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some((cur + 1).min(len - 1)));
            }
            KeyCode::Enter => {
                // Integration point: the app layer reads the selected entry
                // from list_state and calls SessionManager::branch_from_entry().
                return OverlayAction::Close;
            }
            KeyCode::Esc => return OverlayAction::Close,
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.80, 0.70);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(Line::styled(
                " Fork from message ",
                Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let list_items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|entry| {
                let preview_truncated: String = entry.preview.chars().take(60).collect();
                let row = format!(
                    "{:>3}. {:<60} {:>10}",
                    entry.index, preview_truncated, entry.timestamp
                );
                ListItem::new(Span::styled(row, styles.normal))
            })
            .collect();

        let list = List::new(list_items)
            .highlight_style(
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(theme.colors.primary.to_ratatui())
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("→ ");

        frame.render_stateful_widget(
            list,
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            },
            &mut self.list_state,
        );

        // Bottom hint bar
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    " {} messages | Up/Down | Enter fork | Esc cancel ",
                    self.entries.len()
                ),
                styles.muted,
            )),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " Up/Down | Enter fork | Esc cancel"
    }
}
