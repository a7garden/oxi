//! Application bootstrap and run-mode dispatch.
//!
//! Owns: log init, app building (settings → custom providers → router →
//! tools → WASM), and run-mode dispatch (TUI / print / RPC).
//!
//! The helper functions below are moved verbatim from main.rs and
//! retain their original signatures.

use crate::cli::CliArgs;
use crate::print_mode;
use crate::store::settings::Settings;
use anyhow::Result;
use std::path::PathBuf;
use tracing;

/// Build a wired `App` from CLI args. All the wiring that used to be
/// inline in `main()` lives here.
pub async fn build_app(args: &CliArgs) -> Result<crate::App> {
    // Layer 2.5 / Catalog Port (v3): the `FileModelCatalog` wired in
    // `services::build_oxi` performs its own init at `OxiBuilder::build`
    // time — it loads the embedded SNAP, applies overrides, and attempts
    // one refresh if the cache is stale. So we no longer call the legacy
    // `init_models_dev()` here. To skip network access during boot, set
    // `OXI_MODELS_DEV_DISABLE_FETCH=1`.

    // Load settings (global + project + env layers).
    let mut settings = Settings::load().unwrap_or_default();

    // Apply CLI overrides.
    settings.merge_cli(
        args.model.clone(),
        args.provider.clone(),
        Some(args.enable_routing),
        Some(args.prefer_cost_efficient),
        if args.fallback_chain.is_empty() {
            None
        } else {
            Some(args.fallback_chain.clone())
        },
        Some(args.disable_fallback),
    );

    if settings
        .effective_model(None)
        .unwrap_or_default()
        .is_empty()
    {
        eprintln!(
            "{}",
            print_mode::format_error("No model configured. Run `oxi setup` to configure.")
        );
        std::process::exit(1);
    }

    // Register custom OpenAI-compatible providers from settings.
    register_custom_providers(&settings);

    // Register model router (opt-in).
    register_router_provider(&settings);

    // Apply thinking level if specified.
    if let Some(ref level_str) = args.thinking {
        if let Some(level) = crate::store::settings::parse_thinking_level(level_str) {
            settings.thinking_level = level;
        } else {
            anyhow::bail!(
                "Invalid thinking level: {}. Valid options: off, minimal, low, medium, high, xhigh",
                level_str
            );
        }
    }

    // Build the wired Oxi engine + Agent via the SDK composition root.
    let oxi = crate::build_oxi_engine().await?;

    // Per-process liveness identity for issue-system ownership. In TUI mode
    // we use the canonical "tui" id so the agent tool, the TUI panel, and
    // the `/issue` slash command all share the same flock holder. In any
    // non-TUI mode (print, RPC, single-prompt) we generate a stable
    // process-scoped id; that way concurrent ownership checks see this
    // process as a single coherent owner rather than an empty caller.
    let ownership_session_id = if is_tui_mode(args) {
        crate::store::issues::liveness::TUI_OWNERSHIP_ID.to_string()
    } else {
        format!(
            "proc-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        )
    };

    // Spawn the catalog event logger so refresh / override / local-discovery
    // events show up in the log file. UI hooks can subscribe to
    // `oxi.catalog().subscribe()` separately for picker invalidation.
    let _catalog_logger =
        crate::services::spawn_catalog_event_logger(std::sync::Arc::clone(oxi.catalog()));

    let mut app = crate::App::from_oxi(oxi, settings, ownership_session_id).await?;

    // v2.2: wire the MCP credential provider (OAuth2 client_credentials).
    // Reads the same `mcp.json` files the agent uses, picks every server
    // with an `oauth` block, and gives the manager a provider that can
    // obtain + refresh access tokens on demand. No-op when no server
    // declares `oauth`.
    let mcp_cfg = oxi_agent::mcp::config::load_mcp_config();
    let mut oauth_map: std::collections::HashMap<String, oxi_agent::mcp::types::OAuthConfig> =
        std::collections::HashMap::new();
    for (name, entry) in &mcp_cfg.mcp_servers {
        if let Some(oc) = entry.oauth.clone() {
            oauth_map.insert(name.clone(), oc);
        }
    }
    if !oauth_map.is_empty()
        && let Some(manager) = app.agent_tools().mcp_manager()
    {
        let config_dir = dirs::config_dir()
            .map(|d| d.join("oxi"))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        match crate::mcp_credentials::FileMcpCredentialProvider::new(oauth_map, config_dir) {
            Ok(provider) => {
                manager.set_credential_provider(provider);
            }
            Err(e) => {
                tracing::warn!("Failed to construct MCP credential provider: {}", e);
            }
        }
    }

    // Register built-in tools on the agent's tool registry.
    let tools = app.agent_tools();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    register_builtin_tools(&tools, &cwd, args, &app.settings().disabled_tools);

    // Discover and load WASM extensions.
    let wasm_ext = load_wasm_extensions(&app, &cwd, &tools);
    app.set_wasm_ext(wasm_ext);

    // Handle --append-system-prompt.
    if let Some(ref prompt_path) = args.append_system_prompt {
        let content = std::fs::read_to_string(prompt_path)
            .map_err(|e| anyhow::anyhow!("Failed to read system prompt file: {}", e))?;
        app.agent().set_system_prompt(content);
    }

    Ok(app)
}

/// Dispatch the run mode: TUI / print / RPC, based on the CLI flags.
pub async fn dispatch_run_mode(args: &CliArgs, app: crate::App) -> Result<i32> {
    let prompt = args.prompt.join(" ");

    if args.mode.as_deref() == Some("json") || args.print {
        let mode = if args.mode.as_deref() == Some("json") {
            crate::print_mode::PrintMode::Json
        } else {
            crate::print_mode::PrintMode::Text
        };
        let options = crate::print_mode::PrintModeOptions {
            mode,
            initial_message: if prompt.is_empty() {
                None
            } else {
                Some(prompt)
            },
            messages: vec![],
            no_stdin: args.print,
            no_session: args.print || args.no_session,
            quiet: args.print,
            timeout: args.timeout,
        };
        return crate::print_mode::run_print_mode(&app, options).await;
    }

    if prompt.is_empty() || args.interactive {
        if args.continue_session {
            crate::tui::run_tui_interactive_with_continue(app, true).await?;
        } else {
            crate::tui::run_tui_interactive(app).await?;
        }
        return Ok(0);
    }

    crate::main_dispatch::run_single_prompt(app, &prompt).await?;
    Ok(0)
}

/// Parse args, build the app, dispatch.
pub async fn run_with_args(args: CliArgs) -> Result<i32> {
    let app = build_app(&args).await?;
    dispatch_run_mode(&args, app).await
}

// ─── Helpers (moved verbatim from main.rs) ─────────────────────────────

/// Initialize file-based logging to `~/.cache/oxi/oxi.log`.
///
/// Reads `RUST_LOG` for filter (default: `debug`). Builds a
/// `tracing_subscriber::EnvFilter` and writes to a `Mutex<File>` writer.
pub fn init_logging() {
    let log_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("oxi");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("oxi.log");

    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "debug".to_string());
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_filter));

    let log_file = std::fs::File::create(&log_path).expect("Failed to create log file");
    let writer = std::sync::Mutex::new(log_file);

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(writer)
        .with_target(true)
        .with_thread_ids(true)
        .with_ansi(false)
        .init();

    tracing::info!("Logging initialized, log file: {:?}", log_path);
}

