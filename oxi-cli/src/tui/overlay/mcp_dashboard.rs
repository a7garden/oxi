//! MCP dashboard overlay (Phase 2).
//!
//! Implements [`OverlayComponent`] for the `/mcp` command. Renders a
//! generic sectioned dashboard (the `DashboardWidget` from oxi-tui) and
//! translates key presses into [`OverlayAction::McpAction`] events for
//! the TUI handler to act on.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use oxi_agent::mcp::{ConsentState, McpConnectionStatus, McpManager, McpServerInfo, McpToolInfo};
use oxi_tui::Theme;
use oxi_tui::widgets::dashboard::{
    DashboardData, DashboardItem, DashboardSection, DashboardState, DashboardWidget, ItemStatus,
};
use ratatui::{Frame, layout::Rect, widgets::StatefulWidget};

use super::{OverlayAction, OverlayComponent, centered_layout};

/// The MCP dashboard overlay.
pub struct McpDashboardOverlay {
    widget: DashboardWidget,
    state: DashboardState,
    /// `McpManager` used to populate the widget and act on user input.
    manager: Arc<McpManager>,
    /// Set to `true` after a keypress that needs to trigger a re-read
    /// of dashboard data (e.g. after `r` to reconnect). The next
    /// `render()` call will rebuild the data.
    needs_refresh: bool,
}

impl std::fmt::Debug for McpDashboardOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpDashboardOverlay").finish()
    }
}

impl McpDashboardOverlay {
    /// Construct a new overlay for the given manager.
    pub fn new(manager: Arc<McpManager>) -> Self {
        let data = build_dashboard_data(manager.clone());
        let widget = DashboardWidget::new(data);
        Self {
            widget,
            state: DashboardState::new(),
            manager,
            needs_refresh: false,
        }
    }

    /// Mark the overlay for a data refresh on the next render.
    #[allow(dead_code)]
    pub fn mark_refresh(&mut self) {
        self.needs_refresh = true;
    }

    /// ID of the currently selected item, if any.
    fn selected_id(&self) -> Option<String> {
        let data = self.widget.data();
        data.sections
            .get(self.state.selected_section)
            .and_then(|s| {
                s.items
                    .iter()
                    .filter(|i| {
                        self.state.filter.is_empty() || item_label_matches(i, &self.state.filter)
                    })
                    .nth(self.state.selected_item)
                    .map(|i| i.id.clone())
            })
    }
}

impl OverlayComponent for McpDashboardOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if self.state.filter_editing {
            match key.code {
                KeyCode::Esc => self.state.toggle_filter(),
                KeyCode::Enter => self.state.toggle_filter(),
                KeyCode::Backspace => self.state.filter_pop(),
                KeyCode::Char(c) => self.state.filter_push(c),
                _ => {}
            }
            return OverlayAction::None;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                let data = self.widget.data().clone();
                self.state.select_previous(&data);
                OverlayAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let data = self.widget.data().clone();
                self.state.select_next(&data);
                OverlayAction::None
            }
            KeyCode::Char('r') => {
                if let Some(id) = self.selected_id() {
                    OverlayAction::McpAction(McpAction::Reconnect(id))
                } else {
                    OverlayAction::None
                }
            }
            KeyCode::Char('R') => {
                // Capital R: reconnect all
                OverlayAction::McpAction(McpAction::ReconnectAll)
            }
            KeyCode::Char('D') => {
                // Capital D: disconnect selected
                if let Some(id) = self.selected_id() {
                    OverlayAction::McpAction(McpAction::Disconnect(id))
                } else {
                    OverlayAction::None
                }
            }
            KeyCode::Char('a') => {
                if let Some(id) = self.selected_id() {
                    OverlayAction::McpAction(McpAction::SetConsent {
                        name: id,
                        state: ConsentState::Allow,
                    })
                } else {
                    OverlayAction::None
                }
            }
            KeyCode::Char('x') => {
                if let Some(id) = self.selected_id() {
                    OverlayAction::McpAction(McpAction::SetConsent {
                        name: id,
                        state: ConsentState::Deny,
                    })
                } else {
                    OverlayAction::None
                }
            }
            KeyCode::Char('/') => {
                self.state.toggle_filter();
                OverlayAction::None
            }
            KeyCode::Tab => {
                // Cycle sections
                let n = self.widget.data().sections.len();
                if n > 0 {
                    self.state.selected_section = (self.state.selected_section + 1) % n;
                    self.state.selected_item = 0;
                }
                OverlayAction::None
            }
            _ => OverlayAction::None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if self.needs_refresh {
            let data = build_dashboard_data(self.manager.clone());
            self.widget.set_data(data);
            self.needs_refresh = false;
        }

        let popup = centered_layout(area, 0.9, 0.85);
        frame.render_widget(ratatui::widgets::Clear, popup);
        let data = self.widget.data().clone();
        let widget = DashboardWidget::new(data);
        let buf = frame.buffer_mut();
        widget.render(popup, buf, &mut self.state);
        let _ = theme;
    }

    fn hint(&self) -> &str {
        "↑↓/jk Navigate │ r:Reconnect │ R:Reconnect All │ D:Disconnect │ a:Allow │ x:Deny │ Tab:Section │ /:Filter │ Esc:Close"
    }
}

