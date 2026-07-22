//! Model-roles configuration overlay (`/roles`).
//!
//! Lists the built-in roles and their assigned models. `Enter` opens an inline
//! model picker for the selected role (models filtered to those with a stored
//! API key); choosing one assigns it — persisted to `settings.toml` AND applied
//! live to the shared role registry so routing picks it up on the next turn.
//! `d` clears a role.
//!
//! Self-contained: reads models/auth/settings from the global registries, so it
//! needs no `AppState` handle. This is the oxi-native counterpart of omp's
//! role-assignment picker, expressed as a focused single-responsibility overlay.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use super::{OverlayAction, OverlayComponent, centered_layout};
use crate::store::settings::Settings;
use oxi_sdk::ModelRole;
use oxi_tui_legacy::Theme;

#[derive(Debug)]
enum Mode {
    /// Browsing the role list.
    List,
    /// Picking a model for `role`.
    Picking {
        role: String,
        models: Vec<String>,
        state: ListState,
    },
}

/// `/roles` overlay: view and edit `model_roles` assignments.
#[derive(Debug)]
pub struct RolesConfigOverlay {
    roles: Vec<&'static str>,
    assignments: HashMap<String, String>,
    list_state: ListState,
    mode: Mode,
    notice: Option<String>,
}

/// Construct a boxed `/roles` overlay.
#[must_use]
pub fn roles_config_overlay() -> Box<dyn OverlayComponent> {
    Box::new(RolesConfigOverlay::new())
}

