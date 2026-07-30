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
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
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
    /// Provider name filter — always active, fzf-style (no separate search mode).
    provider_filter: String,
    /// When true, the "+ Add custom provider…" sentinel row at the bottom of
    /// the provider list is selected instead of a real provider entry.
    on_sentinel: bool,
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
    /// Dirty flag: a provider key was added/removed since the last model-list
    /// rebuild. The main loop rebuilds (filtered to configured providers) before
    /// drawing step 1.
    models_dirty: bool,
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
/// filter changes. The sentinel ("+ Add custom") is the only selectable
/// position when the filter yields no providers; otherwise the first match
/// wins and the sentinel is cleared so the user is back on real providers.
fn snap_provider_selection(state: &mut WizardState) {
    let indices = filtered_provider_indices(state);
    if indices.is_empty() {
        state.on_sentinel = true;
        return;
    }
    if state.on_sentinel || !indices.contains(&state.provider_selected) {
        state.provider_selected = indices[0];
        state.on_sentinel = false;
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
///
/// When `allowed` is `Some`, only models whose provider is in the set are
/// returned — the wizard uses this to restrict step 2 to providers the user
/// actually configured (added an API key to) in step 1. `Some(empty)` yields an
/// empty list; `None` disables the provider filter entirely.
fn load_models(
    catalog: Option<&std::sync::Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>>,
    allowed: Option<&std::collections::HashSet<String>>,
) -> Vec<ModelEntry> {
    let permit = |provider: &str| match allowed {
        None => true,
        Some(set) => set.contains(provider),
    };

    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Dynamic models from settings cache (fetched from /models endpoints)
    if let Ok(settings) = crate::store::settings::Settings::load() {
        for (provider, model_ids) in &settings.dynamic_models {
            if !permit(provider) {
                continue;
            }
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
            if !permit(&entry.provider) {
                continue;
            }
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
            if !permit(entry.provider) {
                continue;
            }
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

/// Names of providers the user has configured (added an API key to) in step 1.
/// Step 2's model list is restricted to these so the user only chooses among
/// models they can actually call.
fn keyed_provider_names(providers: &[ProviderEntry]) -> std::collections::HashSet<String> {
    providers
        .iter()
        .filter(|p| p.has_key)
        .map(|p| p.name.clone())
        .collect()
}

/// Rebuild `state.models` from the currently-configured providers, keeping the
/// selection on the same model when it survives the rebuild. Called by the main
/// loop whenever the provider-key set changes and the user is on step 1.
fn refresh_models(state: &mut WizardState) {
    let allowed = keyed_provider_names(&state.providers);
    let prev = state
        .models
        .get(state.model_selected)
        .map(|m| (m.provider.clone(), m.id.clone()));
    state.models = load_models(state.catalog.as_ref(), Some(&allowed));
    state.model_selected = match prev {
        Some((p, id)) => state
            .models
            .iter()
            .position(|m| m.provider == p && m.id == id)
            .unwrap_or(0),
        None => 0,
    };
    ensure_model_selected_visible(state);
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
    oxi_vtui::theme::available_themes().into_iter().map(|s| s.to_string()).collect()
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
    terminal.draw(|f| render_wizard(f, state))?;
    Ok(())
}

/// Heuristic: how many lines the footer text wraps into given `cols` width.
/// Packs words greedily (split on whitespace); returns at least 1 so the
/// footer is never allocated zero rows.
fn wrapped_line_count(text: &str, cols: u16) -> u16 {
    let max_w = if cols < 2 { 80usize } else { cols as usize };
    let mut lines: u16 = 1;
    let mut cur = 0usize;
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if cur == 0 {
            cur = wlen;
        } else if cur + 1 + wlen <= max_w {
            cur += 1 + wlen;
        } else {
            lines = lines.saturating_add(1);
            cur = wlen;
        }
    }
    lines.max(1)
}

/// Render the wizard into a frame — title bar, persistent step indicator,
/// step-specific content, and a width-adaptive footer that wraps instead of
/// truncating on narrow terminals.
fn render_wizard(f: &mut ratatui::Frame, state: &mut WizardState) {
    let size = f.area();

    // Build footer text first so we can compute how many rows it needs.
    let footer_text = match state.step {
        0 => match &state.input_mode {
            InputMode::Normal => {
                "  Type to filter · ↑/↓ · Enter: act · → next · Esc back".to_string()
            }
            InputMode::EditingApiKey { .. } => {
                "  Enter: save · Ctrl+R: remove (existing) · Esc: cancel".to_string()
            }
            InputMode::AddingCustom { .. } => "  Tab: next · Enter: save · Esc: cancel".to_string(),
        },
        1 => "  Type to filter · ↑/↓ · Enter: select · Esc: back · ←: prev".to_string(),
        2 => "  ↑/↓ navigate · Enter: select · Esc/←: back".to_string(),
        3 => "  Esc or Enter: quit".to_string(),
        _ => String::new(),
    };
    let footer_rows = wrapped_line_count(&footer_text, size.width).min(2);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),           // Title bar
            Constraint::Length(1),           // Step indicator (always visible)
            Constraint::Min(8),              // Content
            Constraint::Length(footer_rows), // Footer (adapts to width)
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

    // Persistent step indicator — its own dedicated line so it never collides
    // with the first list item (which hid it when it was a borderless block
    // `.title()` — the title and item share row 0).
    f.render_widget(Paragraph::new(build_step_indicator(state.step)), chunks[1]);

    // Content depends on step
    match state.step {
        0 => draw_provider_step(f, state, chunks[2]),
        1 => draw_model_step(f, state, chunks[2]),
        2 => draw_theme_step(f, state, chunks[2]),
        3 => draw_done_step(f, state, chunks[2]),
        _ => {}
    }

    // Footer — wraps instead of truncating when the terminal is narrower than
    // the hint text.
    let footer = Paragraph::new(Line::from(Span::styled(
        footer_text,
        Style::default().fg(Color::DarkGray),
    )))
    .wrap(Wrap { trim: false });
    f.render_widget(footer, chunks[3]);
}

fn draw_provider_step(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    match &state.input_mode {
        InputMode::Normal => draw_provider_list(f, state, area),
        InputMode::EditingApiKey {
            provider_name,
            field_text,
        } => {
            // Surface a "remove" hint only when the provider already has a
            // stored key; there's nothing to remove otherwise.
            let has_existing_key = state
                .providers
                .iter()
                .find(|p| p.name == *provider_name)
                .is_some_and(|p| p.has_key);
            draw_api_key_dialog(f, provider_name, field_text, has_existing_key, area);
        }
        InputMode::AddingCustom {
            fields,
            active_field,
        } => draw_custom_provider_dialog(f, fields, *active_field, area),
    }
}

/// Render the provider step: always-on fzf-style filter at the top, a
/// scrollable list of filtered providers, and a permanent
/// "+ Add custom provider…" sentinel row pinned below the list so it's
/// always reachable (never scrolled off-screen). Mirrors the model step's
/// interaction model — typing filters live, the filter input is always
/// shown, and Enter acts on the selection.
fn draw_provider_list(f: &mut ratatui::Frame, state: &mut WizardState, area: Rect) {
    // Three-row layout: filter (1) / list (scrollable) / sentinel (1).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // filter input
            Constraint::Min(1),    // provider list
            Constraint::Length(1), // "+ Add custom provider…" sentinel
        ])
        .split(area);

    // Filter input — always visible. The solid block cursor sits right after
    // the typed text (and before the placeholder when empty), mirroring a
    // real text cursor.
    let mut filter_spans = vec![
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
        filter_spans.push(Span::styled(
            " type to filter (e.g. 'open', 'anth', 'googl')...",
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(filter_spans)), chunks[0]);

    let indices = filtered_provider_indices(state);

    // Provider list (filtered). The sentinel is NOT part of the list anymore
    // — it lives in its own row below so it stays on screen.
    let items: Vec<ListItem> = indices
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

    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    // Highlight a list item only when the selection is a real provider.
    let list_selected = if state.on_sentinel {
        None
    } else {
        indices.iter().position(|&i| i == state.provider_selected)
    };
    state.provider_list_state.select(list_selected);
    f.render_stateful_widget(list, chunks[1], &mut state.provider_list_state);

    // Persistent "+ Add custom provider…" sentinel row — always visible
    // below the list. When selected, the row is highlighted with the same
    // background as a selected list item so the visual continuity is clear.
    let sentinel = if state.on_sentinel {
        Line::from(Span::styled(
            "▶   + Add custom provider…",
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(Span::styled(
            "    + Add custom provider…",
            Style::default().fg(Color::Cyan),
        ))
    };
    f.render_widget(Paragraph::new(sentinel), chunks[2]);
}

/// `has_existing_key` is used to surface a "Ctrl+R: remove" hint when the
/// provider already has a stored key — the only place the remove action is
/// reachable.
fn draw_api_key_dialog(
    f: &mut ratatui::Frame,
    provider_name: &str,
    field_text: &str,
    has_existing_key: bool,
    area: Rect,
) {
    // Center the dialog
    let dialog_height = 8u16;
    let dialog_width = std::cmp::min(area.width, 60);
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;

    let dialog_area = Rect::new(area.x + x, area.y + y, dialog_width, dialog_height);

    let display_text = if field_text.is_empty() {
        String::new()
    } else {
        "*".repeat(field_text.len())
    };

    let mut paragraphs = vec![
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
    if has_existing_key {
        paragraphs.push(Line::from(Span::styled(
            "  (existing key will be replaced)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        paragraphs.push(Line::from(""));
    }
    paragraphs.push(Line::from(Span::styled(
        if has_existing_key {
            "  Enter: save · Ctrl+R: remove · Esc: cancel"
        } else {
            "  Enter: save · Esc: cancel"
        },
        Style::default().fg(Color::DarkGray),
    )));

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
    // No configured providers (none with an API key) → nothing to choose among.
    // Guide the user back to step 1 instead of rendering an empty list.
    if state.models.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No providers with an API key configured yet.",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Left to go back and add a provider key first.",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        f.render_widget(msg, area);
        return;
    }

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
        .block(Block::default().borders(Borders::NONE))
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
    let items: Vec<ListItem> = state
        .themes
        .iter()
        .map(|t| ListItem::new(Line::from(format!("  {}", t))))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
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
    // A single match dispatches Normal (always-on filter, navigation, enter,
    // esc, →) and the two dialog modes. There is no separate search mode —
    // the filter input is always active, mirroring the model step.
    match &mut state.input_mode {
        InputMode::Normal => {
            if let Event::Key(key) = event {
                match key.code {
                    // Filter input
                    KeyCode::Char(c) => {
                        state.provider_filter.push(c);
                        snap_provider_selection(state);
                    }
                    KeyCode::Backspace => {
                        state.provider_filter.pop();
                        snap_provider_selection(state);
                    }
                    // Navigation: the selectable list is
                    // `filtered_provider_indices` followed by the sentinel at
                    // the end. Positions never wrap at the edges.
                    KeyCode::Up => {
                        let indices = filtered_provider_indices(state);
                        if state.on_sentinel {
                            if let Some(&last) = indices.last() {
                                state.provider_selected = last;
                                state.on_sentinel = false;
                            }
                        } else if let Some(pos) =
                            indices.iter().position(|&i| i == state.provider_selected)
                        {
                            if pos > 0 {
                                state.provider_selected = indices[pos - 1];
                            }
                        } else if let Some(&first) = indices.first() {
                            state.provider_selected = first;
                        } else {
                            state.on_sentinel = true;
                        }
                    }
                    KeyCode::Down => {
                        let indices = filtered_provider_indices(state);
                        if state.on_sentinel {
                            // Already at the bottom; stay.
                        } else if let Some(pos) =
                            indices.iter().position(|&i| i == state.provider_selected)
                        {
                            if pos + 1 < indices.len() {
                                state.provider_selected = indices[pos + 1];
                            } else {
                                // Last real provider → drop down to sentinel.
                                state.on_sentinel = true;
                            }
                        } else if let Some(&first) = indices.first() {
                            state.provider_selected = first;
                        } else {
                            state.on_sentinel = true;
                        }
                    }
                    KeyCode::Enter => {
                        if state.on_sentinel {
                            // "+ Add custom provider…"
                            state.input_mode = InputMode::AddingCustom {
                                fields: [String::new(), String::new(), String::new()],
                                active_field: 0,
                            };
                        } else {
                            let name = state.providers[state.provider_selected].name.clone();
                            state.input_mode = InputMode::EditingApiKey {
                                provider_name: name,
                                field_text: String::new(),
                            };
                        }
                    }
                    KeyCode::Esc => {
                        // Esc backs out: clear an active filter, otherwise quit
                        // (we're on the top-level provider step).
                        if !state.provider_filter.is_empty() {
                            state.provider_filter.clear();
                            snap_provider_selection(state);
                        } else {
                            return Ok(true);
                        }
                    }
                    KeyCode::Right => {
                        state.step = 1;
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
                            if let Some(entry) = state
                                .providers
                                .iter_mut()
                                .find(|p| p.name == *provider_name)
                            {
                                entry.has_key = true;
                                entry.key_masked = mask_key(field_text);
                            }
                            fetch_and_cache_models(provider_name, &state.providers);
                            state.models_dirty = true;
                        }
                        state.input_mode = InputMode::Normal;
                    }
                    // Ctrl+R removes the stored key (destructive; intentionally
                    // hidden behind a non-printable modifier so accidental
                    // typing can't trigger it).
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let name = provider_name.clone();
                        auth_store.remove(&name);
                        if let Some(entry) = state.providers.iter_mut().find(|p| p.name == name) {
                            entry.has_key = false;
                            entry.key_masked = String::new();
                        }
                        state.models_dirty = true;
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
                            if !api_key.is_empty() {
                                auth_store.set_api_key(&name, api_key.clone());
                            }
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
                            if !api_key.is_empty() {
                                fetch_and_cache_models(&name, &state.providers);
                            }
                            state.models_dirty = has_key;
                            // Land on the newly-added provider instead of the
                            // sentinel.
                            state.provider_selected = state.providers.len() - 1;
                            state.on_sentinel = false;
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
                // Esc backs out one level: clear an active filter, otherwise
                // return to the provider step.
                if !state.model_filter.is_empty() {
                    state.model_filter.clear();
                    ensure_model_selected_visible(state);
                } else {
                    state.step = 0;
                }
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
            KeyCode::Esc | KeyCode::Left => {
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
            KeyCode::Enter | KeyCode::Esc => {
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
    let allowed = keyed_provider_names(&providers);
    let models = load_models(catalog.as_ref(), Some(&allowed));
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
        on_sentinel: false,
        input_mode: InputMode::Normal,
        models,
        model_selected,
        model_filter: String::new(),
        model_list_state: ListState::default(),
        models_dirty: false,
        themes,
        theme_selected,
        theme_list_state: ListState::default(),
        auth_path,
        settings_path,
        catalog,
    };

    // Main loop
    loop {
        // If a provider key changed, rebuild the model list (restricted to the
        // now-configured providers) before drawing — so step 1 never shows
        // stale or unconfigured-provider models.
        if state.step == 1 && state.models_dirty {
            refresh_models(&mut state);
            state.models_dirty = false;
        }
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
            on_sentinel: false,
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
            models_dirty: false,
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
    #[test]
    fn keyed_provider_names_only_includes_configured() {
        let providers = vec![
            ProviderEntry {
                name: "anthropic".to_string(),
                has_key: true,
                key_masked: "sk-1...abcd".to_string(),
                is_custom: false,
                base_url: None,
            },
            ProviderEntry {
                name: "openai".to_string(),
                has_key: false,
                key_masked: String::new(),
                is_custom: false,
                base_url: None,
            },
            ProviderEntry {
                name: "local".to_string(),
                has_key: true,
                key_masked: "x...y".to_string(),
                is_custom: true,
                base_url: Some("http://localhost:11434".to_string()),
            },
        ];
        let set = keyed_provider_names(&providers);
        assert!(set.contains("anthropic"));
        assert!(set.contains("local"));
        assert!(!set.contains("openai"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn keyed_provider_names_empty_when_none_configured() {
        let providers = vec![ProviderEntry {
            name: "openai".to_string(),
            has_key: false,
            key_masked: String::new(),
            is_custom: false,
            base_url: None,
        }];
        assert!(keyed_provider_names(&providers).is_empty());
    }
    /// Render the full wizard into a TestBackend buffer and return the
    /// concatenated cell text (rows joined with '\n') for substring assertions.
    fn render_to_buffer(step: usize, models: Vec<ModelEntry>) -> String {
        use ratatui::backend::TestBackend;
        let providers = vec![
            ProviderEntry {
                name: "openai".to_string(),
                has_key: true,
                key_masked: "k...1".to_string(),
                is_custom: false,
                base_url: None,
            },
            ProviderEntry {
                name: "anthropic".to_string(),
                has_key: false,
                key_masked: String::new(),
                is_custom: false,
                base_url: None,
            },
        ];
        let mut state = WizardState {
            step,
            providers,
            provider_selected: 0,
            provider_list_state: ListState::default(),
            provider_filter: String::new(),
            on_sentinel: false,
            input_mode: InputMode::Normal,
            models,
            model_selected: 0,
            model_filter: String::new(),
            model_list_state: ListState::default(),
            themes: vec!["oxi_dark".to_string()],
            theme_selected: 0,
            theme_list_state: ListState::default(),
            auth_path: PathBuf::new(),
            settings_path: PathBuf::new(),
            catalog: None,
            models_dirty: false,
        };
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_wizard(f, &mut state)).unwrap();
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn step_indicator_visible_on_every_step() {
        // The step indicator must render on its own dedicated line for every
        // step — the bug it fixes hid it (borderless-block title collided with
        // the first list item).
        for (step, label) in [
            (0usize, "1. Provider Setup"),
            (1, "2. Default Model"),
            (2, "3. Theme"),
            (3, "4. Done"),
        ] {
            let models = vec![ModelEntry::new(
                "gpt-4o".to_string(),
                "openai".to_string(),
                128_000,
            )];
            let rendered = render_to_buffer(step, models);
            assert!(
                rendered.contains(label),
                "step {step}: indicator label {label:?} missing from buffer:\n{rendered}"
            );
        }
    }

    #[test]
    fn model_step_shows_empty_state_when_no_provider_keyed() {
        // With an empty model list (no keyed providers), step 1 must show the
        // guidance message, not a bare empty filter list.
        let rendered = render_to_buffer(1, vec![]);
        assert!(rendered.contains("No providers with an API key configured yet."));
        assert!(rendered.contains("Press Left to go back"));
    }

    #[test]
    fn model_step_shows_configured_provider_model() {
        // Only models from keyed providers appear. Here the only keyed
        // provider is "openai", so a claude model (anthropic, not keyed) must
        // NOT show even if it were somehow in the list — but since we pass the
        // filtered list directly, we assert the openai model renders.
        let models = vec![ModelEntry::new(
            "gpt-4o".to_string(),
            "openai".to_string(),
            128_000,
        )];
        let rendered = render_to_buffer(1, models);
        assert!(rendered.contains("gpt-4o"));
    }
    // ── Esc-centric key model ──────────────────────────────────────────────
    // Esc is the universal "back out one level" key: cancel sub-mode → clear
    // filter → previous step → quit. These exercise the handlers directly
    // (deterministic, no terminal-timing dependency).

    fn esc_event() -> Event {
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        ))
    }

    #[test]
    fn esc_quits_from_provider_step_normal() {
        let mut state = make_state(vec!["openai"], vec![]);
        state.step = 0;
        let auth = crate::store::auth_storage::shared_auth_storage();
        let quit = handle_provider_event(&mut state, esc_event(), &auth).unwrap();
        assert!(quit, "Esc on step 0 Normal should quit");
    }

    #[test]
    fn esc_clears_provider_filter_without_quitting() {
        // In the always-on filter model, Esc backs out: with an active filter
        // it clears the filter and does NOT quit. Quitting is reserved for
        // Esc with an empty filter (top-level).
        let mut state = make_state(vec!["openai", "anthropic"], vec![]);
        state.step = 0;
        state.provider_filter = "anth".to_string();
        // The Esc handler will clear the filter and call snap_provider_selection.
        let auth = crate::store::auth_storage::shared_auth_storage();
        let quit = handle_provider_event(&mut state, esc_event(), &auth).unwrap();
        assert!(!quit, "Esc with a non-empty filter must clear it, not quit");
        assert!(state.provider_filter.is_empty());
    }

    #[test]
    fn esc_backs_out_of_model_step_when_filter_empty() {
        let mut state = make_state(vec!["openai"], vec![("gpt-4o", "openai")]);
        state.step = 1;
        state.model_filter = String::new();
        handle_model_event(&mut state, esc_event()).unwrap();
        assert_eq!(
            state.step, 0,
            "Esc with empty filter should return to the provider step"
        );
    }

    #[test]
    fn esc_clears_model_filter_when_nonempty() {
        let mut state = make_state(
            vec!["openai"],
            vec![("gpt-4o", "openai"), ("gpt-4", "openai")],
        );
        state.step = 1;
        state.model_filter = "gpt".to_string();
        handle_model_event(&mut state, esc_event()).unwrap();
        assert_eq!(
            state.step, 1,
            "Esc with a non-empty filter should stay on the model step"
        );
        assert!(state.model_filter.is_empty(), "Esc should clear the filter");
    }

    #[test]
    fn esc_backs_out_of_theme_step() {
        let mut state = make_state(vec!["openai"], vec![]);
        state.step = 2;
        state.themes = vec!["oxi_dark".to_string()];
        handle_theme_event(&mut state, esc_event()).unwrap();
        assert_eq!(state.step, 1);
    }

    #[test]
    fn esc_quits_from_done_step() {
        assert!(
            handle_done_event(esc_event()).unwrap(),
            "Esc on the done step should quit"
        );
    }
    #[test]
    fn provider_step_renders_filter_and_sentinel() {
        // Always-on filter: the "Filter:" input line must be visible, and the
        // "+ Add custom provider…" sentinel must always be present in the
        // list — both unfiltered and filtered.
        // Unfiltered view.
        let rendered = render_to_buffer(0, vec![]);
        assert!(
            rendered.contains("Filter:"),
            "filter line missing in unfiltered provider step"
        );
        assert!(
            rendered.contains("Add custom provider"),
            "sentinel missing in unfiltered provider step"
        );
        // Filtered view: filter shows the typed text, sentinel remains.
        let providers = vec![ProviderEntry {
            name: "openai".to_string(),
            has_key: true,
            key_masked: "k".to_string(),
            is_custom: false,
            base_url: None,
        }];
        let mut s = WizardState {
            step: 0,
            providers,
            provider_selected: 0,
            provider_list_state: ListState::default(),
            provider_filter: "open".to_string(),
            on_sentinel: false,
            input_mode: InputMode::Normal,
            models: vec![],
            model_selected: 0,
            model_filter: String::new(),
            model_list_state: ListState::default(),
            themes: vec![],
            theme_selected: 0,
            theme_list_state: ListState::default(),
            auth_path: PathBuf::new(),
            settings_path: PathBuf::new(),
            catalog: None,
            models_dirty: false,
        };
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_wizard(f, &mut s)).unwrap();
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        assert!(out.contains("Filter:"));
        assert!(
            out.contains("open"),
            "typed filter must be shown in the filter line"
        );
        assert!(
            out.contains("Add custom provider"),
            "sentinel must remain under a filter"
        );
    }
    #[test]
    fn footer_wraps_on_narrow_terminal() {
        // On a 50-column terminal the provider-step footer hint (~57 chars)
        // must wrap to two lines instead of being silently truncated.
        // We check that the wrapped word is present (could be on row 1 or 2).
        use ratatui::backend::TestBackend;
        let providers = vec![ProviderEntry {
            name: "openai".to_string(),
            has_key: false,
            key_masked: String::new(),
            is_custom: false,
            base_url: None,
        }];
        let mut s = WizardState {
            step: 0,
            providers,
            provider_selected: 0,
            provider_list_state: ListState::default(),
            provider_filter: String::new(),
            on_sentinel: false,
            input_mode: InputMode::Normal,
            models: vec![],
            model_selected: 0,
            model_filter: String::new(),
            model_list_state: ListState::default(),
            themes: vec![],
            theme_selected: 0,
            theme_list_state: ListState::default(),
            auth_path: PathBuf::new(),
            settings_path: PathBuf::new(),
            catalog: None,
            models_dirty: false,
        };
        let backend = TestBackend::new(50, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_wizard(f, &mut s)).unwrap();
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        // The footer text "Type to filter..." must not be cut off — every
        // word in the hint should appear somewhere in the buffer.
        for word in [
            "Type",
            "filter",
            "\u{2191}/\u{2193}",
            "act",
            "next",
            "Esc",
            "back",
        ] {
            assert!(
                out.contains(word),
                "footer word {word:?} missing at 50 cols — footer may be truncated"
            );
        }
    }
}
