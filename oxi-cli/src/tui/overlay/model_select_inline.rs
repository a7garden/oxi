//! Inline model-selector overlay used by the initial-setup flow.
//!
//! After the user saves an API key in `ProviderSelectOverlay`, the TUI main
//! loop swaps in this overlay to let the user pick a model for the chosen
//! provider.
//!
//! **Design principle**: This overlay is a pure UI component — it owns no
//! references to `AppState` or `AgentSession`. When the user selects a model,
//! it emits [`OverlayAction::ModelSelected`] and the TUI handler applies the
//! change (session, settings, footer, notification).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_tui::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::{centered_layout, OverlayAction, OverlayComponent};

/// Inline model-selector overlay for the initial-setup flow.
///
/// `provider_name` is the provider whose models we're listing.
/// `models` are bare model IDs (e.g. `"gpt-4o"`) — the handler prepends
/// `provider_name/` when applying the selection.
pub struct ModelSelectInlineOverlay {
    provider_name: String,
    models: Vec<String>,
    filter: String,
    selected: usize,
}

impl std::fmt::Debug for ModelSelectInlineOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSelectInlineOverlay")
            .field("provider_name", &self.provider_name)
            .field("models", &self.models.len())
            .field("filter", &self.filter)
            .field("selected", &self.selected)
            .finish()
    }
}

impl ModelSelectInlineOverlay {
    /// Create a new inline model selector.
    pub fn new(provider_name: String, models: Vec<String>) -> Self {
        Self {
            provider_name,
            models,
            filter: String::new(),
            selected: 0,
        }
    }

    /// Filtered view: `(original_index, &model_id)` pairs.
    fn filtered(&self) -> Vec<(usize, &String)> {
        if self.filter.is_empty() {
            self.models.iter().enumerate().collect()
        } else {
            let lower = self.filter.to_lowercase();
            self.models
                .iter()
                .enumerate()
                .filter(|(_, m)| m.to_lowercase().contains(&lower))
                .collect()
        }
    }
}

impl OverlayComponent for ModelSelectInlineOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        let filtered_len = self.filtered().len();

        match key.code {
            KeyCode::Up => {
                self.selected = if self.selected == 0 {
                    filtered_len.saturating_sub(1)
                } else {
                    self.selected.saturating_sub(1)
                };
                OverlayAction::None
            }
            KeyCode::Down => {
                self.selected = if filtered_len == 0 {
                    0
                } else {
                    (self.selected + 1).min(filtered_len - 1)
                };
                OverlayAction::None
            }
            KeyCode::Enter => {
                if let Some((_, model_id)) = self.filtered().get(self.selected) {
                    return OverlayAction::ModelSelected {
                        provider_name: self.provider_name.clone(),
                        model_id: (*model_id).clone(),
                    };
                }
                OverlayAction::Close
            }
            KeyCode::Esc => OverlayAction::Close,
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
                OverlayAction::None
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
                OverlayAction::None
            }
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let filtered = self.filtered();

        // Map selected into the filtered list.
        let selected_in_filtered = if self.filter.is_empty() {
            self.selected
        } else {
            filtered
                .iter()
                .position(|(i, _)| *i == self.selected)
                .unwrap_or(0)
        };

        let popup = centered_layout(area, 0.7, 0.7);
        frame.render_widget(Clear, popup);

        let title = if self.filter.is_empty() {
            format!(" {} — Select a model ", self.provider_name)
        } else {
            format!(" Filter: {} ", self.filter)
        };
        let border_block = Block::default()
            .title(Line::styled(
                title,
                Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Title row
        let title_style = Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} models", filtered.len()),
                title_style,
            )),
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        // List
        let max_show = inner.height.saturating_sub(3) as usize;
        let window_start = if selected_in_filtered >= max_show {
            selected_in_filtered - max_show + 1
        } else {
            0
        };

        let list_items: Vec<ListItem> = filtered
            .iter()
            .skip(window_start)
            .take(max_show)
            .map(|(orig_idx, model)| {
                let is_sel = *orig_idx == self.selected;
                let pointer = if is_sel { "→ " } else { "  " };
                let content = format!("{}{}", pointer, model);
                let style = if is_sel {
                    Style::default()
                        .fg(theme.colors.background)
                        .bg(theme.colors.primary)
                        .add_modifier(Modifier::BOLD)
                } else {
                    styles.normal
                };
                ListItem::new(Span::styled(content, style))
            })
            .collect();

        frame.render_widget(
            List::new(list_items),
            Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(3),
            },
        );

        // Hint
        let hint = " Up/Down  |  type to filter  |  Enter select  |  Esc cancel";
        frame.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            Rect {
                x: inner.x + 1,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " Up/Down  |  type to filter  |  Enter select  |  Esc cancel"
    }
}