impl RolesConfigOverlay {
    fn new() -> Self {
        let settings = Settings::load().unwrap_or_default();
        let mut list_state = ListState::default();
        let roles: Vec<&'static str> = ModelRole::ALL.iter().map(|r| r.as_str()).collect();
        if !roles.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            roles,
            assignments: settings.model_roles.clone(),
            list_state,
            mode: Mode::List,
            notice: None,
        }
    }

    /// Models with a stored API key, as `provider/model` strings.
    fn fetch_models(&self) -> Vec<String> {
        let auth = crate::store::auth_storage::shared_auth_storage();
        let mut out: Vec<String> = oxi_sdk::get_all_models()
            .filter(|e| auth.get_api_key(e.provider).is_some())
            .map(|e| format!("{}/{}", e.provider, e.id))
            .collect();
        // Dynamic / runtime-registered models (custom providers such as
        // `zai-coding-plan`) live outside the static model_db — include them
        // so the picker is not empty for setups whose only provider is one.
        for m in oxi_sdk::dynamic_models() {
            if auth.get_api_key(&m.provider).is_some() {
                out.push(format!("{}/{}", m.provider, m.id));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Apply a new full role-map: persist to disk + rebuild the live registry.
    fn apply(&mut self, new_map: HashMap<String, String>) {
        if let Ok(mut s) = Settings::load() {
            s.model_roles = new_map.clone();
            let _ = s.save();
        }
        if let Some(reg) = oxi_sdk::live_role_registry() {
            *reg.write() = oxi_sdk::RoleRegistry::from_map(new_map.clone());
        }
        self.assignments = new_map;
    }

    fn assign(&mut self, role: &str, model: String) {
        let mut map = self.assignments.clone();
        map.insert(role.to_string(), model.clone());
        self.notice = Some(format!("{role} \u{2192} {model}"));
        self.apply(map);
    }

    fn clear(&mut self, role: &str) {
        let mut map = self.assignments.clone();
        if map.remove(role).is_some() {
            self.notice = Some(format!("{role} cleared"));
            self.apply(map);
        }
    }

    fn move_cursor(&mut self, list: bool, delta: i32) {
        let next = |cur: usize, len: usize| {
            if delta > 0 {
                (cur + 1).min(len.saturating_sub(1))
            } else {
                cur.saturating_sub(1)
            }
        };
        if list {
            let cur = self.list_state.selected().unwrap_or(0);
            self.list_state.select(Some(next(cur, self.roles.len())));
        } else if let Mode::Picking { models, state, .. } = &mut self.mode {
            let cur = state.selected().unwrap_or(0);
            state.select(Some(next(cur, models.len())));
        }
    }

    fn open_picker(&mut self, role: String) {
        let models = self.fetch_models();
        let mut state = ListState::default();
        if !models.is_empty() {
            state.select(Some(0));
        }
        self.notice = if models.is_empty() {
            Some("No models with a stored API key".to_string())
        } else {
            None
        };
        self.mode = Mode::Picking {
            role,
            models,
            state,
        };
    }
}

impl OverlayComponent for RolesConfigOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        if key.kind != KeyEventKind::Press {
            return OverlayAction::None;
        }

        // Compute an owned "pending" action from the current mode (read-only),
        // then apply it. This avoids borrow conflicts between matching
        // `self.mode` and calling `&mut self` methods (`fetch_models` /
        // `assign` / `clear`).
        enum Pending {
            None,
            Close,
            Move { list: bool, delta: i32 },
            OpenPicker { role: String },
            Clear { role: String },
            Assign { role: String, model: String },
            PickerBack,
        }

        let pending = match &self.mode {
            Mode::List => match key.code {
                KeyCode::Up => Pending::Move {
                    list: true,
                    delta: -1,
                },
                KeyCode::Down => Pending::Move {
                    list: true,
                    delta: 1,
                },
                KeyCode::Enter => self
                    .list_state
                    .selected()
                    .and_then(|i| self.roles.get(i).copied())
                    .map(|role| Pending::OpenPicker {
                        role: role.to_string(),
                    })
                    .unwrap_or(Pending::None),
                KeyCode::Char('d') => self
                    .list_state
                    .selected()
                    .and_then(|i| self.roles.get(i).copied())
                    .map(|role| Pending::Clear {
                        role: role.to_string(),
                    })
                    .unwrap_or(Pending::None),
                KeyCode::Esc => Pending::Close,
                _ => Pending::None,
            },
            Mode::Picking {
                role,
                models,
                state,
            } => {
                let role = role.clone();
                match key.code {
                    KeyCode::Up => Pending::Move {
                        list: false,
                        delta: -1,
                    },
                    KeyCode::Down if !models.is_empty() => Pending::Move {
                        list: false,
                        delta: 1,
                    },
                    KeyCode::Enter => match state.selected().and_then(|idx| models.get(idx)) {
                        Some(model) => Pending::Assign {
                            role,
                            model: model.clone(),
                        },
                        None => Pending::PickerBack,
                    },
                    KeyCode::Esc => Pending::PickerBack,
                    _ => Pending::None,
                }
            }
        };

        match pending {
            Pending::None => {}
            Pending::Close => return OverlayAction::Close,
            Pending::Move { list, delta } => self.move_cursor(list, delta),
            Pending::OpenPicker { role } => self.open_picker(role),
            Pending::Clear { role } => self.clear(&role),
            Pending::Assign { role, model } => {
                self.assign(&role, model);
                self.mode = Mode::List;
            }
            Pending::PickerBack => self.mode = Mode::List,
        }
        OverlayAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let styles = theme.to_styles();
        let popup = centered_layout(area, 0.70, 0.70);
        frame.render_widget(Clear, popup);

        let title = match &self.mode {
            Mode::List => " Model Roles (assign a model per role) ".to_string(),
            Mode::Picking { role, .. } => format!(" Pick model for {role} "),
        };
        let border = Block::default()
            .title(Line::styled(
                title,
                Style::default().bg(ratatui::style::Color::Rgb(0, 0, 0)),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border.inner(popup);
        frame.render_widget(border, popup);

        let items: Vec<ListItem> = match &self.mode {
            Mode::List => self
                .roles
                .iter()
                .map(|role| {
                    let assigned = self.assignments.get(*role).cloned().unwrap_or_default();
                    let row = if assigned.is_empty() {
                        format!("{:<10} \u{2014}", role)
                    } else {
                        format!("{:<10} {}", role, assigned)
                    };
                    ListItem::new(Span::styled(row, styles.normal))
                })
                .collect(),
            Mode::Picking { models, .. } => models
                .iter()
                .map(|m| ListItem::new(Span::styled(m.clone(), styles.normal)))
                .collect(),
        };

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(theme.colors.background)
                .bg(theme.colors.primary)
                .add_modifier(Modifier::BOLD),
        );
        let body = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };

        match &mut self.mode {
            Mode::List => frame.render_stateful_widget(list, body, &mut self.list_state),
            Mode::Picking { state, .. } => frame.render_stateful_widget(list, body, state),
        }

        let footer = match (&self.mode, &self.notice) {
            (Mode::Picking { .. }, _) => " Up/Down pick | Enter assign | Esc back ".to_string(),
            (_, Some(n)) => format!(" {n} | Up/Down | Enter assign | d clear | Esc close "),
            _ => " Up/Down | Enter assign | d clear | Esc close ".to_string(),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(footer, styles.muted)),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }

    fn hint(&self) -> &str {
        " Up/Down | Enter assign | d clear | Esc close"
    }
}