/// Build a `DashboardData` from the current `McpManager` state.
fn build_dashboard_data(manager: Arc<McpManager>) -> DashboardData {
    let data = manager.dashboard_data();

    let mut dashboard = DashboardData::new()
        .with_header(vec![format!(
            "MCP — {}/{} servers, {} tools",
            data.settings.connected_servers, data.settings.total_servers, data.settings.total_tools,
        )])
        .with_footer(vec![format!(
            "Tool prefix: {}    Idle timeout: {}m",
            data.settings.tool_prefix,
            data.settings
                .idle_timeout
                .map(|m| m.to_string())
                .unwrap_or_else(|| "default(10)".to_string()),
        )]);

    // Servers section (one item per server).
    let server_items: Vec<DashboardItem> = data.servers.iter().map(server_to_item).collect();
    dashboard = dashboard.add_section(
        DashboardSection::new(format!("Servers ({})", data.servers.len())).with_items(server_items),
    );

    // Tools section(s) — one per server with tools.
    for server in &data.servers {
        if server.tools.is_empty() {
            continue;
        }
        let items: Vec<DashboardItem> = server.tools.iter().map(tool_to_item).collect();
        dashboard = dashboard.add_section(
            DashboardSection::new(format!("Tools: {} ({})", server.name, items.len()))
                .with_items(items),
        );
    }

    dashboard
}

fn server_to_item(server: &McpServerInfo) -> DashboardItem {
    let status = match &server.status {
        McpConnectionStatus::Connected => ItemStatus::Active,
        McpConnectionStatus::Disconnected | McpConnectionStatus::Connecting => ItemStatus::Inactive,
        McpConnectionStatus::Error(msg) => ItemStatus::Error(msg.clone()),
    };

    let detail = match &server.status {
        McpConnectionStatus::Connected => format!("{} tools", server.tool_count),
        McpConnectionStatus::Disconnected => format!("{} tools (cached)", server.tool_count),
        McpConnectionStatus::Connecting => "connecting…".to_string(),
        McpConnectionStatus::Error(msg) => msg.clone(),
    };

    DashboardItem {
        id: server.name.clone(),
        label: server.name.clone(),
        detail,
        status,
        badges: vec![server.lifecycle.clone()],
    }
}

fn tool_to_item(tool: &McpToolInfo) -> DashboardItem {
    let consent_label = match tool.consent {
        ConsentState::Allow => "ALLOW",
        ConsentState::Deny => "DENY",
    };
    let mut badges = vec![];
    if tool.is_direct {
        badges.push("DIRECT".to_string());
    } else {
        badges.push("PROXY".to_string());
    }
    badges.push(consent_label.to_string());

    DashboardItem {
        id: tool.name.clone(),
        label: tool.name.clone(),
        detail: if tool.description.is_empty() {
            String::new()
        } else {
            tool.description.clone()
        },
        status: ItemStatus::Active,
        badges,
    }
}

fn item_label_matches(item: &DashboardItem, filter: &str) -> bool {
    let f = filter.to_lowercase();
    item.label.to_lowercase().contains(&f)
        || item.id.to_lowercase().contains(&f)
        || item.detail.to_lowercase().contains(&f)
}

/// Action emitted by the dashboard to be applied by the TUI handler.
#[derive(Debug, Clone)]
pub enum McpAction {
    /// Reconnect a single server (the id is the server name).
    Reconnect(String),
    /// Reconnect every server.
    ReconnectAll,
    /// Disconnect a single server.
    Disconnect(String),
    /// Set consent for a tool or server.
    SetConsent {
        /// Tool name (or server name) to update.
        name: String,
        /// New consent state.
        state: ConsentState,
    },
    /// Refresh the dashboard.
    #[allow(dead_code)]
    Refresh,
}
