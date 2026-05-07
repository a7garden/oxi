//! oxi CLI - CLI coding harness for oxi
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::Parser;
use oxi::cli::{CliArgs, Commands, ConfigCommands, PkgCommands};
use oxi::extensions::ExtensionRegistry;
use oxi::packages::{PackageManager, ResourceKind};
use oxi::session::{AgentMessage, SessionManager};
use oxi::settings::Settings;
use std::path::{Path, PathBuf};
use uuid::Uuid;



#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Parse arguments (using unified CliArgs from cli module)
    let args = CliArgs::parse();

    // Handle subcommands
    if let Some(command) = &args.command {
        return handle_subcommand(command).await;
    }

    // Load settings (global + project + env layers)
    let mut settings = Settings::load().unwrap_or_default();

    // Apply CLI overrides
    settings.merge_cli(args.model.clone(), args.provider.clone());

    // Validate settings
    let report = settings.validate();
    for warn in &report.warnings {
        tracing::warn!("설정 경고: {} - {}", warn.field, warn.message);
    }
    if !report.is_valid() {
        eprintln!("❌ 설정 오류 {}건:", report.errors.len());
        for err in &report.errors {
            eprintln!("   • {}: {}", err.field, err.message);
        }
        std::process::exit(1);
    }

    // Register custom OpenAI-compatible providers from settings
    for cp in &settings.custom_providers {
        let api_key = std::env::var(&cp.api_key_env).ok();
        let api = cp.api.to_lowercase();

        match api.as_str() {
            "openai-completions" | "openai" => {
                let provider = oxi_ai::OpenAiProvider::with_base_url_and_key(
                    &cp.base_url,
                    api_key.clone(),
                );
                oxi_ai::register_provider(&cp.name, provider);
                tracing::info!("Registered custom provider '{}' (openai-completions) -> {}", cp.name, cp.base_url);
            }
            "openai-responses" | "responses" => {
                let provider = oxi_ai::OpenAiResponsesProvider::with_base_url_and_key(
                    &cp.base_url,
                    api_key.clone(),
                );
                oxi_ai::register_provider(&cp.name, provider);
                tracing::info!("Registered custom provider '{}' (openai-responses) -> {}", cp.name, cp.base_url);
            }
            _ => {
                tracing::warn!(
                    "Unknown API type '{}' for custom provider '{}'. Supported: openai-completions, openai-responses",
                    cp.api, cp.name
                );
            }
        }

        // Auto-fetch models from /v1/models endpoint
        if let Some(ref key) = api_key {
            match oxi_ai::fetch_models_blocking(&cp.base_url, key) {
                Ok(model_ids) => {
                    let count = model_ids.len();
                    for model_id in &model_ids {
                        let api_type = match api.as_str() {
                            "openai-responses" | "responses" => oxi_ai::Api::OpenAiResponses,
                            _ => oxi_ai::Api::OpenAiCompletions,
                        };
                        let model = oxi_ai::Model {
                            id: model_id.clone(),
                            name: model_id.clone(),
                            api: api_type,
                            provider: cp.name.clone(),
                            base_url: cp.base_url.clone(),
                            reasoning: false,
                            input: vec![oxi_ai::InputModality::Text],
                            cost: oxi_ai::Cost::default(),
                            context_window: 128_000,
                            max_tokens: 8_192,
                            headers: Default::default(),
                            compat: None,
                        };
                        oxi_ai::register_model(model);
                    }
                    tracing::info!(
                        "[oxi] auto-fetched {} models from '{}' ({})",
                        count, cp.name, cp.base_url
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[oxi] 경고: {} 모델 조회 실패: {}",
                        cp.name, e
                    );
                }
            }
        }
    }

    // Apply thinking level if specified
    if let Some(ref level_str) = args.thinking {
        if let Some(level) = oxi::settings::parse_thinking_level(level_str) {
            settings.thinking_level = level;
        } else {
            anyhow::bail!(
                "Invalid thinking level: {}. Valid options: none, minimal, standard, thorough",
                level_str
            );
        }
    }

    // Load extensions
    let mut ext_registry = ExtensionRegistry::new();
    if !args.extensions.is_empty() {
        let paths: Vec<&Path> = args.extensions.iter().map(|p| p.as_path()).collect();
        let (loaded, errors) = oxi::extensions::load_extensions(&paths);
        for ext in loaded {
            ext_registry.register(ext);
        }
        for err in &errors {
            tracing::warn!("{}", err);
        }
        if !errors.is_empty() {
            anyhow::bail!("{} extension(s) failed to load", errors.len());
        }
    }

    // Build initial prompt if provided
    let prompt = args.prompt.join(" ");

    // Create app
    let app = oxi::App::new(settings).await?;

    // Register builtin tools, respecting --tools filter
    let tools = app.agent_tools();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let builtin_registry = if let Some(ref tools_str) = args.tools {
        let names: Vec<&str> = tools_str.split(',').map(|s| s.trim()).collect();
        oxi_agent::ToolRegistry::with_selected_tools(cwd.clone(), &names)
    } else {
        oxi_agent::ToolRegistry::with_builtins_cwd(cwd.clone())
    };
    for name in builtin_registry.names() {
        if let Some(tool) = builtin_registry.get(&name) {
            tools.register_arc(tool);
        }
    }

    // Register extension tools with the agent
    for tool in ext_registry.all_tools() {
        tools.register_arc(tool);
    }

    // Handle --append-system-prompt
    if let Some(ref prompt_path) = args.append_system_prompt {
        let content = std::fs::read_to_string(prompt_path)
            .map_err(|e| anyhow::anyhow!("Failed to read system prompt file: {}", e))?;
        app.agent().set_system_prompt(content);
    }

    // Route to appropriate mode
    if args.mode.as_deref() == Some("json") || args.print {
        let mode = if args.mode.as_deref() == Some("json") {
            oxi::print_mode::PrintMode::Json
        } else {
            oxi::print_mode::PrintMode::Text
        };
        let options = oxi::print_mode::PrintModeOptions {
            mode,
            initial_message: if prompt.is_empty() { None } else { Some(prompt) },
            messages: vec![],
        };
        let exit_code = oxi::print_mode::run_print_mode(&app, options).await?;
        std::process::exit(exit_code);
    } else if prompt.is_empty() || args.interactive {
        // TUI interactive mode
        oxi::tui::run_tui_interactive(app).await?;
    } else {
        // Single prompt mode
        run_single_prompt(app, &prompt).await?;
    }

    Ok(())
}

