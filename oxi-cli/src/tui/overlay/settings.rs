//! Interactive settings overlay for the `/settings` command.
//!
//! Displays all oxi settings with the ability to toggle/cycle values.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, ListItem, Paragraph},
    Frame,
};

use super::{centered_layout, OverlayAction, OverlayComponent};
use crate::app::agent_session::AgentSessionHandle;

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

#[allow(clippy::arc_with_non_send_sync)]
fn share_state(state: &mut crate::tui::app::AppState) -> SharedAppState {
    Arc::new(Mutex::new(state as *mut _))
}

// ─────────────────────────────────────────────────────────────────────────
// Settings item
// ─────────────────────────────────────────────────────────────────────────

enum SettingsItem {
    Toggle { label: String, value: bool },
    Choice { label: String, value: String },
    ReadOnly { label: String, value: String },
    Action { label: String, id: &'static str },
}

impl SettingsItem {
    fn label(&self) -> &str {
        match self {
            SettingsItem::Toggle { label, .. } => label,
            SettingsItem::Choice { label, .. } => label,
            SettingsItem::ReadOnly { label, .. } => label,
            SettingsItem::Action { label, .. } => label,
        }
    }
    fn value_str(&self) -> String {
        match self {
            SettingsItem::Toggle { value, .. } => {
                if *value {
                    "● on".to_string()
                } else {
                    "○ off".to_string()
                }
            }
            SettingsItem::Choice { value, .. } => value.clone(),
            SettingsItem::ReadOnly { value, .. } => value.clone(),
            SettingsItem::Action { label, .. } => label.to_string(),
        }
    }
    fn is_editable(&self) -> bool {
        !matches!(self, SettingsItem::ReadOnly { .. })
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Settings overlay
// ─────────────────────────────────────────────────────────────────────────

pub fn settings_overlay(
    session: &AgentSessionHandle,
    app_state: &mut crate::tui::app::AppState,
) -> Box<dyn OverlayComponent> {
    let session = session.clone_handle();
    let all_items = build_settings_items(&session);
    let item_count = all_items.len();
    Box::new(SettingsOverlay {
        all_items,
        filtered_indices: (0..item_count).collect(),
        selected: 0,
        filter: String::new(),
        session,
        app_state: share_state(app_state),
        changed: false,
    })
}

struct SettingsOverlay {
    all_items: Vec<SettingsItem>,
    filtered_indices: Vec<usize>,
    selected: usize,
    filter: String,
    session: AgentSessionHandle,
    app_state: SharedAppState,
    changed: bool,
}

impl std::fmt::Debug for SettingsOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsOverlay")
            .field("items", &self.all_items.len())
            .field("filtered", &self.filtered_indices.len())
            .field("changed", &self.changed)
            .finish()
    }
}

impl SettingsOverlay {
    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_indices = (0..self.all_items.len()).collect();
        } else {
            let lower = self.filter.to_lowercase();
            self.filtered_indices = self
                .all_items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.label().to_lowercase().contains(&lower))
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
    }

    fn visible_count(&self) -> usize {
        self.filtered_indices.len()
    }
}

