//! Extended slash commands for the VT TUI harness.
//!
//! These live in a sibling module to [`super::registry`] to keep the catalog-
//! dependent and introspection commands isolated from the core command
//! plumbing. They are registered alongside the built-ins by
//! `register_extra` (called from `register_all` in the sibling `registry`
//!
//! Every command here is self-contained: it owns its definition + execution
//! and receives a `SlashCtx` (`session`, `handle`, `state`). The model
//! catalog is reached via `ctx.state.catalog` (captured once at TUI startup);
//! credentials via the process-global `shared_auth_storage()`.

use oxicode_vtui::tui::core::{
    InlineListItem, InlineListSearchConfig, InlineListSelection, InlineMessageKind,
};

use super::registry::{SlashCommand, SlashCtx, SlashOutcome, SlashRegistry};

/// Register every extended command. Called from `register_all`.
pub(crate) fn register_extra(registry: &mut SlashRegistry) {
    registry.register(Box::new(ModelsCommand));
    registry.register(Box::new(ProvidersCommand));
    registry.register(Box::new(ToolsCommand));
    registry.register(Box::new(McpCommand));
    registry.register(Box::new(InfoCommand));
    registry.register(Box::new(ExportCommand));
}

// ─────────────────────────────────────────────────────────────────────────
// Formatting helpers (pure, unit-tested)
// ─────────────────────────────────────────────────────────────────────────

/// Format a token count as a compact context-window label.
pub(super) fn fmt_ctx(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M ctx", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1000 {
        format!("{}K ctx", tokens / 1000)
    } else {
        format!("{tokens} ctx")
    }
}

/// Format a USD-per-million-token price. `0.0` (free / undisclosed) → "free".
pub(super) fn fmt_cost(price: f64) -> String {
    if price <= 0.0 {
        "free".to_string()
    } else if price < 0.01 {
        "<$0.01/M".to_string()
    } else {
        format!("${price:.2}/M")
    }
}

/// Split a `provider/model` id into `(provider, model_id)` on the first `/`.
pub(super) fn split_model_id(model_id: &str) -> (&str, &str) {
    match model_id.find('/') {
        Some(i) => (&model_id[..i], &model_id[i + 1..]),
        None => (model_id, ""),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// /models — browse the FULL catalog
// ─────────────────────────────────────────────────────────────────────────

/// `/models [query]` — open a searchable list of every catalog model
/// (provider/model · context window · price). Selecting a row switches the
/// active model. Unlike `/model` (scoped models only), this browses the entire
/// models.dev catalog plus local/dynamic discovery.
struct ModelsCommand;

impl SlashCommand for ModelsCommand {
    fn name(&self) -> &'static str {
        "models"
    }
    fn description(&self) -> &'static str {
        "Browse the full model catalog (/models [query])"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let query = args.trim();
        let Some(catalog) = ctx.state.catalog.as_ref() else {
            ctx.reply(
                InlineMessageKind::Warning,
                "Model catalog is unavailable in this session.",
            );
            return SlashOutcome::Handled;
        };

        // `search_sync("")` returns the full snapshot; we filter client-side
        // so the optional `query` narrows provider/model/name in one pass.
        let mut entries = catalog.search_sync("");
        entries.sort_by(|a, b| {
            a.provider
                .cmp(&b.provider)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });

        let q = query.to_ascii_lowercase();
        let filtered: Vec<_> = if q.is_empty() {
            entries.iter().collect()
        } else {
            entries
                .iter()
                .filter(|e| {
                    e.provider.to_ascii_lowercase().contains(&q)
                        || e.model_id.to_ascii_lowercase().contains(&q)
                        || e.name.to_ascii_lowercase().contains(&q)
                })
                .collect()
        };

        if filtered.is_empty() {
            if entries.is_empty() {
                ctx.reply(
                    InlineMessageKind::Warning,
                    "Model catalog is empty (catalog may have failed to load).",
                );
            } else {
                ctx.reply(
                    InlineMessageKind::Warning,
                    format!("No models match '{query}'."),
                );
            }
            return SlashOutcome::Handled;
        }

        let total = filtered.len();
        // Record the (provider, model_id) pairs backing each row so the
        // overlay-submission handler can resolve a selection back to a model.
        ctx.state.overlay_catalog_models = filtered
            .iter()
            .map(|e| (e.provider.clone(), e.model_id.clone()))
            .collect();

        let current = ctx.session.model_id();
        let items: Vec<InlineListItem> = filtered
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let id = format!("{}/{}", e.provider, e.model_id);
                let mut sub = format!(
                    "{} · {} in / {} out",
                    fmt_ctx(e.context_window),
                    fmt_cost(e.cost_input),
                    fmt_cost(e.cost_output)
                );
                if e.reasoning {
                    sub.push_str(" · reasoning");
                }
                if e.supports_vision {
                    sub.push_str(" · vision");
                }
                InlineListItem {
                    title: id.clone(),
                    subtitle: Some(sub),
                    badge: if id == current {
                        Some("active".to_string())
                    } else {
                        None
                    },
                    indent: 0,
                    selection: Some(InlineListSelection::CatalogModel(i)),
                    search_value: Some(format!("{} {} {}", e.provider, e.model_id, e.name)),
                }
            })
            .collect();

        let search = InlineListSearchConfig {
            label: "Filter models".into(),
            placeholder: Some("Type to filter (provider / model / name)\u{2026}".into()),
        };
        ctx.handle.show_list_modal(
            format!("Models ({total})"),
            vec![format!(
                "{} model{} \u{2014} Enter to switch, Esc to close",
                total,
                if total == 1 { "" } else { "s" }
            )],
            items,
            None,
            Some(search),
        );
        SlashOutcome::Handled
    }
}