/// Handle session-related commands
async fn handle_subcommand(command: &Commands) -> Result<()> {
    match command {
        Commands::Sessions => {
            let manager = SessionManager::new().await?;
            list_sessions(&manager).await?;
        }
        Commands::Tree { session_id } => {
            let manager = SessionManager::new().await?;
            show_tree(&manager, session_id).await?;
        }
        Commands::Fork {
            parent_id,
            entry_id,
        } => {
            let manager = SessionManager::new().await?;
            fork_session(&manager, parent_id, entry_id).await?;
        }
        Commands::Delete { session_id } => {
            let manager = SessionManager::new().await?;
            delete_session(&manager, session_id).await?;
        }
        Commands::Pkg { action } => {
            handle_pkg_command(action)?;
        }
        Commands::Config { action } => {
            handle_config_command(action)?;
        }
        Commands::Models { provider } => {
            handle_models_command(provider)?;
        }
        Commands::Setup { reset } => {
            handle_setup_command(*reset)?;
        }
    }

    Ok(())
}

fn handle_pkg_command(action: &PkgCommands) -> Result<()> {
    let mut mgr = PackageManager::new()?;

    match action {
        PkgCommands::Install { source } => {
            if source.starts_with("npm:") {
                let name = source.strip_prefix("npm:")
                    .ok_or_else(|| anyhow::anyhow!("Invalid npm source format: {}", source))?;
                let manifest = mgr.install_npm(name)?;
                let counts = mgr.resource_counts(&manifest.name).unwrap_or_default();
                println!(
                    "Installed {} v{} ({})",
                    manifest.name, manifest.version, counts
                );
            } else {
                let manifest = mgr.install(source)?;
                let counts = mgr.resource_counts(&manifest.name).unwrap_or_default();
                println!(
                    "Installed {} v{} ({})",
                    manifest.name, manifest.version, counts
                );
            }
        }
        PkgCommands::List => {
            let packages = mgr.list();
            if packages.is_empty() {
                println!("No packages installed.");
            } else {
                println!(
                    "{:<30} {:<10} {:<15} {}",
                    "NAME", "VERSION", "RESOURCES", "INSTALL DIR"
                );
                println!("{:-<30} {:-<10} {:-<15} {:-<40}", "", "", "", "");
                for pkg in packages {
                    let counts = mgr.resource_counts(&pkg.name).unwrap_or_default();
                    let install_dir = mgr
                        .get_install_dir(&pkg.name)
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "-".to_string());
                    println!(
                        "{:<30} {:<10} {:<15} {}",
                        pkg.name, pkg.version, counts, install_dir
                    );

                    // Show discovered resources
                    if let Ok(resources) = mgr.discover_resources(&pkg.name) {
                        for r in &resources {
                            println!("    {} {}", r.kind, r.relative_path);
                        }
                    }
                }
            }
        }
        PkgCommands::Uninstall { name } => {
            mgr.uninstall(name)?;
            println!("Uninstalled {}", name);
        }
        PkgCommands::Update { name } => match name {
            Some(pkg_name) => {
                let manifest = mgr.update(pkg_name)?;
                println!("Updated {} to v{}", manifest.name, manifest.version);
            }
            None => {
                let packages: Vec<String> = mgr.list().iter().map(|p| p.name.clone()).collect();
                if packages.is_empty() {
                    println!("No packages to update.");
                } else {
                    for pkg_name in &packages {
                        match mgr.update(pkg_name) {
                            Ok(manifest) => {
                                println!("Updated {} to v{}", manifest.name, manifest.version);
                            }
                            Err(e) => {
                                eprintln!("Failed to update {}: {}", pkg_name, e);
                            }
                        }
                    }
                }
            }
        },
    }

    Ok(())
}

