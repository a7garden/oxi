//! Model selector overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame, layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::{centered_layout, OverlayAction, OverlayComponent};
use oxi_tui::Theme;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Model selector state.
/// Stored in AppState as a Box<dyn OverlayComponent> variant.
#[derive(Debug)]
pub struct ModelSelectOverlay {
    pub models: Vec<String>,
    pub filter: String,
    pub selected: usize,
    /// Callback to set the active model — set by App when creating the overlay.
    pub on_select: Box<dyn FnMut(&str) -> Result<(), String>>,
    /// Callback to add a system message.
    pub on_message: Box<dyn FnMut(String)>,
}

impl ModelSelectOverlay {
    pub fn new(models: Vec<String>, on_select: impl FnMut(&str) -> Result<(), String> + 'static, on_message: impl FnMut(String) + 'static) -> Self {
        Self {
            models,
            filter: String::new(),
            selected: 0,
            on_select,
            on_message,
        }
    }

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

// ---------------------------------------------------------------------------
// OverlayComponent
// ---------------------------------------------------------------------------

impl OverlayComponent for ModelSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        let filtered = self.filtered();

        match key.code {
            KeyCode::Up => {
                self.selected = if self.selected == 0 {
                    filtered.len().saturating_sub(1)
                } else {
                    self.selected.saturating_sub(1)
                };
                OverlayAction::None
            }
            KeyCode::Down => {
                self.selected = if filtered.is_empty() {
                    0
                } else {
                    (self.selected + 1).min(filtered.len() - 1)
                };
                OverlayAction::None
            }
            KeyCode::Enter => {
                if let Some((_idx, model_id)) = filtered.get(self.selected) {
                    let model_id = (*model_id).clone();
                    match (self.on_select)(&model_id) {
                        Ok(()) => (self.on_message)(format!("Model: {}", model_id)),
                        Err(e) => (self.on_message)(format!("Error: {}", e)),
                    }
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

        let border_block = Block::default()
            .title(title_line(&self.filter))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let title_area = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
        let title_style = Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Span::styled(title_text(&self.filter), title_style)),
            title_area,
        );

        let max_show = (inner.height as usize).saturating_sub(3).max(1);
        let window_start = if selected_in_filtered >= max_show {
            selected_in_filtered - max_show + 1
        } else {
            0
        };

        let list_items: Vec<ListItem> = filtered
            .iter()
            .skip(window_start)
            .take(max_show)
            .enumerate()
            .map(|(i, (_, model))| {
                let is_sel = window_start + i == selected_in_filtered;
                let pointer = if is_sel { "-> " } else { "   " };
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

        let list_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(3),
        };
        frame.render_widget(List::new(list_items), list_area);

        let hint = format!(
            " {} models  |  Up/Down  |  type to filter  |  Enter select  |  Esc cancel",
            filtered.len()
        );
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            hint_area,
        );
    }

    fn hint(&self) -> &str {
        " Up/Down  |  type to filter  |  Enter select  |  Esc cancel"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn title_text(filter: &str) -> String {
    if filter.is_empty() {
        " Select a model ".to_string()
    } else {
        format!(" Filter: {} ", filter)
    }
}

fn title_line(filter: &str) -> ratatui::text::Line<'static> {
    let text = title_text(filter);
    let fg = ratatui::style::Color::Rgb(ratatui::style::RgbColor(0, 0, 0));
    ratatui::text::Line::styled(text, Style::default().fg(ratatui::style::Color::Reset).bg(fg))
}