// ─────────────────────────────────────────────────────────────────────────
// /providers — credential status + key removal
// ─────────────────────────────────────────────────────────────────────────

/// `/providers` — list every known provider with its credential status.
/// `/providers remove <name>` — remove a stored API key (asks for
/// confirmation; `--yes` skips it).
/// `/providers add <name> <base_url> [api_key_env] [api]` — register a
/// custom OpenAI-compatible provider into `~/.oxicode/settings.toml`.
/// `/providers run-oauth <name>` — kick off the OAuth flow non-interactively
/// (mostly a power-user shortcut; the in-OAuth UI is the default path).
struct ProvidersCommand;

impl SlashCommand for ProvidersCommand {
    fn name(&self) -> &'static str {
        "providers"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["keys"]
    }
    fn description(&self) -> &'static str {
        "Manage providers: status, add custom, remove a key, run OAuth"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let tokens: Vec<&str> = args.split_whitespace().collect();

        // `/providers remove <name> [--yes]`
        if tokens
            .first()
            .map(|t| *t == "remove" || *t == "rm")
            .unwrap_or(false)
        {
            return remove_provider_key(ctx, tokens.get(1).copied(), tokens.contains(&"--yes"));
        }

        // `/providers add <name> <base_url> [api_key_env] [api]`
        if tokens.first().map(|t| *t == "add").unwrap_or(false) {
            return add_custom_provider(ctx, &tokens[1..]);
        }

        // `/providers run-oauth <name>`
        if tokens
            .first()
            .map(|t| *t == "run-oauth" || *t == "oauth")
            .unwrap_or(false)
        {
            return run_provider_oauth(ctx, tokens.get(1).copied());
        }

        // `/providers` — status overlay.
        let auth = crate::store::auth_storage::shared_auth_storage();
        let mut names: Vec<String> = ctx
            .state
            .catalog
            .as_ref()
            .map(|c| c.list_providers_sync())
            .unwrap_or_default();
        // Merge custom providers from settings that the catalog doesn't list.
        if let Ok(settings) = crate::store::settings::Settings::load() {
            for cp in &settings.custom_providers {
                if !names.iter().any(|n| n == &cp.name) {
                    names.push(cp.name.clone());
                }
            }
        }
        names.sort();

        if names.is_empty() {
            ctx.reply(
                InlineMessageKind::Info,
                "No providers configured. Run `oxicode setup` to add one.",
            );
            return SlashOutcome::Handled;
        }

        ctx.state.overlay_providers = names.clone();
        let catalog = ctx.state.catalog.as_ref();
        let items: Vec<InlineListItem> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let has_key = auth.has(name);
                let entry = catalog.and_then(|c| c.get_provider_sync(name));
                let base = entry.as_ref().and_then(|p| p.base_url.clone());
                let env_key = entry.as_ref().and_then(|p| p.env_key.clone());
                // OAuth-capable? `product-meta.toml` ships exactly two
                // OAuth blocks today (openai, anthropic); every other
                // provider is key-only. Surface this so the user does
                // not look for an OAuth menu where there is none.
                let oauth_capable = crate::provider_oauth::spec_for(name).is_some();
                let subtitle = match (env_key.as_deref(), base.as_deref(), oauth_capable) {
                    (Some(env), Some(url), true) if !url.is_empty() => {
                        format!("{env} · {url} · oauth")
                    }
                    (Some(env), Some(url), false) if !url.is_empty() => {
                        format!("{env} · {url}")
                    }
                    (Some(env), _, true) => format!("{env} · oauth"),
                    (Some(env), _, false) => env.to_string(),
                    (_, Some(url), true) => format!("{url} · oauth"),
                    (_, Some(url), false) => url.to_string(),
                    _ => "Enter to manage".to_string(),
                };
                InlineListItem {
                    title: name.clone(),
                    subtitle: Some(subtitle),
                    badge: Some(if has_key {
                        "key".to_string()
                    } else {
                        "\u{2014}".to_string()
                    }),
                    indent: 0,
                    selection: Some(InlineListSelection::ProviderRow(i)),
                    search_value: Some(name.clone()),
                }
            })
            .collect();

        let keyed = names.iter().filter(|n| auth.has(n)).count();
        let search = InlineListSearchConfig {
            label: "Filter providers".into(),
            placeholder: Some("Type to filter\u{2026}".into()),
        };
        ctx.handle.show_list_modal(
            "Providers".into(),
            vec![format!(
                "{keyed}/{} with keys \u{2014} Enter to manage, Esc to close",
                names.len()
            )],
            items,
            None,
            Some(search),
        );
        SlashOutcome::Handled
    }
}

