//! Logout / Remove API Key overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame, layout::Rect,
    style::Style,
    text::Span,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::{centered_layout, OverlayAction, OverlayComponent};
use oxi_tui::Theme;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct LogoutSelectOverlay {
    pub providers: Vec<String>,
    pub selected: usize,
    pub on_remove: Box<dyn FnMut(&str)>,
    pub on_message: Box<dyn FnMut(String)>,
}

impl LogoutSelectOverlay {
    pub fn new(
        providers: Vec<String>,
        on_remove: impl FnMut(&str) + 'static,
        on_message: impl FnMut(String) + 'static,
    ) -> Self {
        Self {
            providers,
            selected: 0,
            on_remove,
            on_message,
        }
    }
}

// ---------------------------------------------------------------------------
// OverlayComponent
// ---------------------------------------------------------------------------

impl OverlayComponent for LogoutSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        match key.code {
            KeyCode::Up => {
                self.selected = if self.selected == 0 {
                    self.providers.len().saturating_sub(1)
                } else {
                    self.selected - 1
                };
                OverlayAction::None
            }
            KeyCode::Down => {
                self.selected = if self.providers.is_empty() {
                    0
                } else {
                    (self.selected + 1).min(self.providers.len() - 1)
                };
                OverlayAction::None
            }
            KeyCode::Enter => {
                if let Some(provider) = self.providers.get(self.selected) {
                    let p = provider.clone();
                    (self.on_remove)(&p);
                    (self.on_message)(format!("Removed {}", p));
                }
                OverlayAction::Close
            }
            KeyCode::Esc => OverlayAction::Close,
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.5, 0.5);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(title_line())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let list_items: Vec<ListItem> = self
            .providers
            .iter()
            .enumerate()
            .map(|(i, provider)| {
                let is_sel = i == self.selected;
                let pointer = if is_sel { "-> " } else { "   " };
                let content = format!("{}{}", pointer, provider);
                let style = if is_sel {
                    Style::default()
                        .fg(theme.colors.background)
                        .bg(theme.colors.primary)
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
                y: inner.y,
                width: inner.width,
                height: inner.height,
            },
        );

        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                " Up/Down select  |  Enter remove  |  Esc cancel",
                styles.muted,
            )),
            hint_area,
        );
    }

    fn hint(&self) -> &str {
        " Up/Down  |  Enter remove  |  Esc cancel"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn title_line() -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        " Remove API Key ",
        Style::default().bg(ratatui::style::Color::Rgb(ratatui::style::RgbColor(0, 0, 0))),
    )
}