/// Register custom OpenAI-compatible providers from settings and auto-fetch their models.
fn register_custom_providers(settings: &Settings) {
    let auth_storage = crate::store::auth_storage::shared_auth_storage();
    for cp in &settings.custom_providers {
        let api_key = auth_storage.get_api_key(&cp.name);
        let api = cp.api.to_lowercase();

        match api.as_str() {
            "openai-completions" | "openai" => {
                let provider =
                    oxi_ai::OpenAiProvider::with_base_url_and_key(&cp.base_url, api_key.clone());
                oxi_sdk::register_provider(&cp.name, provider);
                tracing::info!(
                    "Registered custom provider '{}' (openai-completions) -> {}",
                    cp.name,
                    cp.base_url
                );
            }
            "openai-responses" | "responses" => {
                let provider = oxi_sdk::OpenAiResponsesProvider::with_base_url_and_key(
                    &cp.base_url,
                    api_key.clone(),
                );
                oxi_sdk::register_provider(&cp.name, provider);
                tracing::info!(
                    "Registered custom provider '{}' (openai-responses) -> {}",
                    cp.name,
                    cp.base_url
                );
            }
            _ => {
                tracing::warn!(
                    "Unknown API type '{}' for custom provider '{}'. Supported: openai-completions, openai-responses",
                    cp.api,
                    cp.name
                );
            }
        }

        fetch_and_register_models(cp, &api, &api_key);
    }
}