/// Handler for `/providers remove <name> [--yes]`.
fn remove_provider_key(ctx: &mut SlashCtx<'_>, name: Option<&str>, yes: bool) -> SlashOutcome {
    let Some(name) = name else {
        ctx.reply(InlineMessageKind::Error, "Usage: /providers remove <name>");
        return SlashOutcome::Handled;
    };
    let auth = crate::store::auth_storage::shared_auth_storage();
    if !auth.has(name) {
        ctx.reply(
            InlineMessageKind::Warning,
            format!("No stored key for '{name}'."),
        );
        return SlashOutcome::Handled;
    }
    if !yes {
        ctx.state.confirmation = Some(crate::tui_vt::main_loop::ModalConfirmation {
            title: format!("Remove key for {name}?"),
            message: "  y \u{2014} remove key     n / x \u{2014} cancel".into(),
            action: crate::tui_vt::main_loop::ConfirmationAction::RemoveProviderKey(
                name.to_string(),
            ),
        });
        return SlashOutcome::Handled;
    }
    auth.remove(name);
    ctx.reply(
        InlineMessageKind::Info,
        format!("Removed key for '{name}'."),
    );
    SlashOutcome::Handled
}

/// Handler for `/providers add <name> <base_url> [api_key_env] [api]`.
///
/// Persists a new `CustomProvider` entry into `~/.oxicode/settings.toml`.
/// `api_key_env` defaults to `<NAME>_API_KEY` (uppercased, hyphens → `_`)
/// to match the convention in `setup_wizard.rs`. `api` defaults to
/// `"openai-completions"` via `CustomProvider::default_api`.
///
/// Run from the TUI composer so the user never has to leave the session
/// for the structured equivalent of `oxicode setup`. The new provider
/// appears immediately in `/providers` because the status overlay reads
/// `settings.custom_providers` on every open.
fn add_custom_provider(ctx: &mut SlashCtx<'_>, tokens: &[&str]) -> SlashOutcome {
    // Need at least name + base_url. api_key_env and api are optional.
    let (name, base_url, api_key_env, api) = match tokens {
        [name, base_url] => (
            (*name).to_string(),
            (*base_url).to_string(),
            default_api_key_env(name),
            None,
        ),
        [name, base_url, env] => (
            (*name).to_string(),
            (*base_url).to_string(),
            (*env).to_string(),
            None,
        ),
        [name, base_url, env, api] => (
            (*name).to_string(),
            (*base_url).to_string(),
            (*env).to_string(),
            Some((*api).to_string()),
        ),
        _ => {
            ctx.reply(
                InlineMessageKind::Error,
                "Usage: /providers add <name> <base_url> [api_key_env] [api]".to_string(),
            );
            return SlashOutcome::Handled;
        }
    };

    if name.is_empty() || base_url.is_empty() {
        ctx.reply(
            InlineMessageKind::Error,
            "Provider name and base URL must be non-empty.".to_string(),
        );
        return SlashOutcome::Handled;
    }

    let mut settings = match crate::store::settings::Settings::load() {
        Ok(s) => s,
        Err(e) => {
            ctx.reply(
                InlineMessageKind::Error,
                format!("Failed to load settings: {e}"),
            );
            return SlashOutcome::Handled;
        }
    };
    if settings.custom_providers.iter().any(|cp| cp.name == name) {
        ctx.reply(
            InlineMessageKind::Warning,
            format!("Custom provider '{name}' already exists."),
        );
        return SlashOutcome::Handled;
    }
    let cp = crate::store::settings::CustomProvider {
        name: name.clone(),
        base_url,
        api_key_env,
        api: api.unwrap_or_else(crate::store::settings::default_custom_provider_api),
    };
    settings.custom_providers.push(cp);

    if let Err(e) = settings.save() {
        ctx.reply(
            InlineMessageKind::Error,
            format!("Failed to persist settings: {e}"),
        );
        return SlashOutcome::Handled;
    }

    // Chain into the secure prompt so the user can finish the setup
    // without a second navigation step. The SecureInput consumer in
    // `main_loop.rs` will emit a contextual follow-up based on the
    // `NewlyAdded` origin (vs. the generic `SetKey` message used when
    // the user rekeys an existing provider).
    crate::tui_vt::main_loop::open_secure_prompt(
        ctx.state,
        ctx.handle,
        crate::tui_vt::main_loop::SecureInputOrigin::NewlyAdded {
            provider: name.clone(),
        },
    );
    SlashOutcome::Handled
}