/// Parse a resource type string into a ResourceKind
fn parse_resource_type(s: &str) -> Option<ResourceKind> {
    match s.to_lowercase().as_str() {
        "extension" | "extensions" | "ext" => Some(ResourceKind::Extension),
        "skill" | "skills" => Some(ResourceKind::Skill),
        "prompt" | "prompts" => Some(ResourceKind::Prompt),
        "theme" | "themes" => Some(ResourceKind::Theme),
        _ => None,
    }
}

/// Parse a boolean value from a config string
fn parse_config_bool(s: &str) -> Result<bool> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => anyhow::bail!(
            "Invalid boolean value: '{}'. Use true/false, yes/no, on/off, or 1/0",
            s
        ),
    }
}

fn handle_config_command(action: &ConfigCommands) -> Result<()> {
    match action {
        ConfigCommands::Show => {
            let settings = Settings::load()?;
            println!("oxi configuration:");
            println!("  Settings file: {}", Settings::settings_path()?.display());
            println!();
            println!("  Model: {}", settings.effective_model(None));
            println!("  Provider: {}", settings.effective_provider(None));
            println!("  Theme: {}", settings.theme);
            println!("  Thinking: {:?}", settings.thinking_level);
            println!("  Extensions enabled: {}", settings.extensions_enabled);
            println!("  Stream responses: {}", settings.stream_responses);
            println!("  Auto-compaction: {}", settings.auto_compaction);
            println!("  Tool timeout: {}s", settings.tool_timeout_seconds);

            let resource_types = [
                ("Extensions", &settings.extensions),
                ("Skills", &settings.skills),
                ("Prompts", &settings.prompts),
                ("Themes", &settings.themes),
            ];

            for (label, list) in &resource_types {
                if list.is_empty() {
                    println!("  {}: (none)", label);
                } else {
                    println!("  {}:", label);
                    for item in list.iter() {
                        println!("    - {}", item);
                    }
                }
            }

            // Show custom providers
            if settings.custom_providers.is_empty() {
                println!("  Custom providers: (none)");
            } else {
                println!("  Custom providers:");
                for cp in &settings.custom_providers {
                    println!("    - {} ({} @ {})", cp.name, cp.api, cp.base_url);
                }
            }
        }

        ConfigCommands::List { resource_type } => {
            let settings = Settings::load()?;

            let resource_types: Vec<(&str, &Vec<String>, ResourceKind)> = vec![
                ("extensions", &settings.extensions, ResourceKind::Extension),
                ("skills", &settings.skills, ResourceKind::Skill),
                ("prompts", &settings.prompts, ResourceKind::Prompt),
                ("themes", &settings.themes, ResourceKind::Theme),
            ];

            let filtered: Vec<_> = if let Some(rt) = resource_type {
                let kind = parse_resource_type(rt).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Unknown resource type '{}'. Valid: extension, skill, prompt, theme",
                        rt
                    )
                })?;
                resource_types
                    .into_iter()
                    .filter(|(_, _, k)| *k == kind)
                    .collect()
            } else {
                resource_types
            };

            for (label, list, _) in &filtered {
                if list.is_empty() {
                    println!("No {} configured.", label);
                } else {
                    println!("{}:", label);
                    for (i, item) in list.iter().enumerate() {
                        println!("  {}. {}", i + 1, item);
                    }
                }
                println!();
            }

            // Also show resources from installed packages
            let mgr = PackageManager::new()?;
            let packages = mgr.list();
            if !packages.is_empty() {
                println!("Package resources:");
                for pkg in packages {
                    if let Ok(resources) = mgr.discover_resources(&pkg.name) {
                        for r in &resources {
                            // Filter by requested type if specified
                            if let Some(rt) = resource_type {
                                if let Some(kind) = parse_resource_type(rt) {
                                    if r.kind != kind {
                                        continue;
                                    }
                                }
                            }
                            println!("  {} [{}] {}", pkg.name, r.kind, r.relative_path);
                        }
                    }
                }
            }
        }

        ConfigCommands::Enable {
            resource_type,
            name,
        } => {
            let kind = parse_resource_type(resource_type).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown resource type '{}'. Valid: extension, skill, prompt, theme",
                    resource_type
                )
            })?;

            let mut settings = Settings::load()?;

            let list = match kind {
                ResourceKind::Extension => &mut settings.extensions,
                ResourceKind::Skill => &mut settings.skills,
                ResourceKind::Prompt => &mut settings.prompts,
                ResourceKind::Theme => &mut settings.themes,
            };

            if list.iter().any(|item| item == name) {
                println!("{} '{}' is already enabled.", kind, name);
                return Ok(());
            }

            list.push(name.clone());
            settings.save()?;
            println!("Enabled {} '{}'", kind, name);
        }

        ConfigCommands::Disable {
            resource_type,
            name,
        } => {
            let kind = parse_resource_type(resource_type).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown resource type '{}'. Valid: extension, skill, prompt, theme",
                    resource_type
                )
            })?;

            let mut settings = Settings::load()?;

            let list = match kind {
                ResourceKind::Extension => &mut settings.extensions,
                ResourceKind::Skill => &mut settings.skills,
                ResourceKind::Prompt => &mut settings.prompts,
                ResourceKind::Theme => &mut settings.themes,
            };

            let original_len = list.len();
            list.retain(|item| item != name);

            if list.len() == original_len {
                println!("{} '{}' was not enabled.", kind, name);
                return Ok(());
            }

            settings.save()?;
            println!("Disabled {} '{}'", kind, name);
        }

        ConfigCommands::Set { key, value } => {
            let mut settings = Settings::load()?;

            match key.as_str() {
                "theme" => {
                    settings.theme = value.clone();
                }
                "default_model" | "model" => {
                    settings.default_model = Some(value.clone());
                }
                "default_provider" | "provider" => {
                    settings.default_provider = Some(value.clone());
                }
                "thinking_level" | "thinking" => {
                    let level = oxi::settings::parse_thinking_level(value)
                        .ok_or_else(|| anyhow::anyhow!(
                            "Invalid thinking level: '{}'. Valid: none, minimal, standard, thorough",
                            value
                        ))?;
                    settings.thinking_level = level;
                }
                "extensions_enabled" => {
                    settings.extensions_enabled = parse_config_bool(value)?;
                }
                "stream_responses" | "stream" => {
                    settings.stream_responses = parse_config_bool(value)?;
                }
                "auto_compaction" => {
                    settings.auto_compaction = parse_config_bool(value)?;
                }
                "tool_timeout" | "tool_timeout_seconds" => {
                    settings.tool_timeout_seconds = value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid timeout: '{}'", value))?;
                }
                "max_tokens" => {
                    settings.max_tokens = Some(
                        value
                            .parse()
                            .map_err(|_| anyhow::anyhow!("Invalid max_tokens: '{}'", value))?,
                    );
                }
                "temperature" => {
                    settings.default_temperature = Some(
                        value
                            .parse()
                            .map_err(|_| anyhow::anyhow!("Invalid temperature: '{}'", value))?,
                    );
                }
                "session_history_size" => {
                    settings.session_history_size = value.parse().map_err(|_| {
                        anyhow::anyhow!("Invalid session_history_size: '{}'", value)
                    })?;
                }
                _ => {
                    anyhow::bail!(
                        "Unknown setting: '{}'. Valid keys: theme, default_model, default_provider, \
                         thinking_level, extensions_enabled, stream_responses, auto_compaction, \
                         tool_timeout, max_tokens, temperature, session_history_size",
                        key
                    );
                }
            }

            settings.save()?;
            println!("Set {} = {}", key, value);
        }

        ConfigCommands::Get { key } => {
            let settings = Settings::load()?;

            let value = match key.as_str() {
                "theme" => settings.theme.clone(),
                "default_model" | "model" => settings
                    .default_model
                    .clone()
                    .unwrap_or_else(|| "(not set)".to_string()),
                "default_provider" | "provider" => settings
                    .default_provider
                    .clone()
                    .unwrap_or_else(|| "(not set)".to_string()),
                "thinking_level" | "thinking" => {
                    format!("{:?}", settings.thinking_level).to_lowercase()
                }
                "extensions_enabled" => settings.extensions_enabled.to_string(),
                "stream_responses" | "stream" => settings.stream_responses.to_string(),
                "auto_compaction" => settings.auto_compaction.to_string(),
                "tool_timeout" | "tool_timeout_seconds" => {
                    format!("{}s", settings.tool_timeout_seconds)
                }
                "max_tokens" => settings
                    .max_tokens
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "(not set)".to_string()),
                "temperature" => settings
                    .effective_temperature()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "(not set)".to_string()),
                "session_history_size" => settings.session_history_size.to_string(),
                "extensions" => format!("{:?}", settings.extensions),
                "skills" => format!("{:?}", settings.skills),
                "prompts" => format!("{:?}", settings.prompts),
                "themes" => format!("{:?}", settings.themes),
                "custom_providers" => {
                    let items: Vec<String> = settings.custom_providers.iter()
                        .map(|cp| format!("{} ({} @ {})", cp.name, cp.api, cp.base_url))
                        .collect();
                    if items.is_empty() {
                        "(none)".to_string()
                    } else {
                        items.join(", ")
                    }
                }
                _ => {
                    anyhow::bail!(
                        "Unknown setting: '{}'. Valid keys: theme, default_model, default_provider, \
                         thinking_level, extensions_enabled, stream_responses, auto_compaction, \
                         tool_timeout, max_tokens, temperature, session_history_size, \
                         extensions, skills, prompts, themes, custom_providers",
                        key
                    );
                }
            };

            println!("{} = {}", key, value);
        }

        ConfigCommands::AddProvider { name, base_url, api_key_env, api } => {
            use oxi::settings::CustomProvider;

            let mut settings = Settings::load()?;

            // Check if provider already exists
            if settings.custom_providers.iter().any(|cp| cp.name == *name) {
                // Update existing
                if let Some(cp) = settings.custom_providers.iter_mut().find(|cp| cp.name == *name) {
                    cp.base_url = base_url.clone();
                    cp.api_key_env = api_key_env.clone();
                    cp.api = api.clone();
                }
                settings.save()?;
                println!("Updated custom provider '{}' -> {} ({})", name, base_url, api);
            } else {
                settings.custom_providers.push(CustomProvider {
                    name: name.clone(),
                    base_url: base_url.clone(),
                    api_key_env: api_key_env.clone(),
                    api: api.clone(),
                });
                settings.save()?;
                println!("Added custom provider '{}' -> {} ({})", name, base_url, api);
            }
        }

        ConfigCommands::RemoveProvider { name } => {
            let mut settings = Settings::load()?;
            let original_len = settings.custom_providers.len();
            settings.custom_providers.retain(|cp| cp.name != *name);

            if settings.custom_providers.len() == original_len {
                println!("Custom provider '{}' not found.", name);
                return Ok(());
            }

            settings.save()?;
            println!("Removed custom provider '{}'", name);
        }
    }

    Ok(())
}

