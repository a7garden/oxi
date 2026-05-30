//! Factory functions to create overlay components.
//!
//! Each overlay struct holds Arc<Mutex<*mut AppState>> to mutate app state
//! from within handle_key(), and AgentSessionHandle to call session methods.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use oxi_tui::widgets::routing::{
    RoutingStatus as RoutingStatusWidget, RoutingStatusData, RoutingStatusState,
};
use oxi_tui::widgets::stateful_list::StatefulList;

use super::{centered_layout, OverlayAction, OverlayComponent};
use crate::app::agent_session::{AgentSession, AgentSessionHandle};
use oxi_store::session::SessionInfo;
use ratatui::{layout::Rect, style::Style, Frame};

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

#[allow(clippy::arc_with_non_send_sync)]
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
    Box::new(ModelSelectOverlay {
        list: StatefulList::new(models),
        session,
        app_state: shared,
    })
}

struct ModelSelectOverlay {
    list: StatefulList<String>,
    session: AgentSessionHandle,
    app_state: SharedAppState,
}

impl std::fmt::Debug for ModelSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSelectOverlay")
            .field("filter", &self.list.filter_text())
            .field("selected_index", &self.list.selected_index())
            .finish()
    }
}

impl OverlayComponent for ModelSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match key.code {
            KeyCode::Up => {
                self.list.select_previous();
            }
            KeyCode::Down => {
                self.list.select_next();
            }
            KeyCode::Enter => {
                if let Some(model_id) = self.list.selected().cloned() {
                    if model_id.starts_with("router/") {
                        let gd = dirs::config_dir().unwrap_or_default().join("oxi");
                        let pd = std::env::current_dir().unwrap_or_default();
                        let has_config =
                            oxi_store::router_config::load_router_config(&gd, &pd).is_some();
                        if !has_config {
                            if let Ok(ptr) = self.app_state.lock() {
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_notification(
                                            "Opening router setup...".to_string(),
                                            crate::tui::app::NotificationKind::Info,
                                        );
                                    }
                                }
                            }
                            return OverlayAction::OpenRouterSetup {
                                initial: crate::tui::overlay::RouterSetupData {
                                    profile_name: "auto".to_string(),
                                    ..Default::default()
                                },
                                models: self.list.items().cloned().collect(),
                            };
                        }
                    }
                    match self.session.set_model(&model_id) {
                        Ok(()) => {
                            if let Ok(ptr) = self.app_state.lock() {
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_notification(
                                            format!("Model: {}", model_id),
                                            crate::tui::app::NotificationKind::Success,
                                        );
                                        app.footer_state.data.model_name = model_id.clone();
                                    }
                                }
                            }
                            oxi_store::settings::Settings::save_last_used(&model_id);
                        }
                        Err(e) => {
                            if let Ok(ptr) = self.app_state.lock() {
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_notification(
                                            format!("Error: {}", e),
                                            crate::tui::app::NotificationKind::Error,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                return OverlayAction::Close;
            }
            KeyCode::Esc => {
                return OverlayAction::Close;
            }
            KeyCode::Backspace => {
                self.list.filter_backspace();
            }
            KeyCode::Char(c) => {
                self.list.filter_input(c);
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
        let filter_text = self.list.filter_text().to_string();
        let count = self.list.len();

        let popup = centered_layout(area, 0.7, 0.7);
        frame.render_widget(Clear, popup);
        let border_block = Block::default()
            .title(title_line(&filter_text))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);
        frame.render_widget(
            Paragraph::new(Span::styled(
                title_text(&filter_text),
                Style::default()
                    .fg(theme.colors.primary.to_ratatui())
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            },
        );

        let items: Vec<ListItem> = self
            .list
            .items()
            .map(|m| ListItem::new(Span::styled(m.as_str(), styles.normal)))
            .collect();

        let highlight_style = Style::default()
            .fg(theme.colors.background.to_ratatui())
            .bg(theme.colors.primary.to_ratatui())
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_symbol("→ ")
            .scroll_padding(3);

        let mut list_state = ratatui::widgets::ListState::default();
        if let Some(idx) = self.list.selected_index() {
            list_state.select(Some(idx));
        }
        frame.render_stateful_widget(
            list,
            Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(3),
            },
            &mut list_state,
        );

        if let Some(idx) = list_state.selected() {
            let max_idx = count.saturating_sub(1);
            self.list.state_mut().select(Some(idx.min(max_idx)));
        }

        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    " {} models  |  Up/Down  |  type to filter  |  Enter select  |  Esc cancel",
                    count
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
        " Up/Down  |  type to filter  |  Enter select  |  Esc cancel"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Logout select
// ─────────────────────────────────────────────────────────────────────────

pub fn logout_select(
    providers: Vec<String>,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    Box::new(LogoutSelectOverlay {
        list: StatefulList::new(providers),
        app_state: share_state(app_state),
    })
}

struct LogoutSelectOverlay {
    list: StatefulList<String>,
    app_state: SharedAppState,
}

impl std::fmt::Debug for LogoutSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogoutSelectOverlay")
            .field("providers", &self.list.len())
            .finish()
    }
}

