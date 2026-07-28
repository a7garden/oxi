//! Interactive settings overlay for the `/settings` command.
//!
//! Displays all oxi settings with the ability to toggle/cycle values.

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, ListItem, Paragraph},
};

use super::{OverlayAction, OverlayComponent, centered_layout};
use crate::app::agent_session::AgentSessionHandle;
use oxi_tui_legacy::{GlyphSet, THEME_NAMES, ThemeRegistry};

type SharedAppState = Arc<Mutex<*mut crate::tui::app::AppState>>;

#[allow(clippy::arc_with_non_send_sync)]
fn share_state(state: &mut crate::tui::app::AppState) -> SharedAppState {
    Arc::new(Mutex::new(state as *mut _))
}

// ─────────────────────────────────────────────────────────────────────────
// Settings item
// ─────────────────────────────────────────────────────────────────────────

enum SettingsItem {
    Toggle {
        label: String,
        value: bool,
    },
    Choice {
        label: String,
        value: String,
        /// When `true`, the item is rendered greyed-out and Enter/Space
        /// are blocked (no cycling, optional warning notification).
        /// Used to gate the per-channel language Choice items behind
        /// the `language_policy` master toggle.
        disabled: bool,
    },
    ReadOnly {
        label: String,
        value: String,
    },
    Action {
        label: String,
        id: &'static str,
    },
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
    fn is_disabled(&self) -> bool {
        matches!(self, SettingsItem::Choice { disabled: true, .. })
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
    // Snapshot the theme registry from AppState so the overlay can
    // populate its theme choice list with both built-ins and any
    // custom themes loaded at startup. Cloning is cheap (it's just
    // two HashMaps of small Themes).
    let theme_registry: ThemeRegistry = app_state.theme_registry.clone();
    let all_items = build_settings_items(&session, &theme_registry);
    let item_count = all_items.len();
    Box::new(SettingsOverlay {
        all_items,
        filtered_indices: (0..item_count).collect(),
        selected: 0,
        filter: String::new(),
        session,
        app_state: share_state(app_state),
        changed: false,
        changed_labels: std::collections::HashSet::new(),
        theme_registry,
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
    /// Labels of items the user toggled/cycled in this session. Used to
    /// guard side-effecting live-apply steps (extensions reload,
    /// language policy rebuild) so they only fire when the user actually
    /// touched the relevant toggle — not on every save.
    changed_labels: std::collections::HashSet<String>,
    /// Snapshot of the theme registry at overlay-open time. The
    /// choice list for the `theme` item is built from this: custom
    /// themes first (if any), then the canonical [`THEME_NAMES`].
    /// Re-opening the overlay picks up any new custom themes.
    theme_registry: ThemeRegistry,
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

    /// Persist toggle/choice changes back to disk.
    fn persist_changes(&self) {
        if let Ok(mut settings) = crate::store::settings::Settings::load() {
            for item in &self.all_items {
                match item {
                    SettingsItem::Toggle { label, value } => match label.as_str() {
                        "extensions" => settings.extensions_enabled = *value,
                        "auto_compaction" => settings.auto_compaction = *value,
                        _ => {}
                    },
                    SettingsItem::Choice {
                        label,
                        value,
                        disabled,
                    } => {
                        // Gated Choices (disabled — `language_policy` is
                        // OFF) are skipped on persist. Since the live
                        // re-gate keeps `disabled` in sync with the master
                        // toggle, a disabled channel was never editable this
                        // session, so its in-overlay value equals the disk
                        // original. Skipping preserves that original rather
                        // than redundantly rewriting it.
                        if *disabled {
                            continue;
                        }
                        match label.as_str() {
                            "thinking" => {
                                settings.thinking_level = match value.as_str() {
                                    "Off" => crate::store::settings::ThinkingLevel::Off,
                                    "Minimal" => crate::store::settings::ThinkingLevel::Minimal,
                                    "Low" => crate::store::settings::ThinkingLevel::Low,
                                    "Medium" => crate::store::settings::ThinkingLevel::Medium,
                                    "High" => crate::store::settings::ThinkingLevel::High,
                                    "XHigh" => crate::store::settings::ThinkingLevel::XHigh,
                                    _ => settings.thinking_level,
                                };
                            }
                            "theme" => settings.theme = value.clone(),
                            "glyph" => {
                                // Cycle value is the preset label ("Unicode" /
                                // "ASCII" / "Nerd Font"); resolve via the label
                                // table so the spaced "Nerd Font" label matches.
                                settings.glyph_set = GlyphSet::ALL
                                    .iter()
                                    .find(|g| g.label() == value)
                                    .copied()
                                    .unwrap_or(GlyphSet::Unicode);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            let _ = settings.save();
        }
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
                    SettingsItem::Toggle { label, value } => {
                        *value = !*value;
                        self.changed = true;
                        self.changed_labels.insert(label.clone());
                        // Live re-gate: flipping `language_policy`
                        // immediately enables/disables all `language.*`
                        // channel Choices in the same overlay session.
                        // Without this the channels stay greyed-out and
                        // blocked until the overlay is closed and reopened
                        // (the `disabled` field was a build-time snapshot).
                        if label == "language_policy" {
                            let gated = !*value;
                            for other in &mut self.all_items {
                                if let SettingsItem::Choice {
                                    label: l, disabled, ..
                                } = other
                                    && l.starts_with("language.")
                                {
                                    *disabled = gated;
                                }
                            }
                        }
                    }
                    SettingsItem::Choice {
                        label,
                        value,
                        disabled,
                    } => {
                        // Disabled Choices (gated by language_policy=OFF)
                        // surface a guidance notification and block the cycle.
                        if *disabled {
                            if let Ok(ptr) = self.app_state.lock() {
                                // SAFETY: Mutex lock guarantees exclusive access.
                                unsafe {
                                    if let Some(ref mut app) = (*ptr).as_mut() {
                                        app.add_notification(
                                            "Enable language_policy first.".to_string(),
                                            crate::tui::app::NotificationKind::Warning,
                                        );
                                    }
                                }
                            }
                            return OverlayAction::None;
                        }
                        let options = get_choice_options(label, &self.theme_registry);
                        if let Some(pos) = options.iter().position(|o| o == value) {
                            let next = (pos + 1) % options.len();
                            *value = options[next].clone();
                            self.changed = true;
                            self.changed_labels.insert(label.clone());
                        }
                    }
                    SettingsItem::Action { id, .. } => {
                        if *id == "reload" {
                            self.all_items =
                                build_settings_items(&self.session, &self.theme_registry);
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
                        } else if *id == "router_setup" {
                            let auth = crate::store::auth_storage::shared_auth_storage();
                            // Read models via the catalog port (sync API)
                            // when the AppState is available, else fall back
                            // to legacy global state.
                            let all_pairs: Vec<(String, String)> =
                                if let Ok(ptr) = self.app_state.lock() {
                                    // SAFETY: `ptr` points at the AppState
                                    // borrowed immutably for the duration of
                                    // this block. We only read `catalog`
                                    // and do not mutate AppState.
                                    let state = unsafe { &**ptr };
                                    if let Some(ref cat) = state.catalog {
                                        cat.search_sync("")
                                            .into_iter()
                                            .map(|e| (e.provider, e.model_id))
                                            .collect()
                                    } else {
                                        oxi_sdk::get_all_models()
                                            .map(|e| (e.provider.to_string(), e.id.to_string()))
                                            .collect()
                                    }
                                } else {
                                    oxi_sdk::get_all_models()
                                        .map(|e| (e.provider.to_string(), e.id.to_string()))
                                        .collect()
                                };
                            let models: Vec<String> = all_pairs
                                .into_iter()
                                .filter(|(provider, _)| auth.get_api_key(provider).is_some())
                                .map(|(provider, model_id)| format!("{}/{}", provider, model_id))
                                .collect();
                            return OverlayAction::OpenRouterSetup {
                                initial: crate::tui::overlay::RouterSetupData {
                                    profile_name: "auto".to_string(),
                                    ..Default::default()
                                },
                                models,
                            };
                        }
                    }
                    SettingsItem::ReadOnly { .. } => {}
                }
            }
            KeyCode::Esc => {
                if self.changed {
                    self.persist_changes();
                    // v6: auto-apply changes to the live agent so the next
                    // turn picks them up. `rebuild_system_prompt()` fresh-loads
                    // `Settings` from disk (covering both this overlay's
                    // persist and any external `settings.toml` edits) and
                    // calls `Agent::set_system_prompt`. No live LLM call is
                    // interrupted — `set_system_prompt` only affects future
                    // turns.
                    self.session.rebuild_system_prompt();
                    if let Ok(ptr) = self.app_state.lock() {
                        // SAFETY: We hold the Mutex lock, guaranteeing exclusive access.
                        // The raw pointer is valid for the lock's lifetime.
                        unsafe {
                            if let Some(ref mut app) = (*ptr).as_mut() {
                                // Appearance (glyph set / theme) — only flag
                                // for rebuild if the user actually changed a
                                // relevant item, not on every save.
                                if self.changed_labels.contains("theme")
                                    || self.changed_labels.contains("glyph")
                                {
                                    app.appearance_needs_reload = true;
                                }
                                // Only fire side-effecting live-apply steps
                                // for toggles the user actually touched in
                                // this session — not on every save.
                                if self.changed_labels.contains("extensions") {
                                    let fresh = crate::store::settings::Settings::load()
                                        .unwrap_or_default();
                                    sync_wasm_extensions(
                                        app,
                                        &self.session,
                                        fresh.extensions_enabled,
                                    );
                                }
                                app.add_notification(
                                    "Settings saved and applied.".to_string(),
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

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &oxi_tui_legacy::Theme) {
        use ratatui::widgets::List;
        let styles = theme.to_styles();
        let filter_text = if self.filter.is_empty() {
            "Settings".to_string()
        } else {
            format!("Settings: {}", self.filter)
        };

        let popup = centered_layout(area, 0.75, 0.8);
        frame.render_widget(Clear, popup);
        frame
            .buffer_mut()
            .set_style(popup, Style::default().bg(theme.colors.panel_bg));
        let border_block = Block::default()
            .title(title_line(&filter_text))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.colors.border));
        let inner = border_block.inner(popup);
        frame.render_widget(border_block, popup);

        // Header
        frame.render_widget(
            Paragraph::new(Span::styled(
                " KEY                   VALUE                    ",
                Style::default()
                    .fg(theme.colors.muted)
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
                // Disabled Choices (gated by language_policy=OFF) render
                // dimmer than the muted ReadOnly color to signal that
                // the value is gated, not just informational.
                let style = if item.is_disabled() {
                    Style::default()
                        .fg(theme.colors.muted)
                        .add_modifier(Modifier::DIM)
                } else if item.is_editable() {
                    styles.normal
                } else {
                    Style::default().fg(theme.colors.muted)
                };
                ListItem::new(Span::styled(format!("{} {}", label, value), style))
            })
            .collect();

        let highlight_style = Style::default()
            .fg(theme.colors.background)
            .bg(theme.colors.primary)
            .add_modifier(Modifier::BOLD);

        let list = List::new(list_items)
            .highlight_style(highlight_style)
            .highlight_symbol(theme.symbols.cursor)
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
/// Reload (or unload) WASM extensions to match `extensions_enabled`.
///
/// Shared by the `/settings` overlay Esc handler and the `/reload` slash
/// command so both converge on the same live behavior.
pub(crate) fn sync_wasm_extensions(
    state: &mut crate::tui::app::AppState,
    session: &crate::app::agent_session::AgentSession,
    extensions_enabled: bool,
) {
    use crate::tui::app::NotificationKind;
    if extensions_enabled {
        let cwd_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let wasm_paths = crate::extensions::WasmExtensionManager::discover(&cwd_path);
        if !wasm_paths.is_empty() {
            let mut mgr = crate::extensions::WasmExtensionManager::new();
            let (loaded, errors) = mgr.load_all(&wasm_paths);
            let loaded_count = loaded.len();
            let error_count = errors.len();
            if !mgr.is_empty() {
                // Unregister old WASM tools
                let tools = session.agent_ref().tools();
                let old_names: Vec<String> = if let Some(ref old_ext) = state.wasm_ext {
                    old_ext
                        .all_tool_defs()
                        .iter()
                        .map(|d| d.name.clone())
                        .collect()
                } else {
                    vec![]
                };
                for name in &old_names {
                    tools.unregister(name);
                }
                // Register new WASM tools
                let arc_mgr = std::sync::Arc::new(mgr);
                for tool_def in arc_mgr.all_tool_defs() {
                    let wasm_tool = crate::extensions::WasmTool::new(
                        arc_mgr.clone(),
                        tool_def.name.clone(),
                        tool_def.description.clone(),
                        tool_def.schema.clone(),
                    );
                    tools.register(wasm_tool);
                }
                state.wasm_ext = Some(arc_mgr);
                state.add_notification(
                    format!("{} loaded, {} error(s)", loaded_count, error_count),
                    NotificationKind::Info,
                );
            } else {
                state.wasm_ext = None;
            }
        } else {
            state.wasm_ext = None;
        }
    } else {
        state.wasm_ext = None;
    }
    // Re-sync the long-lived slash registry so `/help` and Tab completion
    // reflect the reloaded extensions. Without this, dispatch (per-call
    // local registry) keeps working but `state.slash_registry` stays stale
    // after a `/settings` toggle or `/reload`. Same fix as the initial
    // wiring in app.rs.
    state
        .slash_registry
        .sync_extensions(state.wasm_ext.as_ref());
}

fn build_settings_items(
    _session: &AgentSessionHandle,
    _theme_registry: &ThemeRegistry,
) -> Vec<SettingsItem> {
    let settings = crate::store::settings::Settings::load().unwrap_or_default();

    let thinking_str = match settings.thinking_level {
        crate::store::settings::ThinkingLevel::Off => "Off",
        crate::store::settings::ThinkingLevel::Minimal => "Minimal",
        crate::store::settings::ThinkingLevel::Low => "Low",
        crate::store::settings::ThinkingLevel::Medium => "Medium",
        crate::store::settings::ThinkingLevel::High => "High",
        crate::store::settings::ThinkingLevel::XHigh => "XHigh",
    };

    let mut items = vec![SettingsItem::Choice {
        label: "thinking".to_string(),
        value: thinking_str.to_string(),
        disabled: false,
    }];

    items.push(SettingsItem::Choice {
        label: "theme".to_string(),
        value: settings.get_theme_name(),
        disabled: false,
    });
    items.push(SettingsItem::Choice {
        label: "glyph".to_string(),
        value: settings.glyph_set.label().to_string(),
        disabled: false,
    });

    items.push(SettingsItem::Toggle {
        label: "extensions".to_string(),
        value: settings.extensions_enabled,
    });

    items.push(SettingsItem::Toggle {
        label: "auto_compaction".to_string(),
        value: settings.auto_compaction,
    });

    // ── Info ─────────────────────────────────────────────────────────────
    items.push(SettingsItem::ReadOnly {
        label: "───────────────────".to_string(),
        value: "─────────────────────".to_string(),
    });

    let max_tokens = settings
        .effective_max_tokens()
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

fn get_choice_options(label: &str, theme_registry: &ThemeRegistry) -> Vec<String> {
    match label {
        "thinking" => vec![
            "Off".to_string(),
            "Minimal".to_string(),
            "Low".to_string(),
            "Medium".to_string(),
            "High".to_string(),
            "XHigh".to_string(),
        ],
        // Custom themes first (lower-cased; insertion order is unspecified),
        // then the canonical [`THEME_NAMES`] (oxi_dark first). If a custom
        // theme shares a name with a built-in, the custom one wins on
        // resolution — but we still show the built-in here so the user can
        // cycle back to it. (When cycling by Enter, both keys are valid;
        // `ThemeRegistry::resolve` always prefers custom on collision.)
        "theme" => {
            let mut opts: Vec<String> = theme_registry.custom_names();
            opts.extend(THEME_NAMES.iter().map(|n| n.to_string()));
            opts
        }
        // Glyph set presets cycle in GlyphSet::ALL order (Unicode first).
        "glyph" => GlyphSet::ALL
            .iter()
            .map(|g| g.label().to_string())
            .collect(),
        _ => vec![],
    }
}