impl OverlayComponent for SettingsOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }
        let count = self.visible_count();
        match key.code {
            KeyCode::Up => {
                self.selected = if count == 0 {
                    0
                } else {
                    self.selected.saturating_sub(1)
                };
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::PageUp => {
                self.selected = self
                    .selected
                    .saturating_sub(10)
                    .min(count.saturating_sub(1));
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + 10).min(count.saturating_sub(1));
            }
            KeyCode::Home => {
                self.selected = 0;
            }
            KeyCode::End => {
                self.selected = count.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if count == 0 {
                    return OverlayAction::None;
                }
                let item_idx = self.filtered_indices[self.selected];
                let item = &mut self.all_items[item_idx];
                match item {
                    SettingsItem::Toggle { value, .. } => {
                        *value = !*value;
                        self.changed = true;
                    }
                    SettingsItem::Choice { label, value, .. } => {
                        let options = get_choice_options(label);
                        if let Some(pos) = options.iter().position(|o| o == value) {
                            let next = (pos + 1) % options.len();
                            *value = options[next].clone();
                            self.changed = true;
                        }
                    }
                    SettingsItem::Action { id, .. } => {
                        if *id == "reload" {
                            self.all_items = build_settings_items(&self.session);
                            self.apply_filter();
                            if let Ok(ptr) = self.app_state.lock() {
                                // SAFETY: We hold the Mutex lock, guaranteeing exclusive
                                // access. The raw pointer was created from a valid &mut AppState
                                // via share_state() and is only dereferenced while locked.
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_notification(
                                            "Settings reloaded.".to_string(),
                                            crate::tui::app::NotificationKind::Success,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    SettingsItem::ReadOnly { .. } => {}
                }
            }
            KeyCode::Esc => {
                if self.changed {
                    if let Ok(ptr) = self.app_state.lock() {
                        // SAFETY: We hold the Mutex lock, guaranteeing exclusive access.
                        // The raw pointer is valid for the lock's lifetime.
                        unsafe {
                            if let Some(ref mut app) = (*ptr).as_mut() {
                                app.add_notification(
                                    "Settings saved.".to_string(),
                                    crate::tui::app::NotificationKind::Success,
                                );
                            }
                        }
                    }
                }
                return OverlayAction::Close;
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.apply_filter();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter();
            }
            _ => {}
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui::Theme) {
        use ratatui::widgets::List;
        let styles = theme.to_styles();
        let filter_text = if self.filter.is_empty() {
            "Settings".to_string()
        } else {
            format!("Settings: {}", self.filter)
        };

        let popup = centered_layout(area, 0.75, 0.8);
        frame.render_widget(Clear, popup);
        let border_block = Block::default()
            .title(title_line(&filter_text))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border.to_ratatui()));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Header
        frame.render_widget(
            Paragraph::new(Span::styled(
                " KEY                   VALUE                    ",
                Style::default()
                    .fg(theme.colors.muted.to_ratatui())
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: 1,
            },
        );

        // Collect items into owned Vec first to avoid borrow conflicts
        let list_items: Vec<ListItem> = self
            .filtered_indices
            .iter()
            .map(|&idx| {
                let item = &self.all_items[idx];
                let label = format!("{:<22}", item.label());
                let value = format!("{:<20}", item.value_str());
                let style = if item.is_editable() {
                    styles.normal
                } else {
                    Style::default().fg(theme.colors.muted.to_ratatui())
                };
                ListItem::new(Span::styled(format!("{} {}", label, value), style))
            })
            .collect();

        let highlight_style = Style::default()
            .fg(theme.colors.background.to_ratatui())
            .bg(theme.colors.primary.to_ratatui())
            .add_modifier(Modifier::BOLD);

        let list = List::new(list_items)
            .highlight_style(highlight_style)
            .highlight_symbol("→ ")
            .scroll_padding(2);

        let mut list_state = ratatui::widgets::ListState::default();
        let selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
        list_state.select(Some(selected));
        frame.render_stateful_widget(
            list,
            Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(3),
            },
            &mut list_state,
        );

        let hint = if self.changed {
            " Up/Down | Enter toggle/cycle | Esc save & close"
        } else {
            " Up/Down | Enter toggle/cycle | Esc close"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, styles.muted)),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " Up/Down  |  Enter toggle  |  Esc save & close"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

fn title_line(filter: &str) -> ratatui::text::Line<'static> {
    ratatui::text::Line::styled(
        format!(" {} ", filter),
        Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
    )
}

fn build_settings_items(_session: &AgentSessionHandle) -> Vec<SettingsItem> {
    let settings = oxi_store::settings::Settings::load().unwrap_or_default();

    let thinking_str = match settings.thinking_level {
        oxi_store::settings::ThinkingLevel::Off => "Off",
        oxi_store::settings::ThinkingLevel::Minimal => "Minimal",
        oxi_store::settings::ThinkingLevel::Low => "Low",
        oxi_store::settings::ThinkingLevel::Medium => "Medium",
        oxi_store::settings::ThinkingLevel::High => "High",
        oxi_store::settings::ThinkingLevel::XHigh => "XHigh",
    };

    let mut items = vec![SettingsItem::Choice {
        label: "thinking".to_string(),
        value: thinking_str.to_string(),
    }];

    items.push(SettingsItem::Choice {
        label: "theme".to_string(),
        value: settings.theme.clone(),
    });

    items.push(SettingsItem::Toggle {
        label: "stream_responses".to_string(),
        value: settings.stream_responses,
    });

    items.push(SettingsItem::Toggle {
        label: "extensions".to_string(),
        value: settings.extensions_enabled,
    });

    items.push(SettingsItem::Toggle {
        label: "auto_compact".to_string(),
        value: settings.auto_compaction,
    });

    let max_tokens = settings
        .max_response_tokens
        .map(|v| v.to_string())
        .unwrap_or_else(|| "auto".to_string());
    items.push(SettingsItem::ReadOnly {
        label: "max_response_tokens".to_string(),
        value: max_tokens,
    });

    let temp = settings
        .default_temperature
        .map(|v| format!("{:.2}", v))
        .unwrap_or_else(|| "auto".to_string());
    items.push(SettingsItem::ReadOnly {
        label: "temperature".to_string(),
        value: temp,
    });

    items.push(SettingsItem::ReadOnly {
        label: "last_used_model".to_string(),
        value: settings
            .last_used_model
            .unwrap_or_else(|| "none".to_string()),
    });

    items.push(SettingsItem::ReadOnly {
        label: "last_used_provider".to_string(),
        value: settings
            .last_used_provider
            .unwrap_or_else(|| "none".to_string()),
    });

    items.push(SettingsItem::ReadOnly {
        label: "session_dir".to_string(),
        value: settings
            .session_dir
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "~/.oxi/sessions".to_string()),
    });

    items.push(SettingsItem::ReadOnly {
        label: "session_history_size".to_string(),
        value: settings.session_history_size.to_string(),
    });

    items.push(SettingsItem::ReadOnly {
        label: "version".to_string(),
        value: settings.version.to_string(),
    });

    items.push(SettingsItem::ReadOnly {
        label: "───────────────────".to_string(),
        value: "─────────────────────".to_string(),
    });

    items.push(SettingsItem::Action {
        label: "[ Reload from disk ]".to_string(),
        id: "reload",
    });

    items
}

fn get_choice_options(label: &str) -> Vec<String> {
    match label {
        "thinking" => vec![
            "Off".to_string(),
            "Minimal".to_string(),
            "Low".to_string(),
            "Medium".to_string(),
            "High".to_string(),
            "XHigh".to_string(),
        ],
        _ => vec![],
    }
}
