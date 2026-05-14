//! Factory functions to create overlay components from AppState + AgentSession.
//!
//! These bridge the old AppState-based world with the new OverlayComponent trait.
//! Each factory captures the necessary references to update AppState when the
//! overlay takes action.

use crossterm::event::{KeyCode, KeyEventKind};

use ratatui::{layout::Rect, style::Style, Frame};

use super::{OverlayAction, OverlayComponent};
use crate::agent_session::AgentSessionHandle;
use oxi_store::session::SessionInfo;
use oxi_store::settings::Settings;

// ---------------------------------------------------------------------------
// Model select
// ---------------------------------------------------------------------------

/// Create a ModelSelectOverlay that hooks into AppState and AgentSession.
pub fn model_select(
    models: Vec<String>,
    session: &AgentSession,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    let session = session.clone_handle();

    Box::new(ModelSelectBridge {
        models,
        session,
        filter: String::new(),
        selected: 0,
        app_state: std::rc::Rc::new(std::cell::RefCell::new(app_state)),
    })
}

struct ModelSelectBridge {
    models: Vec<String>,
    filter: String,
    selected: usize,
    session: AgentSessionHandle,
    app_state: std::rc::Rc<std::cell::RefCell<crate::tui::app::AppState>>,
}

impl std::fmt::Debug for ModelSelectBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSelectBridge")
            .field("models", &self.models)
            .field("filter", &self.filter)
            .field("selected", &self.selected)
            .finish()
    }
}

impl OverlayComponent for ModelSelectBridge {
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::{KeyCode, KeyEventKind};

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
                    match self.session.set_model(&model_id) {
                        Ok(()) => {
                            let mut app = self.app_state.borrow_mut();
                            app.add_system_message(format!("Model: {}", model_id));
                            app.footer_state.data.model_name = model_id.clone();
                            drop(app);
                            let _ = Settings::save_last_used(&model_id);
                        }
                        Err(e) => {
                            self.app_state.borrow_mut().add_system_message(format!("Error: {}", e));
                        }
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

    fn render(&mut self, frame: &mut ratatui::Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{
            style::{Modifier, Style},
            text::Span,
            widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
        };

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

        let popup = centered_popup(area, 0.7, 0.7);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(title_line(&self.filter))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let title_area = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
        let title_style = Style::default()
            .fg(theme.colors.primary.to_ratatui())
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
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
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

impl ModelSelectBridge {
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
// Logout select
// ---------------------------------------------------------------------------

pub fn logout_select(
    providers: Vec<String>,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    Box::new(LogoutSelectBridge {
        providers,
        selected: 0,
        app_state: std::rc::Rc::new(std::cell::RefCell::new(app_state)),
    })
}

struct LogoutSelectBridge {
    providers: Vec<String>,
    selected: usize,
    app_state: std::rc::Rc<std::cell::RefCell<crate::tui::app::AppState>>,
}

impl std::fmt::Debug for LogoutSelectBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogoutSelectBridge")
            .field("providers", &self.providers)
            .field("selected", &self.selected)
            .finish()
    }
}

impl OverlayComponent for LogoutSelectBridge {
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::{KeyCode, KeyEventKind};

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
                    let auth = oxi_store::auth_storage::AuthStorage::new();
                    auth.remove(&p);
                    self.app_state.borrow_mut().add_system_message(format!("Removed {}", p));
                }
                OverlayAction::Close
            }
            KeyCode::Esc => OverlayAction::Close,
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{
            style::Style,
            text::Span,
            widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
        };

        let styles = theme.to_styles();
        let popup = centered_popup(area, 0.5, 0.5);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(title_line_logout())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
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
                        .fg(theme.colors.background.to_ratatui())
                        .bg(theme.colors.primary.to_ratatui())
                } else {
                    styles.normal
                };
                ListItem::new(Span::styled(content, style))
            })
            .collect();

        frame.render_widget(
            List::new(list_items),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: inner.height },
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
// Resume select
// ---------------------------------------------------------------------------

pub fn resume_select(sessions: Vec<SessionInfo>) -> Box<dyn OverlayComponent> {
    Box::new(ResumeSelectBridge { sessions, selected: 0 })
}

struct ResumeSelectBridge {
    sessions: Vec<SessionInfo>,
    selected: usize,
}

impl std::fmt::Debug for ResumeSelectBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeSelectBridge")
            .field("sessions", &self.sessions.len())
            .field("selected", &self.selected)
            .finish()
    }
}

impl OverlayComponent for ResumeSelectBridge {
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> OverlayAction {
        use crossterm::event::{KeyCode, KeyEventKind};

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

    fn render(&mut self, frame: &mut ratatui::Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{
            style::{Modifier, Style},
            text::Span,
            widgets::{Block, Borders, Clear, Paragraph},
        };

        let styles = theme.to_styles();
        let popup = centered_popup(area, 0.85, 0.85);
        frame.render_widget(Clear, popup);

        let border_block = Block::default()
            .title(title_line_resume())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Header
        let header_style = Style::default()
            .fg(theme.colors.muted.to_ratatui())
            .add_modifier(Modifier::BOLD);
        let header = format!(
            "{:<20} {:>6} {:<35} {:>12} {:<20}",
            "NAME", "MSG", "PREVIEW", "TIME", "CWD"
        );
        frame.render_widget(
            Paragraph::new(Span::styled(header, header_style)),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
        );

        // Rows
        let max_show = (inner.height as usize).saturating_sub(3).max(1);
        let window_start = self.selected.saturating_sub(max_show - 1).min(self.selected);

        for (i, session) in self.sessions.iter().skip(window_start).take(max_show).enumerate() {
            let row_idx = window_start + i;
            let is_sel = row_idx == self.selected;

            let name = Self::truncate(session.name.as_deref().unwrap_or("new-session"), 18);
            let msgs = format!("{}", session.message_count);
            let preview = Self::truncate(session.first_message.as_deref().unwrap_or(""), 33);
            let time = Self::relative_time(session.created.timestamp());
            let cwd = Self::truncate(&session.cwd, 18);

            let row = format!("{:<20} {:>6} {:<35} {:>12} {:<20}", name, msgs, preview, time, cwd);

            let style = if is_sel {
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(theme.colors.primary.to_ratatui())
                    .add_modifier(Modifier::BOLD)
            } else {
                styles.normal
            };

            frame.render_widget(
                Paragraph::new(Span::styled(row, style)),
                Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 },
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

impl ResumeSelectBridge {
    fn relative_time(created: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let diff = now.saturating_sub(created);
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
// Helpers
// ---------------------------------------------------------------------------

fn centered_popup(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    let w = (area.width as f32 * width_pct) as u16;
    let h = (area.height as f32 * height_pct) as u16;
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

fn title_text(filter: &str) -> String {
    if filter.is_empty() {
        " Select a model ".to_string()
    } else {
        format!(" Filter: {} ", filter)
    }
}

fn title_line(filter: &str) -> ratatui::text::Line<'static> {
    let text = title_text(filter);
    ratatui::text::Line::styled(
        text,
        Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
    )
}

fn title_line_logout() -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        " Remove API Key ",
        Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
    )
}

fn title_line_resume() -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        " Resume Session ",
        Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
    )
}