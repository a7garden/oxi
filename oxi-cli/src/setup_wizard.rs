//! Interactive setup wizard for oxi (`oxi setup`).
//!
//! Provides a TUI-based configuration experience for:
//! 1. Provider API key management
//! 2. Default model selection
//! 3. Theme selection
//! 4. Summary and persistence
//!
//! Uses crossterm + ratatui for terminal control with proper raw-mode
//! restoration on panic or early exit.


use std::sync::Arc;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};

use std::io;
use std::path::PathBuf;

// ── Provider entry (runtime state) ──────────────────────────────────────────

// ── Provider entry (runtime state) ──────────────────────────────────────────

/// Runtime state for a single provider entry in the wizard list.
#[derive(Clone)]
struct ProviderEntry {
    name: String,
    has_key: bool,
    key_masked: String,
    is_custom: bool,
    base_url: Option<String>,
}

// ── Input mode ──────────────────────────────────────────────────────────────

/// What the wizard is currently editing.
#[derive(Clone)]
enum InputMode {
    /// Normal browsing / selection
    Normal,
    /// Editing an API key for a provider
    EditingApiKey {
        provider_name: String,
        field_text: String,
    },
    /// Adding a new custom provider (multi-field form)
    AddingCustom {
        fields: [String; 3], // [name, base_url, api_key]
        active_field: usize,
    },
}

// ── Wizard state ────────────────────────────────────────────────────────────

/// Top-level wizard state.
struct WizardState {
    /// Current step: 0=providers, 1=model, 2=theme, 3=done
    step: usize,
    /// Provider entries (presets + custom)
    providers: Vec<ProviderEntry>,
    /// Currently selected index in the provider list
    provider_selected: usize,
    /// List state for ratatui
    provider_list_state: ListState,
    /// Input mode
    input_mode: InputMode,
    /// Model entries for step 2
    models: Vec<ModelEntry>,
    /// Currently selected model index
    model_selected: usize,
    /// Model search filter
    model_filter: String,
    /// Whether we're typing in the model search
    model_searching: bool,
    /// Theme names for step 3
    themes: Vec<String>,
    /// Currently selected theme index
    theme_selected: usize,
    /// Theme list state
    theme_list_state: ListState,
    /// Auth storage path
    auth_path: PathBuf,
    /// Settings path
    settings_path: PathBuf,
}

/// A model entry for display.
#[derive(Clone)]
struct ModelEntry {
    id: String,
    provider: String,
    context_window: u32,
}

// ── Masking helper ──────────────────────────────────────────────────────────

/// Mask an API key for display: show first 6 and last 4 chars, rest asterisks.
fn mask_key(key: &str) -> String {
    if key.len() <= 10 {
        "*".repeat(key.len())
    } else {
        format!("{}...{}", &key[..6], &key[key.len() - 4..])
    }
}

// ── Load provider state ─────────────────────────────────────────────────────

/// Build the initial provider list from builtins + stored keys + custom providers.
fn load_providers(auth_store: &crate::store::auth_storage::AuthStorage) -> Vec<ProviderEntry> {
    let mut entries = Vec::new();

    for builtin in oxi_sdk::get_builtin_providers() {
        let key = auth_store.get_api_key(builtin.name);

        let (has_key, key_masked) = match &key {
            Some(k) => (true, mask_key(k)),
            None => (false, String::new()),
        };

        let base_url = builtin.base_url;
        entries.push(ProviderEntry {
            name: builtin.name.to_string(),
            has_key,
            key_masked,
            is_custom: false,
            base_url: if base_url.is_empty() {
                None
            } else {
                Some(base_url.to_string())
            },
        });
    }

    // Add custom providers from settings that aren't already in builtins
    if let Ok(settings) = crate::store::settings::Settings::load() {
        for cp in &settings.custom_providers {
            if oxi_sdk::is_builtin_provider(&cp.name) {
                continue;
            }
            let actual_key = auth_store.get_api_key(&cp.name);

            let (has_key, key_masked) = match &actual_key {
                Some(k) => (true, mask_key(k)),
                None => (false, String::new()),
            };

            entries.push(ProviderEntry {
                name: cp.name.clone(),
                has_key,
                key_masked,
                is_custom: true,
                base_url: Some(cp.base_url.clone()),
            });
        }
    }

    entries
}

