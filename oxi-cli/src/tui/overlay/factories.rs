//! Factory functions to create overlay components.
//!
//! Each overlay struct holds Arc<Mutex<*mut AppState>> to mutate app state
//! from within handle_key(), and AgentSessionHandle to call session methods.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::{centered_popup, OverlayAction, OverlayComponent};
use crate::app::agent_session::{AgentSession, AgentSessionHandle};
use oxi_store::session::SessionInfo;
use oxi_store::settings::Settings;
use ratatui::{layout::Rect, style::Style, Frame};

// ─────────────────────────────────────────────────────────────────────────
// Shared AppState wrapper — holds raw pointer to avoid Default bound
// ─────────────────────────────────────────────────────────────────────────

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

fn share_state(state: &mut crate::tui::app::AppState) -> SharedAppState {
    Arc::new(Mutex::new(state as *mut _))
}

// ─────────────────────────────────────────────────────────────────────────
// Model select
// ─────────────────────────────────────────────────────────────────────────

pub fn model_select(
    models: Vec<String>,
    session: &AgentSession,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    let shared = share_state(app_state);
    let session = session.clone_handle();
    Box::new(ModelSelectOverlay { models, filter: String::new(), selected: 0, session, app_state: shared })
}

struct ModelSelectOverlay {
    models: Vec<String>,
    filter: String,
    selected: usize,
    session: AgentSessionHandle,
    app_state: SharedAppState,
}

impl std::fmt::Debug for ModelSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSelectOverlay")
            .field("models", &self.models)
            .field("filter", &self.filter)
            .field("selected", &self.selected)
            .finish()
    }
}

impl OverlayComponent for ModelSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        let filtered = self.filtered();
        let filtered_len = filtered.len();
        match key.code {
            KeyCode::Up => {
                if self.selected > 0 { self.selected -= 1; }
                if filtered_len > 0 && self.selected >= filtered_len {
                    self.selected = filtered_len - 1;
                }
            }
            KeyCode::Down => {
                if !filtered.is_empty() {
                    self.selected = (self.selected + 1).min(filtered.len() - 1);
                }
            }
            KeyCode::Enter => {
                let selected = self.selected;
                if let Some((_idx, model_id)) = filtered.get(selected) {
                    let model_id = (*model_id).clone();
                    match self.session.set_model(&model_id) {
                        Ok(()) => {
                            if let Ok(ptr) = self.app_state.lock() {
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_system_message(format!("Model: {}", model_id));
                                        app.footer_state.data.model_name = model_id.clone();
                                    }
                                }
                            }
                            let _ = Settings::save_last_used(&model_id);
                        }
                        Err(e) => {
                            if let Ok(ptr) = self.app_state.lock() {
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_system_message(format!("Error: {}", e));
                                    }
                                }
                            }
                        }
                    }
                }
                return OverlayAction::Close;
            }
            KeyCode::Esc => return OverlayAction::Close,
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{
            style::{Modifier, Style},
            text::Span,
            widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
        };
        let styles = theme.to_styles();
        let filtered = self.filtered();
        let selected_in_filtered = if self.filter.is_empty() {
            self.selected.min(filtered.len().saturating_sub(1))
        } else {
            filtered.iter().position(|(i, _)| *i == self.selected).unwrap_or(0)
        };

        let popup = centered_popup(area, 0.7, 0.7);
        frame.render_widget(Clear, popup);
        let border_block = Block::default()
            .title(title_line(&self.filter))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);
        frame.render_widget(
            Paragraph::new(Span::styled(title_text(&self.filter), Style::default().fg(theme.colors.primary.to_ratatui()).add_modifier(Modifier::BOLD))),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
        );
        let max_show = (inner.height as usize).saturating_sub(3).max(1);
        let window_start = if selected_in_filtered >= max_show { selected_in_filtered - max_show + 1 } else { 0 };
        let list_items: Vec<ListItem> = filtered.iter().skip(window_start).take(max_show).enumerate()
            .map(|(i, (_, model))| {
                let is_sel = window_start + i == selected_in_filtered;
                let style = if is_sel {
                    Style::default().fg(theme.colors.background.to_ratatui()).bg(theme.colors.primary.to_ratatui()).add_modifier(Modifier::BOLD)
                } else { styles.normal };
                ListItem::new(Span::styled(format!("{}{}", if is_sel { "-> " } else { "   " }, model), style))
            })
            .collect();
        frame.render_widget(List::new(list_items), Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: inner.height.saturating_sub(3) });
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {} models  |  Up/Down  |  type to filter  |  Enter select  |  Esc cancel", filtered.len()), styles.muted)),
            Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(1), width: inner.width, height: 1 },
        );
    }

    fn hint(&self) -> &str { " Up/Down  |  type to filter  |  Enter select  |  Esc cancel" }
}