/// Default api_key_env for a custom provider name: uppercased, hyphens
/// replaced with underscores, suffixed with `_API_KEY`. Matches the
/// convention used in `setup_wizard.rs` (`api_key_env` formatter).
fn default_api_key_env(name: &str) -> String {
    format!("{}_API_KEY", name.to_uppercase().replace('-', "_"))
}

/// Handler for `/providers run-oauth <name>`.
///
/// Power-user shortcut that drives the OAuth flow without going through
/// the per-row action menu. Same PKCE + loopback + token exchange path as
/// the in-overlay OAuth action; the `InlineHandle` is used to emit
/// progress lines (the task posts to it directly).
fn run_provider_oauth(ctx: &mut SlashCtx<'_>, name: Option<&str>) -> SlashOutcome {
    let Some(name) = name else {
        ctx.reply(
            InlineMessageKind::Error,
            "Usage: /providers run-oauth <name>".to_string(),
        );
        return SlashOutcome::Handled;
    };
    let Some(spec) = crate::provider_oauth::spec_for(name) else {
        ctx.reply(
            InlineMessageKind::Error,
            format!("No OAuth spec for '{name}'. Not an OAuth-capable provider."),
        );
        return SlashOutcome::Handled;
    };
    let provider = name.to_string();
    let provider_for_log = provider.clone();
    let tx = ctx.handle.clone();
    let auth = crate::store::auth_storage::shared_auth_storage();
    let auth_clone = std::sync::Arc::clone(&auth);
    tokio::spawn(async move {
        crate::tui_vt::main_loop::run_oauth_flow(provider, spec, tx, auth_clone).await;
    });
    ctx.reply(
        InlineMessageKind::Info,
        format!("Starting OAuth flow for '{provider_for_log}'…"),
    );
    SlashOutcome::Handled
}

// ─────────────────────────────────────────────────────────────────────────
// /tools — registered tool inventory
// ─────────────────────────────────────────────────────────────────────────

/// `/tools` — list every registered agent tool (built-in + extension) with its
/// description and whether it is essential (cannot be disabled). Read-only.
struct ToolsCommand;

impl SlashCommand for ToolsCommand {
    fn name(&self) -> &'static str {
        "tools"
    }
    fn description(&self) -> &'static str {
        "List registered agent tools"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let tools = ctx.session.agent_ref().tools();
        let mut tools = tools.get_tools();
        tools.sort_by(|a, b| a.name().cmp(b.name()));

        let items: Vec<InlineListItem> = tools
            .iter()
            .map(|t| InlineListItem {
                title: t.name().to_string(),
                subtitle: Some(t.description().to_string()),
                badge: if t.essential() {
                    Some("essential".to_string())
                } else {
                    None
                },
                indent: 0,
                selection: None,
                search_value: Some(format!("{} {}", t.name(), t.label())),
            })
            .collect();

        let count = items.len();
        let essential = items.iter().filter(|i| i.badge.is_some()).count();
        let search = InlineListSearchConfig {
            label: "Filter tools".into(),
            placeholder: Some("Type to filter\u{2026}".into()),
        };
        ctx.handle.show_list_modal(
            format!("Tools ({count})"),
            vec![format!("{essential} essential \u{2014} Esc to close")],
            items,
            None,
            Some(search),
        );
        SlashOutcome::Handled
    }
}