impl OverlayComponent for LogoutSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match key.code {
            KeyCode::Up => {
                self.list.select_previous();
            }
            KeyCode::Down => {
                self.list.select_next();
            }
            KeyCode::Enter => {
                if let Some(provider) = self.list.selected().cloned() {
                    oxi_store::auth_storage::shared_auth_storage().remove(&provider);
                    if let Ok(ptr) = self.app_state.lock() {
                        unsafe {
                            if let Some(ref mut app) = (*ptr).as_mut() {
                                app.add_notification(
                                    format!("Removed {}", provider),
                                    crate::tui::app::NotificationKind::Success,
                                );
                            }
                        }
                    }
                }
                return OverlayAction::Close;
            }
            KeyCode::Esc => {
                return OverlayAction::Close;
            }
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::{
            style::Style,
            text::Span,
            widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
        };
        let styles = theme.to_styles();

        let popup = centered_layout(area, 0.5, 0.5);
        frame.render_widget(Clear, popup);
        let border_block = Block::default()
            .title(title_line_logout())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let items: Vec<ListItem> = self
            .list
            .items()
            .map(|p| ListItem::new(Span::styled(p.as_str(), styles.normal)))
            .collect();

        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(theme.colors.background.to_ratatui())
                    .bg(theme.colors.primary.to_ratatui()),
            )
            .highlight_symbol("→ ");

        let mut list_state = ratatui::widgets::ListState::default();
        if let Some(idx) = self.list.selected_index() {
            list_state.select(Some(idx));
        }
        frame.render_stateful_widget(list, inner, &mut list_state);
        if let Some(idx) = list_state.selected() {
            let max_idx = self.list.len().saturating_sub(1);
            self.list.state_mut().select(Some(idx.min(max_idx)));
        }

        frame.render_widget(
            Paragraph::new(Span::styled(
                " Up/Down select  |  Enter remove  |  Esc cancel",
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
        " Up/Down  |  Enter remove  |  Esc cancel"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Resume select
// ─────────────────────────────────────────────────────────────────────────

pub fn resume_select(sessions: Vec<SessionInfo>) -> Box<dyn OverlayComponent> {
    Box::new(ResumeSelectOverlay {
        sessions,
        selected: 0,
    })
}

struct ResumeSelectOverlay {
    sessions: Vec<SessionInfo>,
    selected: usize,
}

impl std::fmt::Debug for ResumeSelectOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeSelectOverlay")
            .field("sessions", &self.sessions.len())
            .field("selected", &self.selected)
            .finish()
    }
}

impl OverlayComponent for ResumeSelectOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                if !self.sessions.is_empty() && self.selected >= self.sessions.len() {
                    self.selected = self.sessions.len() - 1;
                }
            }
            KeyCode::Down if !self.sessions.is_empty() => {
                self.selected = (self.selected + 1).min(self.sessions.len() - 1);
            }
            KeyCode::Enter => {
                if let Some(s) = self.sessions.get(self.selected) {
                    return OverlayAction::SwitchSession(s.path.clone());
                }
            }
            KeyCode::Esc => {
                return OverlayAction::Close;
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
        let popup = centered_layout(area, 0.85, 0.85);
        frame.render_widget(Clear, popup);
        let border_block = Block::default()
            .title(title_line_resume())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        let header_style = Style::default()
            .fg(theme.colors.muted.to_ratatui())
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    "{:<20} {:>6} {:<35} {:>12} {:<20}",
                    "NAME", "MSG", "PREVIEW", "TIME", "CWD"
                ),
                header_style,
            )),
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        let highlight_style = Style::default()
            .fg(theme.colors.background.to_ratatui())
            .bg(theme.colors.primary.to_ratatui())
            .add_modifier(Modifier::BOLD);

        let items: Vec<ListItem> = self
            .sessions
            .iter()
            .map(|s| {
                let row = format!(
                    "{:<20} {:>6} {:<35} {:>12} {:<20}",
                    truncate(s.name.as_deref().unwrap_or("new-session"), 18),
                    s.message_count,
                    truncate(s.first_message.as_str(), 33),
                    relative_time(s.created),
                    truncate(&s.cwd, 18),
                );
                ListItem::new(Span::styled(row, styles.normal))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_symbol("→ ");

        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(self.selected));
        let list_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        frame.render_stateful_widget(list, list_area, &mut list_state);
        if let Some(s) = list_state.selected() {
            self.selected = s;
        }

        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    " {} sessions  |  Up/Down  |  Enter switch  |  Esc cancel",
                    self.sessions.len()
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
        " Up/Down  |  Enter switch  |  Esc cancel"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Routing status overlay
// ─────────────────────────────────────────────────────────────────────────

const ROUTING_PANEL_WIDTH: u16 = 50;
const ROUTING_PANEL_HEIGHT: u16 = 14;

pub fn routing_status(data: RoutingStatusData) -> Box<dyn OverlayComponent> {
    Box::new(RoutingOverlay {
        data,
        widget_state: RoutingStatusState::new(),
    })
}

struct RoutingOverlay {
    data: RoutingStatusData,
    widget_state: RoutingStatusState,
}

impl std::fmt::Debug for RoutingOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingOverlay")
            .field("data", &self.data)
            .finish()
    }
}