/// Fetch models from a custom provider's /v1/models endpoint and register them.
fn fetch_and_register_models(
    cp: &crate::store::settings::CustomProvider,
    api: &str,
    api_key: &Option<String>,
) {
    if let Some(key) = api_key {
        match oxi_sdk::fetch_models_blocking(&cp.base_url, key.as_str()) {
            Ok(model_ids) => {
                let count = model_ids.len();
                for model_id in &model_ids {
                    let api_type = match api {
                        "openai-responses" | "responses" => oxi_sdk::Api::OpenAiResponses,
                        _ => oxi_sdk::Api::OpenAiCompletions,
                    };
                    let model = oxi_sdk::Model {
                        id: model_id.clone(),
                        name: model_id.clone(),
                        api: api_type,
                        provider: cp.name.clone(),
                        base_url: cp.base_url.clone(),
                        reasoning: false,
                        input: vec![oxi_sdk::InputModality::Text],
                        cost: oxi_sdk::Cost::default(),
                        context_window: 128_000,
                        max_tokens: 8_192,
                        headers: Default::default(),
                        compat: None,
                    };
                    oxi_sdk::register_model(model);
                }
                tracing::info!(
                    "[oxi] auto-fetched {} models from '{}' ({})",
                    count,
                    cp.name,
                    cp.base_url
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[oxi] warning: failed to resolve models for {}: {}",
                    cp.name,
                    e
                );
            }
        }
    }
}

/// Register builtin tools with the agent, respecting --tools filter and disabled_tools.
///
/// Also transfers the [`McpManager`](oxi_agent::mcp::McpManager) reference from
/// the built-in registry to the live agent registry. This matters because
/// `register_arc` only copies the `Arc<dyn AgentTool>` — the manager field is
/// stored separately and would otherwise be `None`, making `/mcp` show a
/// "MCP is not configured" warning even though the `McpTool` is registered.
fn register_builtin_tools(
    tools: &oxi_agent::ToolRegistry,
    cwd: &std::path::Path,
    args: &CliArgs,
    disabled_tools: &[String],
) {
    let builtin_registry = if let Some(ref tools_str) = args.tools {
        let names: Vec<&str> = tools_str.split(',').map(|s| s.trim()).collect();
        oxi_agent::ToolRegistry::with_selected_tools(cwd.to_path_buf(), &names)
    } else {
        oxi_agent::ToolRegistry::with_builtins_cwd(cwd.to_path_buf(), disabled_tools)
    };
    for name in builtin_registry.names() {
        if let Some(tool) = builtin_registry.get(&name) {
            tools.register_arc(tool);
        }
    }
    // Propagate the MCP manager so the TUI's `/mcp` overlay can hot-reload
    // configs, render live connection status, and so on.
    if let Some(mgr) = builtin_registry.mcp_manager() {
        tools.set_mcp_manager(mgr);
    }
}

/// Discover and load WASM extensions, registering their tools.
fn load_wasm_extensions(
    app: &crate::App,
    cwd: &std::path::Path,
    tools: &oxi_agent::ToolRegistry,
) -> Option<std::sync::Arc<crate::extensions::WasmExtensionManager>> {
    if !app.settings().extensions_enabled {
        return None;
    }

    let wasm_paths = crate::extensions::WasmExtensionManager::discover(cwd);
    if wasm_paths.is_empty() {
        return None;
    }

    let mut wasm_mgr = crate::extensions::WasmExtensionManager::new();
    let (loaded, errors) = wasm_mgr.load_all(&wasm_paths);
    for info in &loaded {
        tracing::info!("WASM extension loaded: {} v{}", info.name, info.version);
    }
    for err in &errors {
        tracing::warn!("WASM extension error: {}", err);
    }

    if wasm_mgr.is_empty() {
        return None;
    }

    let mgr = std::sync::Arc::new(wasm_mgr);
    for tool_def in mgr.all_tool_defs() {
        let wasm_tool = crate::extensions::WasmTool::new(
            mgr.clone(),
            tool_def.name.clone(),
            tool_def.description.clone(),
            tool_def.schema.clone(),
        );
        tools.register(wasm_tool);
    }
    Some(mgr)
}