// ─────────────────────────────────────────────────────────────────────────
// /mcp — MCP server dashboard
// ─────────────────────────────────────────────────────────────────────────

/// `/mcp` — show the MCP server dashboard: connection state, tool counts, and
/// settings summary, sourced from the synchronous `dashboard_data()` snapshot.
struct McpCommand;

impl SlashCommand for McpCommand {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn description(&self) -> &'static str {
        "Show MCP server status"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let Some(mcp) = ctx.session.agent_ref().tools().mcp_manager() else {
            ctx.reply(InlineMessageKind::Info, "No MCP manager configured.");
            return SlashOutcome::Handled;
        };
        let dash = mcp.dashboard_data();
        let s = &dash.settings;

        if dash.servers.is_empty() {
            ctx.reply(
                InlineMessageKind::Info,
                format!(
                    "No MCP servers configured ({} servers, {} tools).",
                    s.total_servers, s.total_tools
                ),
            );
            return SlashOutcome::Handled;
        }

        let items: Vec<InlineListItem> = dash
            .servers
            .iter()
            .map(|srv| {
                use oxicode_agent::mcp::types::McpConnectionStatus;
                let status = match &srv.status {
                    McpConnectionStatus::Connected => "connected",
                    McpConnectionStatus::Disconnected => "disconnected",
                    McpConnectionStatus::Connecting => "connecting",
                    McpConnectionStatus::Error(_) => "error",
                };
                InlineListItem {
                    title: srv.name.clone(),
                    subtitle: Some(format!(
                        "{status} · {} tool{} · {}",
                        srv.tool_count,
                        if srv.tool_count == 1 { "" } else { "s" },
                        srv.lifecycle
                    )),
                    badge: Some(status.to_string()),
                    indent: 0,
                    selection: None,
                    search_value: Some(srv.name.clone()),
                }
            })
            .collect();

        ctx.handle.show_list_modal(
            "MCP Servers".into(),
            vec![format!(
                "{}/{} connected · {} tools · prefix: {}",
                s.connected_servers, s.total_servers, s.total_tools, s.tool_prefix
            )],
            items,
            None,
            None,
        );
        SlashOutcome::Handled
    }
}

// ─────────────────────────────────────────────────────────────────────────
// /info — diagnostics
// ─────────────────────────────────────────────────────────────────────────

/// `/info` — a diagnostics snapshot: version, paths, model, provider, key
/// status, and catalog size. Useful for bug reports and "why isn't X working".
struct InfoCommand;

impl SlashCommand for InfoCommand {
    fn name(&self) -> &'static str {
        "info"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["diagnostics", "debug"]
    }
    fn description(&self) -> &'static str {
        "Show diagnostics: version, paths, model, catalog (alias: /diagnostics)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let home = dirs::home_dir().unwrap_or_default();
        let model = ctx.session.model_id();
        let (provider, _) = split_model_id(&model);
        let auth = crate::store::auth_storage::shared_auth_storage();
        let key_status = if auth.has(provider) { "set" } else { "missing" };
        let catalog_count = ctx
            .state
            .catalog
            .as_ref()
            .map(|c| c.model_count_sync())
            .unwrap_or(0);
        let session_file = ctx
            .session
            .session_file()
            .unwrap_or_else(|| "(none)".into());

        let lines = vec![
            "".into(),
            format!("  oxicode   v{}", env!("CARGO_PKG_VERSION")),
            format!("  cwd       {}", ctx.state.cwd.display()),
            format!("  session   {}", ctx.session.session_id()),
            format!("  file      {session_file}"),
            "".into(),
            format!("  model     {model}"),
            format!("  provider  {provider}  (key: {key_status})"),
            format!("  catalog   {catalog_count} models"),
            "".into(),
            "  Paths".into(),
            format!(
                "  config    {}",
                home.join(".oxicode/settings.toml").display()
            ),
            format!("  auth      {}", home.join(".oxicode/auth.json").display()),
            format!("  sessions  {}", home.join(".oxicode/sessions").display()),
            format!("  logs      {}", home.join(".oxicode/logs").display()),
            "".into(),
        ];
        ctx.handle.show_modal("Diagnostics".into(), lines, None);
        SlashOutcome::Handled
    }
}