fn routing_panel_area(area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(ROUTING_PANEL_WIDTH + 1),
        y: area.y + 1,
        width: ROUTING_PANEL_WIDTH.min(area.width.saturating_sub(2)),
        height: ROUTING_PANEL_HEIGHT.min(area.height.saturating_sub(2)),
    }
}

impl OverlayComponent for RoutingOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        match key.code {
            KeyCode::Char('r')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                OverlayAction::Close
            }
            KeyCode::Esc => OverlayAction::Close,
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::widgets::Clear;
        let panel_area = routing_panel_area(area);
        frame.render_widget(Clear, panel_area);
        self.widget_state.data = self.data.clone();
        self.widget_state.visible = true;
        let widget = RoutingStatusWidget::new(theme);
        frame.render_stateful_widget(widget, panel_area, &mut self.widget_state);
    }

    fn hint(&self) -> &str {
        " Esc or Ctrl+R to close"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

fn title_text(filter: &str) -> String {
    if filter.is_empty() {
        " Select a model ".to_string()
    } else {
        format!(" Filter: {} ", filter)
    }
}

fn title_line(filter: &str) -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        title_text(filter),
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

fn truncate(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len > max_width {
        format!(
            "{}...",
            text.chars()
                .take(max_width.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        text.to_string()
    }
}

fn relative_time(dt: DateTime<Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = (now - dt).num_seconds();
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