/// Handle `oxi setup [--reset]`
fn handle_setup_command(reset: bool) -> Result<()> {
    if reset {
        let settings_path = oxi::settings::Settings::settings_path()?;
        if settings_path.exists() {
            std::fs::remove_file(&settings_path)?;
            println!("Removed settings: {}", settings_path.display());
        }
        // Reset auth file
        let auth_path = dirs::config_dir()
            .unwrap_or_default()
            .join("oxi")
            .join("auth.json");
        if auth_path.exists() {
            std::fs::remove_file(&auth_path)?;
            println!("Removed auth: {}", auth_path.display());
        }
        println!("Settings reset to defaults.");
    }
    oxi::setup_wizard::run()
}

/// Handle `oxi models [--provider <name>]`
fn handle_models_command(provider: &Option<String>) -> Result<()> {
    use oxi_ai::{get_all_models, get_provider_models, model_count};

    // If a custom provider is specified, also try to fetch models dynamically
    if let Some(ref provider_name) = *provider {
        let settings = Settings::load().unwrap_or_default();
        if let Some(cp) = settings.custom_providers.iter().find(|cp| cp.name == *provider_name) {
            let api_key = std::env::var(&cp.api_key_env).ok();
            if let Some(ref key) = api_key {
                match oxi_ai::fetch_models_blocking(&cp.base_url, key) {
                    Ok(model_ids) => {
                        let api_type = match cp.api.to_lowercase().as_str() {
                            "openai-responses" | "responses" => oxi_ai::Api::OpenAiResponses,
                            _ => oxi_ai::Api::OpenAiCompletions,
                        };
                        for model_id in &model_ids {
                            let model = oxi_ai::Model {
                                id: model_id.clone(),
                                name: model_id.clone(),
                                api: api_type,
                                provider: cp.name.clone(),
                                base_url: cp.base_url.clone(),
                                reasoning: false,
                                input: vec![oxi_ai::InputModality::Text],
                                cost: oxi_ai::Cost::default(),
                                context_window: 128_000,
                                max_tokens: 8_192,
                                headers: Default::default(),
                                compat: None,
                            };
                            oxi_ai::register_model(model);
                        }
                        if model_ids.is_empty() {
                            println!("No models found for provider '{}'.", provider_name);
                        } else {
                            println!("Models from '{}' ({} fetched):", provider_name, model_ids.len());
                            for id in &model_ids {
                                println!("  {}", id);
                            }
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("[oxi] 경고: {} 모델 조회 실패: {}", provider_name, e);
                    }
                }
            } else {
                eprintln!("[oxi] API key not set for provider '{}' (expected: {})", provider_name, cp.api_key_env);
            }
        }

        // Fallback: show static models for this provider
        let models = get_provider_models(provider_name);
        if models.is_empty() {
            println!("No models found for provider '{}' (static or dynamic).", provider_name);
        } else {
            println!("Models for provider '{}' ({}):", provider_name, models.len());
            for m in models {
                println!("  {} ({})", m.id, m.name);
            }
        }
        return Ok(());
    }

    // No provider filter: show everything
    let all: Vec<_> = get_all_models().collect();
    let static_count = model_count();
    println!("Available models ({} static, {} total):", static_count, all.len());
    for entry in &all {
        println!("  {}/{} — {}", entry.provider, entry.id, entry.name);
    }
    Ok(())
}

async fn list_sessions(manager: &SessionManager) -> Result<()> {
    let sessions = manager.list_sessions().await?;

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Sessions:");
    println!("{:<36} {:<20} {}", "ID", "BRANCH", "UPDATED");
    println!("{:-<36} {:-<20} {:-<20}", "", "", "");

    for meta in sessions {
        let branch_str = if let Some(ref pid) = meta.parent_id {
            format!("forked from {}", &pid.to_string()[..8])
        } else {
            "root".to_string()
        };
        let updated = chrono::DateTime::from_timestamp_millis(meta.updated_at)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("{:<36} {:<20} {}", meta.id, branch_str, updated);
    }

    Ok(())
}

async fn show_tree(manager: &SessionManager, session_id: &str) -> Result<()> {
    let id = if session_id.is_empty() {
        // Get most recent session
        let sessions = manager.list_sessions().await?;
        match sessions.first() {
            Some(s) => s.id,
            None => {
                println!("No sessions found.");
                return Ok(());
            }
        }
    } else {
        Uuid::parse_str(session_id)?
    };

    let tree = manager.get_tree(id)?;
    let branch_info = manager.get_branch_info(id).await?;

    if let Some(info) = branch_info {
        if let Some(ref pid) = info.parent_session_id {
            println!("Session: {} (branched from {})", id, pid);
        } else {
            println!("Session: {} (root)", id);
        }
    } else {
        println!("Session: {} (root)", id);
    }
    println!();

    // Show tree structure
    for node in &tree {
        let role_marker = match &node.entry.message {
            AgentMessage::User { .. } => "👤",
            AgentMessage::Assistant { .. } => "🤖",
            AgentMessage::System { .. } => "⚙️",
            _ => "•",
        };

        let content_preview = truncate(&node.entry.content(), 60);
        let prefix = if node.entry.parent_id.is_some() {
            "├─"
        } else {
            "└─"
        };

        println!(
            "  {}{} [{:.8}] {}",
            prefix, role_marker, node.entry.id, content_preview
        );
    }

    Ok(())
}

async fn fork_session(
    manager: &SessionManager,
    parent_id_str: &str,
    entry_id_str: &str,
) -> Result<()> {
    let sessions = manager.list_sessions().await?;
    let info = sessions
        .iter()
        .find(|s| s.id.to_string().starts_with(parent_id_str))
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", parent_id_str))?;
    let entry_id = Uuid::parse_str(entry_id_str)
        .map_err(|_| anyhow::anyhow!("Invalid entry ID: {}", entry_id_str))?;
    let (new_session_id, _) = manager.branch_from(info.id, entry_id).await?;
    println!("Created forked session: {}", new_session_id);
    println!("File: {}", manager.session_path(&new_session_id).display());
    Ok(())
}

async fn delete_session(manager: &SessionManager, session_id: &str) -> Result<()> {
    let sessions = manager.list_sessions().await?;
    let info = sessions
        .iter()
        .find(|s| s.id.to_string().starts_with(session_id))
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;
    let path = manager.session_path(&info.id);
    manager.delete(info.id).await?;
    println!("Deleted session: {}", path.display());
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Run a single prompt and print the result
async fn run_single_prompt(app: oxi::App, prompt: &str) -> Result<()> {
    let mut session = app.run_interactive().await?;
    session.send_message(prompt.to_string()).await?;

    // Print response
    for msg in session.messages() {
        if msg.role == "assistant" {
            println!("{}", msg.content);
        }
    }

    Ok(())
}