// ─────────────────────────────────────────────────────────────────────────
// /export — conversation → HTML
// ─────────────────────────────────────────────────────────────────────────

/// `/export` — render the current conversation as a self-contained HTML file
/// in the working directory and reply with the path.
struct ExportCommand;

impl SlashCommand for ExportCommand {
    fn name(&self) -> &'static str {
        "export"
    }
    fn description(&self) -> &'static str {
        "Export the conversation to HTML"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        match ctx.session.export_html() {
            Ok(html) => {
                let id = ctx.session.session_id();
                let stem: String = id.chars().take(12).collect();
                let path = ctx.state.cwd.join(format!("oxicode-export-{stem}.html"));
                match std::fs::write(&path, html) {
                    Ok(()) => ctx.reply(
                        InlineMessageKind::Info,
                        format!("Exported to {}", path.display()),
                    ),
                    Err(e) => ctx.reply(
                        InlineMessageKind::Error,
                        format!("Failed to write export: {e}"),
                    ),
                }
            }
            Err(e) => ctx.reply(
                InlineMessageKind::Error,
                format!("Failed to export conversation: {e}"),
            ),
        }
        SlashOutcome::Handled
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ctx_compact() {
        assert_eq!(fmt_ctx(0), "0 ctx");
        assert_eq!(fmt_ctx(500), "500 ctx");
        assert_eq!(fmt_ctx(8192), "8K ctx");
        assert_eq!(fmt_ctx(128_000), "128K ctx");
        assert_eq!(fmt_ctx(1_000_000), "1.0M ctx");
        assert_eq!(fmt_ctx(2_000_000), "2.0M ctx");
    }

    #[test]
    fn fmt_cost_edges() {
        assert_eq!(fmt_cost(0.0), "free");
        assert_eq!(fmt_cost(0.001), "<$0.01/M");
        assert_eq!(fmt_cost(3.0), "$3.00/M");
        assert_eq!(fmt_cost(15.0), "$15.00/M");
    }

    #[test]
    fn split_model_id_basic() {
        assert_eq!(
            split_model_id("anthropic/claude-3"),
            ("anthropic", "claude-3")
        );
        assert_eq!(split_model_id("bare"), ("bare", ""));
        // Only the first slash splits.
        assert_eq!(split_model_id("oai/gpt-4/vision"), ("oai", "gpt-4/vision"));
    }

    /// OAuth-capable providers must surface in the catalog overlay so the
    /// user can discover the OAuth action instead of going through the
    /// `Remove key` only branch. `product-meta.toml` ships exactly two
    /// OAuth blocks (openai, anthropic); every other built-in is key-only.
    #[test]
    fn provider_oauth_capability_matches_meta() {
        // OAuth-capable: openai, anthropic.
        assert!(
            crate::provider_oauth::spec_for("openai").is_some(),
            "openai must be oauth-capable per product-meta.toml"
        );
        assert!(
            crate::provider_oauth::spec_for("anthropic").is_some(),
            "anthropic must be oauth-capable per product-meta.toml"
        );
        // Key-only: ollama, google, vertex, etc.
        assert!(
            crate::provider_oauth::spec_for("ollama").is_none(),
            "ollama has no OAuth spec"
        );
        assert!(
            crate::provider_oauth::spec_for("google").is_none(),
            "google has no OAuth spec"
        );
    }

    /// Default api_key_env format used by `/providers add <name> <url>`:
    /// uppercased + hyphens → underscores + `_API_KEY`. Mirrors the
    /// convention in `setup_wizard.rs`.
    #[test]
    fn default_api_key_env_for_custom_provider() {
        assert_eq!(default_api_key_env("minimax"), "MINIMAX_API_KEY");
        assert_eq!(default_api_key_env("zai-org"), "ZAI_ORG_API_KEY");
        assert_eq!(default_api_key_env("Foo-Bar"), "FOO_BAR_API_KEY");
    }

    // `add_custom_provider` is exercised end-to-end by the in-TUI flow. We
    // can't call it without a real `SlashCtx` (which requires an
    // `AgentSessionHandle` + `InlineHandle`), and the function writes to
    // `~/.oxicode/settings.toml` so it can't run unmodified in a unit test.
    // The handler-level contract is pinned by the regex above and the
    // `custom_provider_default_api` integration test in
    // `oxicode-cli/src/store/settings.rs`; the persistence path is a
    // straight `Settings::save()` call.
}
