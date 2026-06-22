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

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
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
    /// Provider name filter (live while `provider_searching` is true)
    provider_filter: String,
    /// Whether the provider list is being filtered by typing (`/`)
    provider_searching: bool,
    /// Input mode
    input_mode: InputMode,
    /// Model entries for step 2
    models: Vec<ModelEntry>,
    /// Currently selected model index (into `models`)
    model_selected: usize,
    /// Model filter — the list is always filtered live (fzf-style)
    model_filter: String,
    /// List state for the filtered model list
    model_list_state: ListState,
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
    /// Catalog port handle (None = use legacy global state).
    catalog: Option<std::sync::Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>>,
}

/// A model entry for display.
#[derive(Clone)]
struct ModelEntry {
    id: String,
    provider: String,
    context_window: u32,
    /// Lowercased `id`, cached once at load so per-keystroke filtering of the
    /// 5000+ model catalog doesn't allocate on every keypress.
    id_lower: String,
    /// Lowercased `provider`.
    provider_lower: String,
}

impl ModelEntry {
    fn new(id: String, provider: String, context_window: u32) -> Self {
        let provider_lower = provider.to_lowercase();
        let id_lower = id.to_lowercase();
        Self {
            id,
            provider,
            context_window,
            id_lower,
            provider_lower,
        }
    }
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

// ── Filter helpers ──────────────────────────────────────────────────────────

/// Indices of providers whose name matches the current filter (case-insensitive
/// substring). Empty filter ⇒ all providers.
fn filtered_provider_indices(state: &WizardState) -> Vec<usize> {
    if state.provider_filter.is_empty() {
        (0..state.providers.len()).collect()
    } else {
        let f = state.provider_filter.to_lowercase();
        state
            .providers
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name.to_lowercase().contains(&f))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Indices of models matching the current filter (matches id OR provider,
/// case-insensitive substring). Empty filter ⇒ all models. Uses the
/// pre-lowercased `id_lower`/`provider_lower` cached on each `ModelEntry` so
/// the per-keystroke filter doesn't allocate per item.
fn filtered_model_indices(state: &WizardState) -> Vec<usize> {
    if state.model_filter.is_empty() {
        (0..state.models.len()).collect()
    } else {
        let f = state.model_filter.to_lowercase();
        state
            .models
            .iter()
            .enumerate()
            .filter(|(_, m)| m.id_lower.contains(&f) || m.provider_lower.contains(&f))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Clamp `model_selected` so it always points at an item present in the
/// filtered list. If the current selection was filtered out, snap to the
/// first match. No-op when the filter yields nothing.
fn ensure_model_selected_visible(state: &mut WizardState) {
    let filtered = filtered_model_indices(state);
    if filtered.is_empty() {
        return;
    }
    if !filtered.contains(&state.model_selected) {
        state.model_selected = filtered[0];
    }
}

/// Snap `provider_selected` back into the filtered provider set after the
/// filter changes. No-op when the filter yields nothing.
fn snap_provider_selection(state: &mut WizardState) {
    let indices = filtered_provider_indices(state);
    if indices.is_empty() {
        return;
    }
    if !indices.contains(&state.provider_selected) {
        state.provider_selected = indices[0];
    }
}

// ── Load provider state ─────────────────────────────────────────────────────

/// Build the initial provider list from builtins + stored keys + custom providers.
fn load_providers(
    auth_store: &crate::store::auth_storage::AuthStorage,
    catalog: Option<&std::sync::Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>>,
) -> Vec<ProviderEntry> {
    let mut entries = Vec::new();

    let builtin_names: Vec<String> = if let Some(cat) = catalog {
        cat.list_providers_sync()
    } else {
        oxi_sdk::get_builtin_providers()
            .iter()
            .map(|p| p.name.to_string())
            .collect()
    };

    for name in &builtin_names {
        let key = auth_store.get_api_key(name);

        let (has_key, key_masked) = match &key {
            Some(k) => (true, mask_key(k)),
            None => (false, String::new()),
        };

        let base_url = if let Some(cat) = catalog {
            cat.get_provider_sync(name).and_then(|p| p.base_url)
        } else {
            oxi_sdk::get_provider_base_url(name)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        };

        entries.push(ProviderEntry {
            name: name.clone(),
            has_key,
            key_masked,
            is_custom: false,
            base_url,
        });
    }

    // Add custom providers from settings that aren't already in builtins
    if let Ok(settings) = crate::store::settings::Settings::load() {
        for cp in &settings.custom_providers {
            if builtin_names.iter().any(|n| n == &cp.name) {
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

/// Build the model list from the catalog port + dynamic cache.
fn load_models(
    catalog: Option<&std::sync::Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>>,
) -> Vec<ModelEntry> {
    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Dynamic models from settings cache (fetched from /models endpoints)
    if let Ok(settings) = crate::store::settings::Settings::load() {
        for (provider, model_ids) in &settings.dynamic_models {
            for id in model_ids {
                let key = format!("{}/{}", provider, id);
                if seen.insert(key.clone()) {
                    // Try to get context_window from catalog/model_db, default 128_000
                    let ctx = if let Some(cat) = catalog {
                        cat.get_model_sync(provider, id)
                            .map(|e| e.context_window)
                            .unwrap_or(128_000)
                    } else {
                        oxi_sdk::get_model_entry(provider, id)
                            .map(|e| e.context_window)
                            .unwrap_or(128_000)
                    };
                    models.push(ModelEntry::new(id.clone(), provider.clone(), ctx));
                }
            }
        }
    }

    // 2. Catalog models (sync read) or static model_db fallback
    if let Some(cat) = catalog {
        for entry in cat.search_sync("") {
            let key = format!("{}/{}", entry.provider, entry.model_id);
            if seen.insert(key) {
                models.push(ModelEntry::new(
                    entry.model_id,
                    entry.provider,
                    entry.context_window,
                ));
            }
        }
    } else {
        for entry in oxi_sdk::get_all_models() {
            let key = format!("{}/{}", entry.provider, entry.id);
            if seen.insert(key) {
                models.push(ModelEntry::new(
                    entry.id.to_string(),
                    entry.provider.to_string(),
                    entry.context_window,
                ));
            }
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
            Span::styled(
                " oxi ",
                Style::default()
                    .fg(Color::Rgb(255, 165, 0))
                    .add_modifier(Modifier::BOLD),
            ),
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
                    if state.provider_searching {
                        "  Type: filter · ↑/↓ navigate · Enter: select & edit key · Esc: close search · ←: previous".to_string()
                    } else {
                        "  ↑/↓ navigate · /: search · Enter: API key · d: delete · →: next · q: quit".to_string()
                    }
                }
                InputMode::EditingApiKey { .. } => "  Enter: save · Esc: cancel".to_string(),
                InputMode::AddingCustom { .. } => {
                    "  Tab: next field · Enter: save · Esc: cancel".to_string()
                }
            },
            1 => "  Type: filter · ↑/↓ navigate · Enter: select · Esc: clear filter · ←: previous".to_string(),
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

    // When filtering, reserve a one-line search input above the list.
    let (list_area, search_area) = if state.provider_searching {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        (chunks[1], Some(chunks[0]))
    } else {
        (area, None)
    };

    // Search input. The solid block cursor sits right after the typed text
    // (and before the placeholder when empty), mirroring a real text cursor.
    if let Some(search_rect) = search_area {
        let mut spans = vec![
            Span::styled(
                "  Filter: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                &state.provider_filter,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().bg(Color::Yellow)),
        ];
        if state.provider_filter.is_empty() {
            spans.push(Span::styled(
                " type a provider name to filter...",
                Style::default().fg(Color::DarkGray),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), search_rect);
    }

    // Rendered provider indices: filtered while searching, all otherwise.
    let indices: Vec<usize> = if state.provider_searching {
        filtered_provider_indices(state)
    } else {
        (0..state.providers.len()).collect()
    };

    let mut items: Vec<ListItem> = indices
        .iter()
        .map(|&i| {
            let p = &state.providers[i];
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

    // "Add custom" sentinel row only in the unfiltered view.
    if !state.provider_searching {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("   + ", Style::default().fg(Color::Cyan)),
            Span::styled("Add custom provider...", Style::default().fg(Color::Cyan)),
        ])));
    }

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
        )
        .highlight_symbol("▶ ");

    // Map the absolute selected index onto its position in the rendered list.
    let selected_pos = if state.provider_searching {
        indices.iter().position(|&i| i == state.provider_selected)
    } else {
        // provider_selected indexes the provider slice directly; the "+ Add
        // custom" row sits at the end so the absolute index still maps 1:1.
        Some(state.provider_selected)
    };
    state.provider_list_state.select(selected_pos);
    f.render_stateful_widget(list, list_area, &mut state.provider_list_state);
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

    // Reserve a one-line filter input at the top; the list fills the rest.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    // Filter input — always visible (the list is filtered live, fzf-style).
    // The solid block cursor sits right after the typed text (and before the
    // placeholder when empty), mirroring a real text cursor.
    let mut spans = vec![
        Span::styled(
            "  Filter: ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            &state.model_filter,
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(Color::Yellow)),
    ];
    if state.model_filter.is_empty() {
        spans.push(Span::styled(
            " type to filter (e.g. 'gpt-4', 'claude', 'gemini')...",
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);

    // Filtered model list with highlight + scrolling (handles the huge catalog).
    let indices = filtered_model_indices(state);
    let items: Vec<ListItem> = indices
        .iter()
        .map(|&i| {
            let m = &state.models[i];
            let ctx_str = if m.context_window >= 1_000_000 {
                format!("{}M ctx", m.context_window / 1_000_000)
            } else {
                format!("{}K ctx", m.context_window / 1_000)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<40}", m.id), Style::default()),
                Span::styled(
                    format!("({})", m.provider),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!(", {}", ctx_str),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
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
        )
        .highlight_symbol("▶ ");

    let selected_pos = indices.iter().position(|&i| i == state.model_selected);
    state.model_list_state.select(selected_pos);
    f.render_stateful_widget(list, chunks[1], &mut state.model_list_state);

    // Empty-state hint when the filter matches nothing.
    if indices.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            "  No models match your filter. Press Esc to clear.",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(hint, chunks[1]);
    }
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
    // Provider search mode (only active in Normal input mode). Handled before
    // the input_mode match so search keys never collide with key-editing.
    if state.provider_searching && matches!(state.input_mode, InputMode::Normal) {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Esc => {
                    state.provider_searching = false;
                    state.provider_filter.clear();
                    snap_provider_selection(state);
                }
                KeyCode::Enter => {
                    // Pick the highlighted filtered provider, exit search, and
                    // open the API-key dialog for it.
                    state.provider_searching = false;
                    state.provider_filter.clear();
                    if state.provider_selected < state.providers.len() {
                        let name = state.providers[state.provider_selected].name.clone();
                        state.input_mode = InputMode::EditingApiKey {
                            provider_name: name,
                            field_text: String::new(),
                        };
                    }
                }
                KeyCode::Up => {
                    let indices = filtered_provider_indices(state);
                    if let Some(pos) = indices.iter().position(|&i| i == state.provider_selected)
                        && pos > 0
                    {
                        state.provider_selected = indices[pos - 1];
                    } else if let Some(&first) = indices.first() {
                        state.provider_selected = first;
                    }
                }
                KeyCode::Down => {
                    let indices = filtered_provider_indices(state);
                    if let Some(pos) = indices.iter().position(|&i| i == state.provider_selected)
                        && pos + 1 < indices.len()
                    {
                        state.provider_selected = indices[pos + 1];
                    } else if let Some(&first) = indices.first() {
                        state.provider_selected = first;
                    }
                }
                KeyCode::Backspace => {
                    state.provider_filter.pop();
                    snap_provider_selection(state);
                }
                KeyCode::Char(c) => {
                    state.provider_filter.push(c);
                    snap_provider_selection(state);
                }
                KeyCode::Left => {
                    state.provider_searching = false;
                    state.provider_filter.clear();
                    snap_provider_selection(state);
                }
                _ => {}
            }
        }
        return Ok(false);
    }

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
                    KeyCode::Char('/') => {
                        state.provider_searching = true;
                        state.provider_filter.clear();
                        // Snap off the "+ Add custom" sentinel (index ==
                        // providers.len()) so a filtered row is highlighted.
                        snap_provider_selection(state);
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
                            state.models = load_models(state.catalog.as_ref());
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
                                state.models = load_models(state.catalog.as_ref());
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
        // The model list is always filtered live (fzf-style): every printable
        // char extends the filter, Backspace shrinks it. There is no separate
        // "search mode" to enter — the filter input is always active.
        match key.code {
            KeyCode::Char(c) => {
                state.model_filter.push(c);
                ensure_model_selected_visible(state);
            }
            KeyCode::Backspace => {
                state.model_filter.pop();
                ensure_model_selected_visible(state);
            }
            KeyCode::Up => {
                let indices = filtered_model_indices(state);
                if let Some(pos) = indices.iter().position(|&i| i == state.model_selected)
                    && pos > 0
                {
                    state.model_selected = indices[pos - 1];
                } else if let Some(&first) = indices.first() {
                    state.model_selected = first;
                }
            }
            KeyCode::Down => {
                let indices = filtered_model_indices(state);
                if let Some(pos) = indices.iter().position(|&i| i == state.model_selected)
                    && pos + 1 < indices.len()
                {
                    state.model_selected = indices[pos + 1];
                } else if let Some(&first) = indices.first() {
                    state.model_selected = first;
                }
            }
            KeyCode::Enter => {
                // Only advance when the filter yields a selectable model. A
                // non-empty filter that matches nothing leaves nothing to
                // confirm, so stay put rather than silently carrying over a
                // stale selection into the next step.
                if !filtered_model_indices(state).is_empty() {
                    state.step = 2;
                }
            }
            KeyCode::Esc => {
                // Clear the filter (Left handles going back to providers).
                state.model_filter.clear();
                ensure_model_selected_visible(state);
            }
            KeyCode::Left => {
                state.step = 0;
            }
            _ => {}
        }
    }
    Ok(false)
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
pub async fn run() -> Result<()> {
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

    // Initialize the catalog port for model/provider lookups.
    let catalog: Option<std::sync::Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>> = {
        let paths = crate::services::OxiPaths::default_paths().ok();
        if let Some(paths) = paths {
            let config = oxi_sdk::CatalogConfig {
                cache_path: paths.home.join("cache").join("models-dev.json"),
                etag_path: paths.home.join("cache").join("models-dev.json.etag"),
                override_path: paths.home.join("catalog").join("overrides.toml"),
                // Don't trigger a network refresh during setup.
                fetch_enabled: false,
                ..Default::default()
            };
            oxi_sdk::FileModelCatalog::init(config)
                .await
                .ok()
                .map(|c| c as _)
        } else {
            None
        }
    };

    // Load data
    let auth_store = crate::store::auth_storage::shared_auth_storage();
    let providers = load_providers(&auth_store, catalog.as_ref());
    let models = load_models(catalog.as_ref());
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
        provider_filter: String::new(),
        provider_searching: false,
        input_mode: InputMode::Normal,
        models,
        model_selected,
        model_filter: String::new(),
        model_list_state: ListState::default(),
        themes,
        theme_selected,
        theme_list_state: ListState::default(),
        auth_path,
        settings_path,
        catalog,
    };

    // Main loop
    loop {
        draw_wizard(&mut terminal, &mut state)?;

        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
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

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(providers: Vec<&str>, models: Vec<(&str, &str)>) -> WizardState {
        WizardState {
            step: 0,
            providers: providers
                .iter()
                .map(|n| ProviderEntry {
                    name: n.to_string(),
                    has_key: false,
                    key_masked: String::new(),
                    is_custom: false,
                    base_url: None,
                })
                .collect(),
            provider_selected: 0,
            provider_list_state: ListState::default(),
            provider_filter: String::new(),
            provider_searching: false,
            input_mode: InputMode::Normal,
            models: models
                .iter()
                .map(|(id, provider)| {
                    ModelEntry::new(id.to_string(), provider.to_string(), 128_000)
                })
                .collect(),
            model_selected: 0,
            model_filter: String::new(),
            model_list_state: ListState::default(),
            themes: vec![],
            theme_selected: 0,
            theme_list_state: ListState::default(),
            auth_path: PathBuf::new(),
            settings_path: PathBuf::new(),
            catalog: None,
        }
    }

    #[test]
    fn provider_filter_matches_name_case_insensitive() {
        let mut s = make_state(vec!["anthropic", "openai", "google", "mistral"], vec![]);
        assert_eq!(filtered_provider_indices(&s), vec![0, 1, 2, 3]);

        s.provider_filter = "ANT".to_string();
        assert_eq!(filtered_provider_indices(&s), vec![0]); // anthropic

        s.provider_filter = "goog".to_string();
        assert_eq!(filtered_provider_indices(&s), vec![2]); // google
    }

    #[test]
    fn model_filter_matches_id_or_provider() {
        let mut s = make_state(
            vec![],
            vec![
                ("gpt-4o", "openai"),
                ("gpt-4-turbo", "openai"),
                ("claude-3-opus", "anthropic"),
                ("gemini-pro", "google"),
            ],
        );
        assert_eq!(filtered_model_indices(&s), vec![0, 1, 2, 3]);

        s.model_filter = "gpt".to_string();
        assert_eq!(filtered_model_indices(&s), vec![0, 1]);

        s.model_filter = "anthropic".to_string();
        assert_eq!(filtered_model_indices(&s), vec![2]); // matched by provider

        s.model_filter = "OPUS".to_string();
        assert_eq!(filtered_model_indices(&s), vec![2]); // case-insensitive
    }

    #[test]
    fn model_filter_empty_result_yields_no_indices() {
        let mut state = make_state(vec![], vec![("gpt-4o", "openai")]);
        state.model_filter = "zzz".to_string();
        assert!(filtered_model_indices(&state).is_empty());
    }

    #[test]
    fn ensure_model_selected_snaps_to_first_match() {
        let mut state = make_state(
            vec![],
            vec![
                ("gpt-4o", "openai"),
                ("claude-3", "anthropic"),
                ("gpt-3.5", "openai"),
            ],
        );
        // Selection starts at index 0 (gpt-4o).
        state.model_filter = "gpt".to_string();
        ensure_model_selected_visible(&mut state);
        // gpt-4o is in the filtered set {0, 2}, so it stays.
        assert_eq!(state.model_selected, 0);

        // Now filter to only claude; selection must snap to it.
        state.model_filter = "claude".to_string();
        ensure_model_selected_visible(&mut state);
        assert_eq!(state.model_selected, 1);
    }

    #[test]
    fn snap_provider_selection_into_filtered_set() {
        let mut state = make_state(vec!["anthropic", "openai", "google"], vec![]);
        state.provider_selected = 2; // google
        state.provider_filter = "open".to_string();
        snap_provider_selection(&mut state);
        assert_eq!(state.provider_selected, 1); // openai
    }

    #[test]
    fn snap_provider_noop_when_filter_empty_matches_all() {
        let mut state = make_state(vec!["anthropic", "openai"], vec![]);
        state.provider_selected = 1;
        state.provider_filter = String::new();
        snap_provider_selection(&mut state);
        assert_eq!(state.provider_selected, 1); // unchanged
    }
}