// ── Load model list ────────────────────────────────────────────────────────

/// Build the model list from the static model database + dynamic cache.
fn load_models() -> Vec<ModelEntry> {
    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Dynamic models from settings cache (fetched from /models endpoints)
    if let Ok(settings) = crate::store::settings::Settings::load() {
        for (provider, model_ids) in &settings.dynamic_models {
            for id in model_ids {
                let key = format!("{}/{}", provider, id);
                if seen.insert(key.clone()) {
                    // Try to get context_window from model_db, default 128_000
                    let ctx = oxi_sdk::get_model_entry(provider, id)
                        .map(|e| e.context_window)
                        .unwrap_or(128_000);
                    models.push(ModelEntry {
                        id: id.clone(),
                        provider: provider.clone(),
                        context_window: ctx,
                    });
                }
            }
        }
    }

    // 2. Static models from model_db
    for entry in oxi_sdk::get_all_models() {
        let key = format!("{}/{}", entry.provider, entry.id);
        if seen.insert(key) {
            models.push(ModelEntry {
                id: entry.id.to_string(),
                provider: entry.provider.to_string(),
                context_window: entry.context_window,
            });
        }
    }

    models
}

// ── Fetch and cache dynamic models ─────────────────────────────────────────

/// Try to fetch models from a provider's `/models` endpoint and cache them in settings.
///
/// Only works for OpenAI-compatible providers that have a `base_url`.
/// Non-OpenAI-compatible providers are silently skipped.
/// On failure, logs a warning and keeps the existing cache (if any).
fn fetch_and_cache_models(provider_name: &str, providers: &[ProviderEntry]) {
    // Resolve base_url for this provider
    let base_url = providers
        .iter()
        .find(|p| p.name == provider_name)
        .and_then(|p| p.base_url.clone())
        .or_else(|| oxi_sdk::get_provider_base_url(provider_name).map(|s| s.to_string()));

    let base_url = match base_url {
        Some(url) if !url.is_empty() => url,
        _ => {
            tracing::debug!(
                "Skipping dynamic model fetch for '{}': no base_url",
                provider_name
            );
            return;
        }
    };

    // Get the API key from auth storage
    let auth_store = crate::store::auth_storage::shared_auth_storage();
    let api_key = match auth_store.get_api_key(provider_name) {
        Some(key) => key,
        None => {
            tracing::debug!(
                "Skipping dynamic model fetch for '{}': no API key",
                provider_name
            );
            return;
        }
    };

    // Only fetch for OpenAI-compatible providers (api = openai-completions or openai-responses)
    let api_type = oxi_sdk::get_provider_api(provider_name);
    let is_openai_compatible = api_type.is_none_or(|api| {
        matches!(
            api,
            oxi_sdk::Api::OpenAiCompletions | oxi_sdk::Api::OpenAiResponses
        )
    });

    if !is_openai_compatible {
        tracing::debug!(
            "Skipping dynamic model fetch for '{}': not OpenAI-compatible",
            provider_name
        );
        return;
    }

    tracing::info!(
        "Fetching models from {}/models for provider '{}'...",
        base_url,
        provider_name
    );

    match oxi_sdk::fetch_models_blocking(&base_url, &api_key) {
        Ok(model_ids) => {
            tracing::info!(
                "Fetched {} models from provider '{}'",
                model_ids.len(),
                provider_name
            );

            // Update settings cache
            if let Ok(mut settings) = crate::store::settings::Settings::load() {
                settings
                    .dynamic_models
                    .insert(provider_name.to_string(), model_ids);
                if let Err(e) = settings.save() {
                    tracing::warn!("Failed to save dynamic models cache: {}", e);
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to fetch models from provider '{}': {}. \
                 Falling back to static model list.",
                provider_name,
                e
            );
        }
    }
}

// ── Load theme list ─────────────────────────────────────────────────────────

fn load_themes() -> Vec<String> {
    // Built-in theme names from oxi-cli theme system
    vec![
        "oxi_dark".to_string(),
        "oxi_light".to_string(),
        "nord".to_string(),
        "catppuccin".to_string(),
        "github_dark".to_string(),
        "monokai".to_string(),
    ]
}

// ── Save auth keys ──────────────────────────────────────────────────────────

// ── Save settings ───────────────────────────────────────────────────────────

/// Save the selected model and theme to settings.
fn save_settings(
    model_id: &str,
    theme_name: &str,
    custom_base_urls: &[(String, String)],
) -> Result<()> {
    let mut settings = crate::store::settings::Settings::load().unwrap_or_default();

    // Split "provider/model" and store as last_used
    if let Some((provider, model_name)) = model_id.split_once('/') {
        settings.last_used_provider = Some(provider.to_string());
        settings.last_used_model = Some(model_name.to_string());
    } else {
        settings.last_used_model = Some(model_id.to_string());
    }
    settings.theme = theme_name.to_string();

    // Ensure custom providers with base_url are registered
    for (name, base_url) in custom_base_urls {
        let already_exists = settings.custom_providers.iter().any(|cp| cp.name == *name);
        if !already_exists {
            settings
                .custom_providers
                .push(crate::store::settings::CustomProvider {
                    name: name.clone(),
                    base_url: base_url.clone(),
                    api_key_env: format!("{}_API_KEY", name.to_uppercase().replace('-', "_")),
                    api: "openai-completions".to_string(),
                });
        }
    }

    settings.save()?;
    Ok(())
}

// ── Draw functions ──────────────────────────────────────────────────────────

fn draw_wizard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut WizardState,
) -> Result<()> {
    terminal.draw(|f| {
        let size = f.area();

        // Create outer layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title bar
                Constraint::Min(10),   // Content
                Constraint::Length(2), // Footer
            ])
            .split(size);

        // Title bar
        let title = Paragraph::new(Line::from(vec![
            Span::styled(" 🦊 ", Style::default().fg(Color::Rgb(255, 165, 0))),
            Span::styled(
                "oxi Setup Wizard",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]))
        .block(Block::default().borders(Borders::TOP));
        f.render_widget(title, chunks[0]);

        // Content depends on step
        match state.step {
            0 => draw_provider_step(f, state, chunks[1]),
            1 => draw_model_step(f, state, chunks[1]),
            2 => draw_theme_step(f, state, chunks[1]),
            3 => draw_done_step(f, state, chunks[1]),
            _ => {}
        }

        // Footer
        let footer_text = match state.step {
            0 => match &state.input_mode {
                InputMode::Normal => {
                    "  ↑/↓ navigate · Enter: enter/change API key · d: delete · →: next · q: quit"
                        .to_string()
                }
                InputMode::EditingApiKey { .. } => "  Enter: save · Esc: cancel".to_string(),
                InputMode::AddingCustom { .. } => {
                    "  Tab: next field · Enter: save · Esc: cancel".to_string()
                }
            },
            1 => {
                if state.model_searching {
                    "  Type: search · Esc: close search · Enter: select · ←: previous".to_string()
                } else {
                    "  ↑/↓ navigate · /: search · Enter: select · ←: previous".to_string()
                }
            }
            2 => "  ↑/↓ navigate · Enter: select · ←: previous".to_string(),
            3 => "  Enter: quit".to_string(),
            _ => String::new(),
        };
        let footer = Paragraph::new(Line::from(Span::styled(
            footer_text,
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(footer, chunks[2]);
    })?;

    Ok(())
}

fn draw_provider_step(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    match &state.input_mode {
        InputMode::Normal => draw_provider_list(f, state, area),
        InputMode::EditingApiKey {
            provider_name,
            field_text,
        } => draw_api_key_dialog(f, provider_name, field_text, area),
        InputMode::AddingCustom {
            fields,
            active_field,
        } => draw_custom_provider_dialog(f, fields, *active_field, area),
    }
}

fn draw_provider_list(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    let step_indicator = build_step_indicator(state.step);

    let items: Vec<ListItem> = state
        .providers
        .iter()
        .map(|p| {
            let check = if p.has_key { "[x]" } else { "[ ]" };
            let key_info = if p.has_key {
                format!("API key: {}", p.key_masked)
            } else {
                "No API key".to_string()
            };
            let custom_tag = if p.is_custom { " (custom)" } else { "" };

            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", check),
                    Style::default().fg(if p.has_key {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::styled(
                    format!("{:<14}", p.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("[{}]", key_info),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(custom_tag.to_string(), Style::default().fg(Color::Yellow)),
            ]);
            ListItem::new(line)
        })
        .collect();

    // Add custom provider entry
    let add_custom = ListItem::new(Line::from(vec![
        Span::styled("   + ", Style::default().fg(Color::Cyan)),
        Span::styled("Add custom provider...", Style::default().fg(Color::Cyan)),
    ]));

    let mut all_items = items;
    all_items.push(add_custom);

    let list = List::new(all_items)
        .block(
            Block::default()
                .borders(Borders::NONE)
                .title(step_indicator),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    // Update list state selection
    state
        .provider_list_state
        .select(Some(state.provider_selected));
    f.render_stateful_widget(list, area, &mut state.provider_list_state);
}

fn draw_api_key_dialog(f: &mut ratatui::Frame, provider_name: &str, field_text: &str, area: Rect) {
    // Center the dialog
    let dialog_height = 7u16;
    let dialog_width = std::cmp::min(area.width, 60);
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;

    let dialog_area = Rect::new(area.x + x, area.y + y, dialog_width, dialog_height);

    let display_text = if field_text.is_empty() {
        String::new()
    } else {
        "*".repeat(field_text.len())
    };

    let paragraphs = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  API Key: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("[{:<width$}]", display_text, width = 30),
                Style::default(),
            ),
            if field_text.is_empty() {
                Span::styled("Enter your API key", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            },
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} API Key ", provider_name));

    let para = Paragraph::new(paragraphs).block(block);
    f.render_widget(para, dialog_area);
}

fn draw_custom_provider_dialog(
    f: &mut ratatui::Frame,
    fields: &[String; 3],
    active_field: usize,
    area: Rect,
) {
    let dialog_height = 9u16;
    let dialog_width = std::cmp::min(area.width, 60);
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;

    let dialog_area = Rect::new(area.x + x, area.y + y, dialog_width, dialog_height);

    let field_labels = ["Name", "Base URL", "API Key"];
    let lines: Vec<Line> = std::iter::once(Line::from(""))
        .chain(field_labels.iter().enumerate().map(|(i, label)| {
            let display = if i == 2 && !fields[i].is_empty() {
                "*".repeat(fields[i].len())
            } else {
                fields[i].clone()
            };
            let is_active = i == active_field;
            let style = if is_active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(format!("  {:<10}", format!("{}:", label)), style),
                Span::styled(format!("[{:<width$}]", display, width = 35), style),
                if is_active && fields[i].is_empty() {
                    Span::styled("<enter>", Style::default().fg(Color::DarkGray))
                } else {
                    Span::raw("")
                },
            ])
        }))
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add Custom Provider ");

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, dialog_area);
}

fn draw_model_step(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    let step_indicator = build_step_indicator(state.step);

    // Filter models
    let filtered: Vec<&ModelEntry> = if state.model_filter.is_empty() {
        state.models.iter().collect()
    } else {
        let filter = state.model_filter.to_lowercase();
        state
            .models
            .iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&filter) || m.provider.to_lowercase().contains(&filter)
            })
            .collect()
    };

    let mut lines: Vec<Line> = Vec::new();

    if state.model_searching {
        lines.push(Line::from(vec![
            Span::styled("  Search: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                &state.model_filter,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("_"),
        ]));
    }

    lines.push(Line::from(""));

    for m in &filtered {
        let ctx_str = if m.context_window >= 1_000_000 {
            format!("{}M ctx", m.context_window / 1_000_000)
        } else {
            format!("{}K ctx", m.context_window / 1_000)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<40}", m.id), Style::default()),
            Span::styled(
                format!("({})", m.provider),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!(", {}", ctx_str),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    let block = Block::default()
        .borders(Borders::NONE)
        .title(step_indicator);

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn draw_theme_step(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    let step_indicator = build_step_indicator(state.step);

    let items: Vec<ListItem> = state
        .themes
        .iter()
        .map(|t| ListItem::new(Line::from(format!("  {}", t))))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::NONE)
                .title(step_indicator),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    state.theme_list_state.select(Some(state.theme_selected));
    f.render_stateful_widget(list, area, &mut state.theme_list_state);
}

fn draw_done_step(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    let settings_path_display = state.settings_path.display().to_string();
    let auth_path_display = state.auth_path.display().to_string();

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Settings saved!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Settings file: {}", settings_path_display),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!("  Auth file: {}", auth_path_display),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Run 'oxi' to start.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];

    let block = Block::default().borders(Borders::NONE);
    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn build_step_indicator(current_step: usize) -> Line<'static> {
    let steps = [
        ("1. Provider Setup", 0),
        ("2. Default Model", 1),
        ("3. Theme", 2),
        ("4. Done", 3),
    ];

    let spans: Vec<Span> = steps
        .iter()
        .flat_map(|(label, step)| {
            let style = if *step == current_step {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Cyan)
            } else if *step < current_step {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            vec![Span::styled(format!("  {}", label), style), Span::raw(" ")]
        })
        .collect();

    Line::from(spans)
}

// ── Event handling ──────────────────────────────────────────────────────────

fn handle_event(
    state: &mut WizardState,
    event: Event,
    auth_store: &crate::store::auth_storage::AuthStorage,
) -> Result<bool> {
    match state.step {
        0 => handle_provider_event(state, event, auth_store),
        1 => handle_model_event(state, event),
        2 => handle_theme_event(state, event),
        3 => handle_done_event(event),
        _ => Ok(false),
    }
}

fn handle_provider_event(
    state: &mut WizardState,
    event: Event,
    auth_store: &crate::store::auth_storage::AuthStorage,
) -> Result<bool> {
    match &mut state.input_mode {
        InputMode::Normal => {
            if let Event::Key(key) = event {
                match key.code {
                    KeyCode::Up if state.provider_selected > 0 => {
                        state.provider_selected -= 1;
                    }
                    KeyCode::Down => {
                        // +1 for "add custom" row
                        let max = state.providers.len();
                        if state.provider_selected < max {
                            state.provider_selected += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if state.provider_selected == state.providers.len() {
                            // Add custom provider
                            state.input_mode = InputMode::AddingCustom {
                                fields: [String::new(), String::new(), String::new()],
                                active_field: 0,
                            };
                        } else {
                            // Edit API key
                            let name = state.providers[state.provider_selected].name.clone();
                            state.input_mode = InputMode::EditingApiKey {
                                provider_name: name,
                                field_text: String::new(),
                            };
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Delete
                        if state.provider_selected < state.providers.len() =>
                    {
                        let name = state.providers[state.provider_selected].name.clone();
                        auth_store.remove(&name);
                        state.providers[state.provider_selected].has_key = false;
                        state.providers[state.provider_selected].key_masked = String::new();
                    }
                    KeyCode::Right => {
                        state.step = 1;
                    }
                    KeyCode::Char('q') => {
                        return Ok(true); // quit
                    }
                    _ => {}
                }
            }
        }
        InputMode::EditingApiKey {
            provider_name,
            field_text,
        } => {
            if let Event::Key(key) = event {
                match key.code {
                    KeyCode::Esc => {
                        state.input_mode = InputMode::Normal;
                    }
                    KeyCode::Enter => {
                        if !field_text.is_empty() {
                            auth_store.set_api_key(provider_name, field_text.clone());

                            // Update the provider entry
                            if let Some(entry) = state
                                .providers
                                .iter_mut()
                                .find(|p| p.name == *provider_name)
                            {
                                entry.has_key = true;
                                entry.key_masked = mask_key(field_text);
                            }

                            // Try to fetch models dynamically from the provider's /models endpoint
                            fetch_and_cache_models(provider_name, &state.providers);

                            // Refresh the model list to include newly fetched models
                            state.models = load_models();
                        }
                        state.input_mode = InputMode::Normal;
                    }
                    KeyCode::Backspace => {
                        field_text.pop();
                    }
                    KeyCode::Char(c) => {
                        field_text.push(c);
                    }
                    _ => {}
                }
            }
        }
        InputMode::AddingCustom {
            fields,
            active_field,
        } => {
            if let Event::Key(key) = event {
                match key.code {
                    KeyCode::Esc => {
                        state.input_mode = InputMode::Normal;
                    }
                    KeyCode::Tab => {
                        *active_field = (*active_field + 1) % 3;
                    }
                    KeyCode::BackTab => {
                        *active_field = (*active_field + 2) % 3;
                    }
                    KeyCode::Enter => {
                        let name = fields[0].trim().to_string();
                        let base_url = fields[1].trim().to_string();
                        let api_key = fields[2].trim().to_string();

                        if !name.is_empty() && !base_url.is_empty() {
                            // Save API key
                            if !api_key.is_empty() {
                                auth_store.set_api_key(&name, api_key.clone());
                            }

                            // Add to provider list
                            let (has_key, key_masked) = if !api_key.is_empty() {
                                (true, mask_key(&api_key))
                            } else {
                                (false, String::new())
                            };

                            state.providers.push(ProviderEntry {
                                name: name.clone(),
                                has_key,
                                key_masked,
                                is_custom: true,
                                base_url: Some(base_url),
                            });

                            // Try to fetch models from this custom provider
                            if !api_key.is_empty() {
                                fetch_and_cache_models(&name, &state.providers);
                                state.models = load_models();
                            }

                            // Move back to normal
                            state.input_mode = InputMode::Normal;
                        }
                    }
                    KeyCode::Backspace => {
                        fields[*active_field].pop();
                    }
                    KeyCode::Char(c) => {
                        fields[*active_field].push(c);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(false)
}

fn handle_model_event(state: &mut WizardState, event: Event) -> Result<bool> {
    if let Event::Key(key) = event {
        if state.model_searching {
            match key.code {
                KeyCode::Esc => {
                    state.model_searching = false;
                    state.model_filter.clear();
                }
                KeyCode::Enter => {
                    // Select the first filtered model
                    state.model_searching = false;
                    select_filtered_model(state);
                }
                KeyCode::Backspace => {
                    state.model_filter.pop();
                }
                KeyCode::Char(c) => {
                    state.model_filter.push(c);
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Up if state.model_selected > 0 => {
                    state.model_selected -= 1;
                }
                KeyCode::Down if state.model_selected + 1 < state.models.len() => {
                    state.model_selected += 1;
                }
                KeyCode::Char('/') => {
                    state.model_searching = true;
                    state.model_filter.clear();
                }
                KeyCode::Enter => {
                    // Move to theme step
                    state.step = 2;
                }
                KeyCode::Left => {
                    state.step = 0;
                }
                _ => {}
            }
        }
    }
    Ok(false)
}

fn select_filtered_model(state: &mut WizardState) {
    if state.model_filter.is_empty() {
        state.step = 2;
        return;
    }
    let filter = state.model_filter.to_lowercase();
    if let Some(idx) = state.models.iter().position(|m| {
        m.id.to_lowercase().contains(&filter) || m.provider.to_lowercase().contains(&filter)
    }) {
        state.model_selected = idx;
    }
    state.step = 2;
}

fn handle_theme_event(state: &mut WizardState, event: Event) -> Result<bool> {
    if let Event::Key(key) = event {
        match key.code {
            KeyCode::Up if state.theme_selected > 0 => {
                state.theme_selected -= 1;
            }
            KeyCode::Down if state.theme_selected + 1 < state.themes.len() => {
                state.theme_selected += 1;
            }
            KeyCode::Enter => {
                // Save everything and go to done
                finish_setup(state)?;
                state.step = 3;
            }
            KeyCode::Left => {
                state.step = 1;
            }
            _ => {}
        }
    }
    Ok(false)
}

fn handle_done_event(event: Event) -> Result<bool> {
    if let Event::Key(key) = event {
        match key.code {
            KeyCode::Enter | KeyCode::Char('q') => {
                return Ok(true); // quit
            }
            _ => {}
        }
    }
    Ok(false)
}

// ── Finish: persist all selections ──────────────────────────────────────────

fn finish_setup(state: &mut WizardState) -> Result<()> {
    // Get selected model
    let model_id = state
        .models
        .get(state.model_selected)
        .map(|m| format!("{}/{}", m.provider, m.id))
        .unwrap_or_default();

    // Get selected theme
    let theme_name = state
        .themes
        .get(state.theme_selected)
        .cloned()
        .unwrap_or_else(|| "oxi_dark".to_string());

    // Collect custom provider base URLs
    let custom_base_urls: Vec<(String, String)> = state
        .providers
        .iter()
        .filter_map(|p| {
            if p.is_custom {
                p.base_url.as_ref().map(|url| (p.name.clone(), url.clone()))
            } else {
                None
            }
        })
        .collect();

    save_settings(&model_id, &theme_name, &custom_base_urls)?;

    Ok(())
}

// ── Main entry point ────────────────────────────────────────────────────────

/// Run the interactive setup wizard.
pub fn run() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Ensure terminal is restored on panic
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        panic_hook(info);
    }));

    // Load data
    let auth_store = crate::store::auth_storage::shared_auth_storage();
    let providers = load_providers(&auth_store);
    let models = load_models();
    let themes = load_themes();

    let auth_path = crate::store::auth_storage::AuthStorage::default_path().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".oxi")
            .join("auth.json")
    });
    let settings_path = crate::store::settings::Settings::settings_path().unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".oxi")
            .join("settings.json")
    });

    // Find the index of the current default model
    let current_model = crate::store::settings::Settings::load()
        .ok()
        .and_then(|s| s.last_used_model.clone())
        .unwrap_or_default();

    let model_selected = models
        .iter()
        .position(|m| {
            let full_id = format!("{}/{}", m.provider, m.id);
            full_id == current_model || m.id == current_model
        })
        .unwrap_or(0);

    // Find the index of the current theme
    let current_theme = crate::store::settings::Settings::load()
        .ok()
        .map(|s| s.theme.clone())
        .unwrap_or_else(|| "oxi_dark".to_string());

    let theme_selected = themes.iter().position(|t| *t == current_theme).unwrap_or(0);

    let mut state = WizardState {
        step: 0,
        providers,
        provider_selected: 0,
        provider_list_state: ListState::default(),
        input_mode: InputMode::Normal,
        models,
        model_selected,
        model_filter: String::new(),
        model_searching: false,
        themes,
        theme_selected,
        theme_list_state: ListState::default(),
        auth_path,
        settings_path,
    };

    // Main loop
    loop {
        draw_wizard(&mut terminal, &mut state)?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Ctrl+C always quits
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }

                let should_quit = handle_event(&mut state, Event::Key(key), &auth_store)?;
                if should_quit {
                    break;
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}