impl ModelSelectOverlay {
    fn filtered(&self) -> Vec<(usize, &String)> {
        if self.filter.is_empty() {
            self.models.iter().enumerate().collect()
        } else {
            let lower = self.filter.to_lowercase();
            self.models.iter().enumerate().filter(|(_, m)| m.to_lowercase().contains(&lower)).collect()
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Logout select
// ─────────────────────────────────────────────────────────────────────────

pub fn logout_select(
    providers: Vec<String>,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    Box::new(LogoutSelectOverlay { providers, selected: 0, app_state: share_state(app_state) })
}

struct LogoutSelectOverlay {
    providers: Vec<String>,
    selected: usize,
    app_state: SharedAppState,
}

impl std::fmt::Debug for LogoutSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogoutSelectOverlay").field("providers", &self.providers).field("selected", &self.selected).finish()
    }
}

impl OverlayComponent for LogoutSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press { return OverlayAction::None; }
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                if !self.providers.is_empty() && self.selected >= self.providers.len() { self.selected = self.providers.len() - 1; }
            }
            KeyCode::Down => {
                if !self.providers.is_empty() { self.selected = (self.selected + 1).min(self.providers.len() - 1); }
            }
            KeyCode::Enter => {
                if let Some(provider) = self.providers.get(self.selected) {
                    let p = provider.clone();
                    oxi_store::auth_storage::shared_auth_storage().remove(&p);
                    if let Ok(ptr) = self.app_state.lock() {
                        unsafe {
                            if let Some(ref mut app) = (*ptr).as_mut() {
                                app.add_system_message(format!("Removed {}", p));
                            }
                        }
                    }
                }
                return OverlayAction::Close;
            }
            KeyCode::Esc => return OverlayAction::Close,
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{style::Style, text::Span, widgets::{Block, Borders, Clear, List, ListItem, Paragraph}};
        let styles = theme.to_styles();
        let popup = centered_popup(area, 0.5, 0.5);
        frame.render_widget(Clear, popup);
        let border_block = Block::default().title(title_line_logout()).borders(Borders::ALL).border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);
        let list_items: Vec<ListItem> = self.providers.iter().enumerate()
            .map(|(i, provider)| {
                let is_sel = i == self.selected;
                let style = if is_sel { Style::default().fg(theme.colors.background.to_ratatui()).bg(theme.colors.primary.to_ratatui()) } else { styles.normal };
                ListItem::new(Span::styled(format!("{}{}", if is_sel { "-> " } else { "   " }, provider), style))
            })
            .collect();
        frame.render_widget(List::new(list_items), Rect { x: inner.x, y: inner.y, width: inner.width, height: inner.height });
        frame.render_widget(
            Paragraph::new(Span::styled(" Up/Down select  |  Enter remove  |  Esc cancel", styles.muted)),
            Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(1), width: inner.width, height: 1 },
        );
    }

    fn hint(&self) -> &str { " Up/Down  |  Enter remove  |  Esc cancel" }
}

