//! Session resume / switch overlay.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame, layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Paragraph},
};

use super::{centered_layout, OverlayAction, OverlayComponent};
use oxi_tui::Theme;

use oxi_store::session::SessionInfo;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ResumeSelectOverlay {
    pub sessions: Vec<SessionInfo>,
    pub selected: usize,
}

impl ResumeSelectOverlay {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        Self { sessions, selected: 0 }
    }

    fn relative_time(created: DateTime<Utc>) -> String {
        let now = Utc::now();
        let diff = now.signed_duration_since(created).num_seconds();
        if diff < 60 {
            "< 1m ago".to_string()
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else if diff < 86400 {
            format!("{}h ago", diff / 3600)
        } else {
            format!("{}d ago", diff / 86400)
        }
    }

    fn truncate(text: &str, max_width: usize) -> String {
        let text_len = text.chars().count();
        if text_len > max_width {
            format!("{}...", text.chars().take(max_width.saturating_sub(3)).collect::<String>())
        } else {
            text.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// OverlayComponent
// ---------------------------------------------------------------------------

impl OverlayComponent for ResumeSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        match key.code {
            KeyCode::Up => {
                self.selected = if self.selected == 0 {
                    self.sessions.len().saturating_sub(1)
                } else {
                    self.selected - 1
                };
                OverlayAction::None
            }
            KeyCode::Down => {
                self.selected = if self.sessions.is_empty() {
                    0
                } else {
                    (self.selected + 1).min(self.sessions.len() - 1)
                };
                OverlayAction::None
            }
            KeyCode::Enter => {
                if let Some(session_info) = self.sessions.get(self.selected) {
                    let path = session_info.path.clone();
                    return OverlayAction::SwitchSession(path);
                }
                OverlayAction::None
            }
            KeyCode::Esc => OverlayAction::Close,
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.85, 0.85);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(title_line())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Header
        let header_style = Style::default()
            .fg(theme.colors.muted)
            .add_modifier(Modifier::BOLD);
        let header = format!(
            "{:<20} {:>6} {:<35} {:>12} {:<20}",
            "NAME", "MSG", "PREVIEW", "TIME", "CWD"
        );
        frame.render_widget(
            Paragraph::new(Span::styled(header, header_style)),
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        // Rows
        let max_show = (inner.height as usize).saturating_sub(3).max(1);
        let window_start = self.selected.saturating_sub(max_show - 1).min(self.selected);

        for (i, session) in self.sessions.iter().skip(window_start).take(max_show).enumerate() {
            let row_idx = window_start + i;
            let is_sel = row_idx == self.selected;

            let name = Self::truncate(&session.name, 18);
            let msgs = format!("{}", session.message_count);
            let preview = Self::truncate(session.first_message.as_deref().unwrap_or(""), 33);
            let time = Self::relative_time(session.created);
            let cwd = Self::truncate(&session.cwd, 18);

            let row = format!(
                "{:<20} {:>6} {:<35} {:>12} {:<20}",
                name, msgs, preview, time, cwd
            );

            let style = if is_sel {
                Style::default()
                    .fg(theme.colors.background)
                    .bg(theme.colors.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                styles.normal
            };

            frame.render_widget(
                Paragraph::new(Span::styled(row, style)),
                Rect {
                    x: inner.x + 1,
                    y: inner.y + 1 + i as u16,
                    width: inner.width.saturating_sub(2),
                    height: 1,
                },
            );
        }

        // Hint
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        let hint = format!(
            " {} sessions  |  Up/Down  |  Enter switch  |  Esc cancel",
            self.sessions.len()
        );
        frame.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            hint_area,
        );
    }

    fn hint(&self) -> &str {
        " Up/Down  |  Enter switch  |  Esc cancel"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn title_line() -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        " Resume Session ",
        Style::default().bg(ratatui::style::Color::Rgb(ratatui::style::RgbColor(0, 0, 0))),
    )
}