/// Register the model auto-router if configured in settings.
fn register_router_provider(settings: &Settings) {
    let global_dir = dirs::config_dir().unwrap_or_default().join("oxi");
    let project_dir = std::env::current_dir().unwrap_or_default();

    let store_cfg = match crate::store::router_config::load_router_config(&global_dir, &project_dir)
    {
        Some(cfg) => cfg,
        None => {
            tracing::debug!("No router config found — router/auto will not appear in model list");
            return;
        }
    };

    // Register router models only when configured.
    oxi_sdk::register_model(oxi_sdk::Model::new(
        "auto",
        "Router (auto)".to_string(),
        oxi_sdk::Api::AnthropicMessages,
        "router",
        "router://local",
    ));

    // Convert store config to AI config.
    let mut ai_profiles = std::collections::HashMap::new();
    for (name, sp) in store_cfg.profiles() {
        fn parse_thinking(s: &Option<String>) -> Option<oxi_sdk::ThinkingLevel> {
            s.as_ref().and_then(|s| match s.as_str() {
                "off" => Some(oxi_sdk::ThinkingLevel::Off),
                "minimal" => Some(oxi_sdk::ThinkingLevel::Minimal),
                "low" => Some(oxi_sdk::ThinkingLevel::Low),
                "medium" => Some(oxi_sdk::ThinkingLevel::Medium),
                "high" => Some(oxi_sdk::ThinkingLevel::High),
                "xhigh" => Some(oxi_sdk::ThinkingLevel::XHigh),
                _ => None,
            })
        }
        ai_profiles.insert(
            name.clone(),
            oxi_sdk::router::RouterProfile {
                high: oxi_sdk::router::RoutedTierConfig {
                    model: sp.high.model.clone(),
                    thinking: parse_thinking(&sp.high.thinking),
                    fallbacks: sp.high.fallbacks.clone(),
                },
                medium: oxi_sdk::router::RoutedTierConfig {
                    model: sp.medium.model.clone(),
                    thinking: parse_thinking(&sp.medium.thinking),
                    fallbacks: sp.medium.fallbacks.clone(),
                },
                low: oxi_sdk::router::RoutedTierConfig {
                    model: sp.low.model.clone(),
                    thinking: parse_thinking(&sp.low.thinking),
                    fallbacks: sp.low.fallbacks.clone(),
                },
            },
        );
    }
    let ai_cfg = oxi_sdk::router::RouterConfig::with_pinning(
        store_cfg.default_profile().to_string(),
        store_cfg.classifier_model().map(String::from),
        store_cfg.context_upgrade_threshold(),
        store_cfg.max_session_budget(),
        ai_profiles,
        oxi_sdk::router::ScoringWeights {
            structural: store_cfg.weights().structural,
            behavioral: store_cfg.weights().behavioral,
            context_budget: store_cfg.weights().context_budget,
            vision: store_cfg.weights().vision,
            message: store_cfg.weights().message,
        },
        store_cfg.pin_tier().and_then(|s| match s {
            "high" => Some(oxi_sdk::router::RouterTier::High),
            "medium" => Some(oxi_sdk::router::RouterTier::Medium),
            "low" => Some(oxi_sdk::router::RouterTier::Low),
            _ => None,
        }),
        store_cfg.phase_bias(),
    );

    oxi_sdk::router::register_router(&ai_cfg);

    if let Some(profile) = settings.router_profile() {
        tracing::info!("Router active with profile: {profile}");
    }
}

/// Decide whether this run is the TUI (interactive) mode. Mirrors the
/// dispatch in [`dispatch_run_mode`]: print / RPC / single-prompt are
/// non-TUI. Used by [`build_app`] to pick the canonical liveness identity.
fn is_tui_mode(args: &CliArgs) -> bool {
    if args.mode.as_deref() == Some("json") || args.print {
        return false;
    }
    // prompt-only (no `--interactive` and non-empty prompt) is non-TUI too;
    // dispatch_run_mode sends it through main_dispatch::run_single_prompt.
    if !args.interactive && !args.prompt.is_empty() {
        return false;
    }
    true
}