// ─────────────────────────────────────────────────────────────────────────
// Resume select
// ─────────────────────────────────────────────────────────────────────────

pub fn resume_select(sessions: Vec<SessionInfo>) -> Box<dyn OverlayComponent> {
    Box::new(ResumeSelectOverlay { sessions, selected: 0 })
}

struct ResumeSelectOverlay {
    sessions: Vec<SessionInfo>,
    selected: usize,
}

impl std::fmt::Debug for ResumeSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeSelectOverlay").field("sessions", &self.sessions.len()).field("selected", &self.selected).finish()
    }
}

impl OverlayComponent for ResumeSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press { return OverlayAction::None; }
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                if !self.sessions.is_empty() && self.selected >= self.sessions.len() { self.selected = self.sessions.len() - 1; }
            }
            KeyCode::Down => {
                if !self.sessions.is_empty() { self.selected = (self.selected + 1).min(self.sessions.len() - 1); }
            }
            KeyCode::Enter => {
                if let Some(s) = self.sessions.get(self.selected) {
                    return OverlayAction::SwitchSession(s.path.clone());
                }
            }
            KeyCode::Esc => return OverlayAction::Close,
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{style::{Modifier, Style}, text::Span, widgets::{Block, Borders, Clear, Paragraph}};
        let styles = theme.to_styles();
        let popup = centered_popup(area, 0.85, 0.85);
        frame.render_widget(Clear, popup);
        let border_block = Block::default().title(title_line_resume()).borders(Borders::ALL).border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);
        let header_style = Style::default().fg(theme.colors.muted.to_ratatui()).add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{:<20} {:>6} {:<35} {:>12} {:<20}", "NAME", "MSG", "PREVIEW", "TIME", "CWD"), header_style)),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
        );
        let max_show = (inner.height as usize).saturating_sub(3).max(1);
        let window_start = self.selected.saturating_sub(max_show - 1).min(self.selected);
        for (i, session) in self.sessions.iter().skip(window_start).take(max_show).enumerate() {
            let row_idx = window_start + i;
            let is_sel = row_idx == self.selected;
            let style = if is_sel { Style::default().fg(theme.colors.background.to_ratatui()).bg(theme.colors.primary.to_ratatui()).add_modifier(Modifier::BOLD) } else { styles.normal };
            let row = format!(
                "{:<20} {:>6} {:<35} {:>12} {:<20}",
                truncate(session.name.as_deref().unwrap_or("new-session"), 18),
                session.message_count,
                truncate(session.first_message.as_str(), 33),
                relative_time(session.created),
                truncate(&session.cwd, 18),
            );
            frame.render_widget(
                Paragraph::new(Span::styled(row, style)),
                Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 },
            );
        }
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {} sessions  |  Up/Down  |  Enter switch  |  Esc cancel", self.sessions.len()), styles.muted)),
            Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(1), width: inner.width, height: 1 },
        );
    }

    fn hint(&self) -> &str { " Up/Down  |  Enter switch  |  Esc cancel" }
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

fn title_text(filter: &str) -> String {
    if filter.is_empty() { " Select a model ".to_string() } else { format!(" Filter: {} ", filter) }
}

fn title_line(filter: &str) -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(title_text(filter), Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)))
}

fn title_line_logout() -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(" Remove API Key ", Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)))
}

fn title_line_resume() -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(" Resume Session ", Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)))
}

fn truncate(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len > max_width { format!("{}...", text.chars().take(max_width.saturating_sub(3)).collect::<String>()) } else { text.to_string() }
}

fn relative_time(dt: DateTime<Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = (now - dt).num_seconds();
    if diff < 60 { "< 1m ago".to_string() }
    else if diff < 3600 { format!("{}m ago", diff / 60) }
    else if diff < 86400 { format!("{}h ago", diff / 3600) }
    else { format!("{}d ago", diff / 86400) }
}