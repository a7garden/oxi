//! oxi CLI - CLI coding harness for oxi
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::Parser;
use oxi::cli::{CliArgs, Commands, ConfigCommands, IssueCommands, PkgCommands};
use oxi::storage::packages::{PackageManager, ResourceKind};
use oxi::store::session::{AgentMessage, SessionManager};
use oxi::store::settings::Settings;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("OXI_PORT_CHECK").is_ok() {
        oxi::run_port_check().await?;
        std::process::exit(0);
    }
    oxi::bootstrap::init_logging();
    let args = CliArgs::parse();
    if let Some(command) = &args.command {
        return handle_subcommand(command).await;
    }
    let exit_code = oxi::bootstrap::run_with_args(args).await?;
    std::process::exit(exit_code);
}

/// Handle session-related commands
async fn handle_subcommand(command: &Commands) -> Result<()> {
    match command {
        Commands::Sessions => {
            let cwd = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .to_string_lossy()
                .to_string();
            let sessions = oxi::store::session::SessionManager::list(&cwd, None).await?;
            if sessions.is_empty() {
                println!("No sessions found.");
            } else {
                println!("Sessions:");
                println!(
                    "{:<20} {:>6} {:<30} {:>12}",
                    "NAME", "MSG", "PREVIEW", "TIME"
                );
                println!("{:-<20} {:-<6} {:-<30} {:-<12}", "", "", "", "");
                for session in &sessions {
                    let name = session.name.as_deref().unwrap_or("-");
                    let preview = if session.first_message.len() > 28 {
                        format!("{}...", &session.first_message[..28])
                    } else {
                        session.first_message.clone()
                    };
                    let time = chrono::DateTime::<chrono::Local>::from(session.modified)
                        .format("%m-%d %H:%M")
                        .to_string();
                    println!(
                        "{:<20} {:>6} {:<30} {:>12}",
                        &name[..name.len().min(20)],
                        session.message_count,
                        preview,
                        time
                    );
                }
            }
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
        Commands::Issue { action } => {
            handle_issue_command(action).await?;
        }
        Commands::Config { action } => {
            handle_config_command(action)?;
        }
        Commands::Ext { action } => {
            handle_ext_command(action).await?;
        }
        Commands::Models { provider } => {
            handle_models_command(provider)?;
        }
        Commands::Setup { reset } => {
            handle_setup_command(*reset)?;
        }
        Commands::Reset {
            yes,
            include_project,
        } => {
            handle_reset_command(*yes, *include_project)?;
        }
        Commands::Export { session_id, output } => {
            handle_export_command(session_id.as_deref(), output.as_deref())?;
        }
        Commands::Import { path } => {
            handle_import_command(path)?;
        }
        Commands::Share { session_id } => {
            handle_share_command(session_id.as_deref()).await?;
        }
    }

    Ok(())
}

/// Handle `oxi issue …` subcommands. Opens the local issue store rooted at
/// the project and dispatches to the requested action.
async fn handle_issue_command(action: &IssueCommands) -> Result<()> {
    use oxi::store::issues::{IssueFilter, Priority, Status};
    use oxi::tools::format_issue_full;

    let cwd = std::env::current_dir()?;
    let store = oxi::store::issues::FileIssueStore::open_from_cwd(&cwd)?;

    match action {
        IssueCommands::List { all, label, text } => {
            let filter = IssueFilter {
                status: if *all { None } else { Some(Status::Open) },
                priority: None,
                label: label.clone(),
                assigned_to_session: None,
                text: text.clone(),
            };
            let issues = store.list(&filter)?;
            if issues.is_empty() {
                println!("(no issues)");
            } else {
                for i in &issues {
                    println!("{}", oxi::tools::format_issue_line(i));
                }
            }
        }
        IssueCommands::Show { id } => {
            let (issue, hash) = store.read(*id)?;
            println!("{}", format_issue_full(&issue, &hash));
        }
        IssueCommands::New {
            title,
            body,
            priority,
            labels,
        } => {
            let body = body.clone().unwrap_or_default();
            let prio = match priority.as_deref() {
                Some("low") => Priority::Low,
                Some("medium") | None => Priority::Medium,
                Some("high") => Priority::High,
                Some("critical") => Priority::Critical,
                Some(other) => anyhow::bail!("invalid priority: {other}"),
            };
            let labels: Vec<String> = labels
                .as_deref()
                .map(|s| s.split(',').map(|l| l.trim().to_string()).collect())
                .unwrap_or_default();
            let issue = store.create(title.clone(), body, prio, labels, None)?;
            println!("created issue #{}: {}", issue.meta.id, issue.meta.title);
        }
        IssueCommands::Close { id, hash } => {
            // Unique session id per CLI invocation (pid + nanos) so two
            // concurrent `oxi issue close` calls don't collide.
            let session = format!(
                "cli-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            // Hold the liveness lock for the duration of this command so
            // that any other process trying to read the issue sees us as a
            // live owner while we operate.
            let _guard = oxi::store::issues::liveness::acquire(
                &store.issues_dir(),
                &session,
            )
            .ok();

            // 1. Read current state to inspect the current assignment.
            let (issue, current_hash) = store.read(*id)?;
            if let Some(ref a) = issue.meta.assigned_to {
                if a.session != session
                    && oxi::store::issues::liveness::is_session_alive(
                        &store.issues_dir(),
                        &a.session,
                    )
                {
                    anyhow::bail!(
                        "issue #{id} is currently being worked on by session {} \
                         (since {}); cannot close from CLI",
                        a.session,
                        a.acquired_at,
                    );
                }
            }
            // 2. Claim (or re-claim if previous owner is dead), then close.
            //    Use the user-supplied hash if any; otherwise the current one.
            let effective_hash = hash.clone().unwrap_or(current_hash);
            store
                .start(*id, &session, Some(effective_hash.clone()))
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let closed = store
                .close(*id, &session, Some(effective_hash))
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!("closed issue #{}: {}", closed.meta.id, closed.meta.title);
        }
    }
    Ok(())
}

fn handle_pkg_command(action: &PkgCommands) -> Result<()> {
    let mut mgr = PackageManager::new()?;

    match action {
        PkgCommands::Install { source } => {
            if source.starts_with("npm:") {
                let name = source
                    .strip_prefix("npm:")
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
                    "{:<30} {:<10} {:<15} INSTALL DIR",
                    "NAME", "VERSION", "RESOURCES"
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
                                eprintln!(
                                    "{}",
                                    oxi::print_mode::format_error(&format!(
                                        "Failed to update {}: {}",
                                        pkg_name, e
                                    ))
                                );
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

async fn handle_ext_command(action: &oxi::cli::ExtCommands) -> Result<()> {
    use oxi::cli::ExtCommands;
    use oxi::extensions::ext_cli;

    match action {
        ExtCommands::Install { source, prerelease } => {
            let result = ext_cli::install_extension(source, *prerelease).await?;
            println!(
                "Installed {} v{} from {}",
                result.name, result.version, result.source
            );
        }
        ExtCommands::List => {
            let entries = ext_cli::list_extensions()?;
            if entries.is_empty() {
                println!("No extensions installed.");
                println!("Install with: oxi ext install owner/repo");
            } else {
                println!("Installed extensions:\n");
                for (source, entry) in &entries {
                    let name = entry
                        .wasm_file
                        .strip_suffix(".wasm")
                        .unwrap_or(&entry.wasm_file);
                    println!(
                        "  {} v{} — {} ({})",
                        source,
                        entry.version,
                        name,
                        entry.installed_at.split('T').next().unwrap_or("?")
                    );
                }
                println!("\n{} extension(s)", entries.len());
            }
        }
        ExtCommands::Remove { source } => {
            ext_cli::remove_extension(source)?;
            println!("Removed extension: {}", source);
        }
        ExtCommands::Update { source } => {
            let results = ext_cli::update_extension(source.as_deref()).await?;
            if results.is_empty() {
                println!("Nothing to update.");
            } else {
                for r in &results {
                    println!("Updated {} to {}", r.source, r.version);
                }
            }
        }
        ExtCommands::Info { source } => {
            ext_cli::info_extension(source).await?;
        }
    }
    Ok(())
}

fn handle_config_command(action: &ConfigCommands) -> Result<()> {
    match action {
        ConfigCommands::Show => config_show(),
        ConfigCommands::List { resource_type } => config_list(resource_type.as_ref()),
        ConfigCommands::Enable {
            resource_type,
            name,
        } => config_toggle_resource(resource_type, name, true),
        ConfigCommands::Disable {
            resource_type,
            name,
        } => config_toggle_resource(resource_type, name, false),
        ConfigCommands::Set { key, value } => config_set(key, value),
        ConfigCommands::Get { key } => config_get(key),
        ConfigCommands::AddProvider {
            name,
            base_url,
            api_key_env,
            api,
        } => config_add_provider(name, base_url, api_key_env, api),
        ConfigCommands::RemoveProvider { name } => config_remove_provider(name),
        ConfigCommands::Reset { all } => handle_config_reset(*all),
    }
}

/// Handle `oxi config show`
fn config_show() -> Result<()> {
    let settings = Settings::load()?;
    println!("oxi configuration:");
    println!("  Settings file: {}", Settings::settings_path()?.display());
    println!();
    println!(
        "  Model: {}",
        settings
            .effective_model(None)
            .unwrap_or_else(|| "(not set)".to_string())
    );
    println!(
        "  Provider: {}",
        settings
            .effective_provider(None)
            .unwrap_or_else(|| "(not set)".to_string())
    );
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

    if settings.custom_providers.is_empty() {
        println!("  Custom providers: (none)");
    } else {
        println!("  Custom providers:");
        for cp in &settings.custom_providers {
            println!("    - {} ({} @ {})", cp.name, cp.api, cp.base_url);
        }
    }
    Ok(())
}

/// Handle `oxi config list [resource_type]`
fn config_list(resource_type: Option<&String>) -> Result<()> {
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
                    if let Some(rt) = resource_type
                        && let Some(kind) = parse_resource_type(rt)
                        && r.kind != kind
                    {
                        continue;
                    }
                    println!("  {} [{}] {}", pkg.name, r.kind, r.relative_path);
                }
            }
        }
    }
    Ok(())
}

/// Handle `oxi config enable/disable <type> <name>`
fn config_toggle_resource(resource_type: &str, name: &str, enable: bool) -> Result<()> {
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

    if enable {
        if list.iter().any(|item| item == name) {
            println!("{} '{}' is already enabled.", kind, name);
            return Ok(());
        }
        list.push(name.to_string());
        settings.save()?;
        println!("Enabled {} '{}'", kind, name);
    } else {
        let original_len = list.len();
        list.retain(|item| item != name);
        if list.len() == original_len {
            println!("{} '{}' was not enabled.", kind, name);
            return Ok(());
        }
        settings.save()?;
        println!("Disabled {} '{}'", kind, name);
    }
    Ok(())
}

/// Handle `oxi config set <key> <value>`
fn config_set(key: &str, value: &str) -> Result<()> {
    let mut settings = Settings::load()?;

    match key {
        "theme" => {
            settings.theme = value.to_string();
        }
        "model" => {
            settings.last_used_model = Some(value.to_string());
        }
        "provider" => {
            settings.last_used_provider = Some(value.to_string());
        }
        "thinking_level" | "thinking" => {
            let level = oxi::store::settings::parse_thinking_level(value).ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid thinking level: '{}'. Valid: off, minimal, low, medium, high, xhigh",
                    value
                )
            })?;
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
            settings.session_history_size = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid session_history_size: '{}'", value))?;
        }
        _ => {
            anyhow::bail!(
                "Unknown setting: '{}'. Valid keys: theme, model, provider,\
                  thinking_level, extensions_enabled, stream_responses, auto_compaction,\
                  tool_timeout, max_tokens, temperature, session_history_size",
                key
            );
        }
    }

    settings.save()?;
    println!("Set {} = {}", key, value);
    Ok(())
}

/// Handle `oxi config get <key>`
fn config_get(key: &str) -> Result<()> {
    let settings = Settings::load()?;

    let value = match key {
        "theme" => settings.theme.clone(),
        "model" => settings
            .last_used_model
            .clone()
            .unwrap_or_else(|| "(not set)".to_string()),
        "provider" => settings
            .last_used_provider
            .clone()
            .unwrap_or_else(|| "(not set)".to_string()),
        "thinking_level" | "thinking" => format!("{:?}", settings.thinking_level).to_lowercase(),
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
            let items: Vec<String> = settings
                .custom_providers
                .iter()
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
                "Unknown setting: '{}'. Valid keys: theme, model, provider,\
                  thinking_level, extensions_enabled, stream_responses, auto_compaction,\
                  tool_timeout, max_tokens, temperature, session_history_size,\
                  extensions, skills, prompts, themes, custom_providers",
                key
            );
        }
    };

    println!("{} = {}", key, value);
    Ok(())
}

/// Handle `oxi config add-provider`
fn config_add_provider(name: &str, base_url: &str, api_key_env: &str, api: &str) -> Result<()> {
    use oxi::store::settings::CustomProvider;

    let mut settings = Settings::load()?;

    // Update existing or add new
    if let Some(cp) = settings
        .custom_providers
        .iter_mut()
        .find(|cp| cp.name == name)
    {
        cp.base_url = base_url.to_string();
        cp.api_key_env = api_key_env.to_string();
        cp.api = api.to_string();
        settings.save()?;
        println!(
            "Updated custom provider '{}' -> {} ({})",
            name, base_url, api
        );
    } else {
        settings.custom_providers.push(CustomProvider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: api_key_env.to_string(),
            api: api.to_string(),
        });
        settings.save()?;
        println!("Added custom provider '{}' -> {} ({})", name, base_url, api);
    }
    Ok(())
}

/// Handle `oxi config remove-provider`
fn config_remove_provider(name: &str) -> Result<()> {
    let mut settings = Settings::load()?;
    let original_len = settings.custom_providers.len();
    settings.custom_providers.retain(|cp| cp.name != name);

    if settings.custom_providers.len() == original_len {
        println!("Custom provider '{}' not found.", name);
        return Ok(());
    }

    settings.save()?;
    println!("Removed custom provider '{}'", name);
    Ok(())
}

/// Handle `oxi setup [--reset]`
fn handle_setup_command(reset: bool) -> Result<()> {
    if reset {
        handle_config_reset(true)?;
    }
    oxi::setup_wizard::run()
}

/// Target descriptor for the reset command.
struct ResetTarget {
    label: String,
    path: std::path::PathBuf,
    description: String,
}

/// Handle `oxi reset [--yes] [--include-project]`
///
/// Factory-reset: deletes ALL oxi data.
/// Optionally also deletes the project-local `.oxi/` directory.
fn handle_reset_command(yes: bool, include_project: bool) -> Result<()> {
    use std::io::{self, Write};

    // ── Collect targets ──────────────────────────────────────────
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;

    let oxi_dir = home.join(".oxi");
    let config_oxi_dir = dirs::config_dir()
        .unwrap_or_else(|| home.join(".config"))
        .join("oxi");
    let cache_oxi_dir = dirs::cache_dir()
        .unwrap_or_else(|| home.join(".cache"))
        .join("oxi");

    let mut targets: Vec<ResetTarget> = vec![];

    // ~/.oxi/ — split into sub-items for clarity
    if oxi_dir.exists() {
        let sub_items = [
            ("settings.toml", "global settings"),
            ("settings.json", "global settings (JSON)"),
            ("auth.json", "credentials (API keys, OAuth tokens)"),
            ("sessions", "session history"),
            ("skills", "skills"),
            ("extensions", "extensions"),
            ("packages", "packages"),
        ];
        let mut has_sub = false;
        for (name, desc) in &sub_items {
            let p = oxi_dir.join(name);
            if p.exists() {
                has_sub = true;
                targets.push(ResetTarget {
                    label: format!("~/.oxi/{}", name),
                    path: p,
                    description: desc.to_string(),
                });
            }
        }
        // If no known sub-items found, target the whole directory
        if !has_sub {
            targets.push(ResetTarget {
                label: "~/.oxi".to_string(),
                path: oxi_dir.clone(),
                description: "oxi home (settings, sessions, skills, extensions, packages)"
                    .to_string(),
            });
        }
    }

    // ~/.config/oxi/ — MCP config, alternative auth location
    if config_oxi_dir.exists() {
        targets.push(ResetTarget {
            label: display_path(&config_oxi_dir),
            path: config_oxi_dir,
            description: "MCP config, credentials".to_string(),
        });
    }

    // ~/.cache/oxi/ — logs
    if cache_oxi_dir.exists() {
        targets.push(ResetTarget {
            label: display_path(&cache_oxi_dir),
            path: cache_oxi_dir,
            description: "logs, cache".to_string(),
        });
    }

    // Project-local .oxi/
    let project_oxi = std::env::current_dir().unwrap_or_default().join(".oxi");
    let mut project_target: Option<ResetTarget> = None;
    if include_project && project_oxi.exists() {
        project_target = Some(ResetTarget {
            label: display_path(&project_oxi),
            path: project_oxi.clone(),
            description: "project settings".to_string(),
        });
    }

    let total_count = targets.len() + usize::from(project_target.is_some());
    if total_count == 0 {
        println!("Nothing to reset — no oxi data found.");
        return Ok(());
    }

    // ── Calculate total size ─────────────────────────────────────
    let mut total_bytes: u64 = 0;
    for t in &targets {
        total_bytes += dir_size_bytes(&t.path);
    }
    if let Some(ref pt) = project_target {
        total_bytes += dir_size_bytes(&pt.path);
    }

    // ── Show what will be deleted ────────────────────────────────
    eprintln!();
    eprintln!("     ⚠ Warning: The following will be permanently deleted:");
    eprintln!();
    for (i, t) in targets.iter().enumerate() {
        eprintln!(
            "       {}. {} ({})",
            i + 1,
            display_path(&t.path),
            dir_size_human(&t.path)
        );
        eprintln!("          {}", t.description);
    }
    if let Some(ref pt) = project_target {
        eprintln!(
            "       {}. {} ({})",
            total_count,
            display_path(&pt.path),
            dir_size_human(&pt.path)
        );
        eprintln!("          {}", pt.description);
    }
    eprintln!();
    eprintln!(
        "     Total: {} item(s), {}",
        total_count,
        bytes_human(total_bytes)
    );
    eprintln!();
    eprintln!(
        "     This cannot be undone. All sessions, skills, extensions, and settings will be deleted."
    );
    eprintln!();

    if !yes {
        eprint!("     Type RESET to continue: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim() != "RESET" {
            eprintln!();
            eprintln!();
            eprintln!("     Cancelled.");
            return Ok(());
        }
    }

    // ── Delete ───────────────────────────────────────────────────
    eprintln!();
    let mut errors = Vec::new();

    for t in &targets {
        eprint!("     ● Deleting {}...", t.label);
        io::stdout().flush()?;
        match remove_path(&t.path) {
            Ok(()) => eprintln!(" done"),
            Err(e) => {
                eprintln!(" failed");
                eprintln!("       ✗ {}: {}", t.label, e);
                errors.push(format!("{}: {}", t.label, e));
            }
        }
    }
    if let Some(ref pt) = project_target {
        eprint!("     ● Deleting {}...", pt.label);
        io::stdout().flush()?;
        match remove_path(&pt.path) {
            Ok(()) => eprintln!(" done"),
            Err(e) => {
                eprintln!(" failed");
                eprintln!("       ✗ {}: {}", pt.label, e);
                errors.push(format!("{}: {}", pt.label, e));
            }
        }
    }

    eprintln!();
    if errors.is_empty() {
        eprintln!("     ✓ All oxi data has been reset.");
        eprintln!("     → Run 'oxi setup' to reconfigure.");
    } else {
        eprintln!("     ⚠ {} item(s) failed to delete:", errors.len());
        for err in &errors {
            eprintln!("       • {}", err);
        }
        eprintln!("     Some data may need manual cleanup.");
    }

    Ok(())
}

/// Remove a file or directory (including all contents).
fn remove_path(path: &std::path::Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Display path with ~/ abbreviation for home directory.
fn display_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        let path_str = path.to_string_lossy();
        if let Some(rest) = path_str.strip_prefix(home_str.as_ref()) {
            return format!("~{}", rest);
        }
    }
    path.display().to_string()
}

/// Calculate total bytes in a directory or file.
fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    if path.is_dir() {
        if let Ok(entries) = walkdir_recursive(path) {
            for entry in entries {
                if let Ok(meta) = std::fs::metadata(&entry)
                    && meta.is_file()
                {
                    total += meta.len();
                }
            }
        }
    } else if let Ok(meta) = std::fs::metadata(path) {
        total = meta.len();
    }
    total
}

/// Calculate a human-readable directory or file size.
fn dir_size_human(path: &std::path::Path) -> String {
    bytes_human(dir_size_bytes(path))
}

/// Format bytes as a human-readable string.
fn bytes_human(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Walk a directory recursively, collecting all file paths.
fn walkdir_recursive(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut result = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                result.extend(walkdir_recursive(&path)?);
            } else {
                result.push(path);
            }
        }
    }
    Ok(result)
}

/// Handle `oxi config reset [--all]`
///
/// Resets auth.json (credentials). With `--all`, also resets settings.
fn handle_config_reset(all: bool) -> Result<()> {
    // Always reset auth.json
    let auth_path = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("oxi")
        .join("auth.json");
    if auth_path.exists() {
        std::fs::remove_file(&auth_path)?;
        println!("Removed credentials: {}", auth_path.display());
    } else {
        println!("No credentials file found at {}", auth_path.display());
    }

    if all {
        // Also reset settings
        if let Ok(settings_path) = oxi::store::settings::Settings::settings_path()
            && settings_path.exists()
        {
            std::fs::remove_file(&settings_path)?;
            println!("Removed settings: {}", settings_path.display());
        }
        println!("Full reset complete. Run 'oxi setup' to reconfigure.");
    } else {
        println!("Credentials reset. Run 'oxi setup' to reconfigure API keys.");
    }

    Ok(())
}

/// Handle `oxi models [--provider <name>]`
fn handle_models_command(provider: &Option<String>) -> Result<()> {
    use oxi_sdk::{get_all_models, get_provider_models, model_count};

    // If a custom provider is specified, also try to fetch models dynamically
    if let Some(ref provider_name) = *provider {
        let settings = Settings::load().unwrap_or_default();
        if let Some(cp) = settings
            .custom_providers
            .iter()
            .find(|cp| cp.name == *provider_name)
        {
            let auth = oxi::store::auth_storage::shared_auth_storage();
            let api_key = auth.get_api_key(&cp.name);
            if let Some(ref key) = api_key {
                match oxi_sdk::fetch_models_blocking(&cp.base_url, key) {
                    Ok(model_ids) => {
                        let api_type = match cp.api.to_lowercase().as_str() {
                            "openai-responses" | "responses" => oxi_sdk::Api::OpenAiResponses,
                            _ => oxi_sdk::Api::OpenAiCompletions,
                        };
                        for model_id in &model_ids {
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
                        if model_ids.is_empty() {
                            println!("No models found for provider '{}'.", provider_name);
                        } else {
                            println!(
                                "Models from '{}' ({} fetched):",
                                provider_name,
                                model_ids.len()
                            );
                            for id in &model_ids {
                                println!("  {}", id);
                            }
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!(
                            "[oxi] warning: failed to resolve models for {}: {}",
                            provider_name, e
                        );
                    }
                }
            } else {
                eprintln!(
                    "[oxi] API key not set for provider '{}' (expected: {})",
                    provider_name, cp.api_key_env
                );
            }
        }

        // Fallback: show static models for this provider
        let models = get_provider_models(provider_name);
        if models.is_empty() {
            println!(
                "No models found for provider '{}' (static or dynamic).",
                provider_name
            );
        } else {
            println!(
                "Models for provider '{}' ({}):",
                provider_name,
                models.len()
            );
            for m in models {
                println!("  {} ({})", m.id, m.name);
            }
        }
        return Ok(());
    }

    // No provider filter: show everything
    let all: Vec<_> = get_all_models().collect();
    let static_count = model_count();
    println!(
        "Available models ({} static, {} total):",
        static_count,
        all.len()
    );
    for entry in &all {
        println!("  {}/{} — {}", entry.provider, entry.id, entry.name);
    }
    Ok(())
}

#[allow(dead_code)]
async fn list_sessions(manager: &SessionManager) -> Result<()> {
    let sessions = manager.list_sessions().await?;

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Sessions:");
    println!("{:<36} {:<20} UPDATED", "ID", "BRANCH");
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

    if let Some(info) = branch_info
        && let Some(ref pid) = info.parent_session_id
    {
        println!("Session: {} (branched from {})", id, pid);
    } else {
        println!("Session: {} (root)", id);
    }
    println!();

    // Show tree structure
    for node in &tree {
        let role_marker = match &node.entry.message {
            AgentMessage::User { .. } => "U",
            AgentMessage::Assistant { .. } => "A",
            AgentMessage::System { .. } => "S",
            _ => "-",
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
        return s.to_string();
    }
    let boundary = s
        .char_indices()
        .take_while(|(i, _)| *i <= max_len.saturating_sub(3))
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}...", &s[..boundary])
}

// ── Export / Import / Share ──────────────────────────────────────────────────

/// Handle `oxi export [SESSION_ID] [--output PATH]`
fn handle_export_command(
    session_id: Option<&str>,
    output_path: Option<&std::path::Path>,
) -> Result<()> {
    use oxi::store::session::SessionManager;

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    // Resolve session file
    let session_path = if let Some(sid) = session_id {
        // Try as a direct path first
        let direct = std::path::Path::new(sid);
        if direct.exists() {
            direct.to_path_buf()
        } else {
            anyhow::bail!("Session not found: {}", sid);
        }
    } else {
        // Find the most recent session for this CWD
        // SessionManager::list is async but only does std::fs I/O internally.
        let sessions = std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(SessionManager::list(&cwd, None))
            })
            .join()
            .map_err(|e| anyhow::anyhow!("thread panicked: {:?}", e))?
        })?;
        let most_recent = sessions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No sessions found for this project"))?;
        // Reconstruct the path from session_dir + id
        let session_dir: std::path::PathBuf =
            oxi::store::session::get_default_session_dir(&cwd).into();
        session_dir.join(format!("{}.jsonl", most_recent.id))
    };

    if !session_path.exists() {
        anyhow::bail!("Session file not found: {}", session_path.display());
    }

    // Load session entries
    let sm = SessionManager::open(&session_path.to_string_lossy(), None, Some(&cwd));
    let branch = sm.get_branch(None);

    // Build metadata
    let meta = oxi::storage::export::ExportMeta {
        model: None,
        provider: None,
        exported_at: chrono::Utc::now().timestamp_millis(),
        total_user_tokens: None,
        total_assistant_tokens: None,
    };

    let entries: Vec<oxi::store::session::SessionEntry> = branch.into_iter().collect();
    let html = oxi::storage::export::export_to_html(
        &entries,
        &meta,
        &oxi::storage::export::HtmlExportOptions::default(),
    )?;

    // Determine output path
    let out = if let Some(p) = output_path {
        p.to_path_buf()
    } else {
        let sid_short = session_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session");
        let short = &sid_short[..8.min(sid_short.len())];
        std::path::PathBuf::from(format!("oxi-export-{}.html", short))
    };

    std::fs::write(&out, &html)?;
    println!(
        "Exported {} entries to {} ({} bytes)",
        entries.len(),
        out.display(),
        html.len()
    );
    Ok(())
}

/// Handle `oxi import <PATH>`
fn handle_import_command(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("File not found: {}", path.display());
    }

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    let resolved = oxi::store::session::resolve_session_path(&path.to_string_lossy(), &cwd)
        .map_err(|e| anyhow::anyhow!("Error resolving path: {}", e))?;

    if !std::path::Path::new(&resolved).exists() {
        anyhow::bail!("File not found: {}", resolved);
    }

    // Copy the session file into the sessions directory
    let sessions_dir: std::path::PathBuf =
        oxi::store::session::get_default_session_dir(&cwd).into();
    std::fs::create_dir_all(&sessions_dir)?;

    let filename = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("imported.jsonl"));
    let dest = sessions_dir.join(filename);

    // Avoid overwriting existing sessions
    if dest.exists() {
        let stem = dest
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported");
        let ext = dest.extension().and_then(|s| s.to_str()).unwrap_or("jsonl");
        let unique_name = format!(
            "{}-{}.{}",
            stem,
            chrono::Utc::now().format("%Y%m%d%H%M%S"),
            ext
        );
        let alt_dest = sessions_dir.join(&unique_name);
        std::fs::copy(path, &alt_dest)?;
        println!("Imported session to {}", alt_dest.display());
    } else {
        std::fs::copy(path, &dest)?;
        println!("Imported session to {}", dest.display());
    }
    Ok(())
}

/// Handle `oxi share [SESSION_ID]`
async fn handle_share_command(session_id: Option<&str>) -> Result<()> {
    // Check if gh CLI is available
    let gh_check = std::process::Command::new("gh")
        .args(["auth", "status"])
        .output()?;

    if !gh_check.status.success() {
        anyhow::bail!("GitHub CLI (gh) is not authenticated. Run: gh auth login");
    }

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    // Resolve session
    let session_path = if let Some(sid) = session_id {
        let direct = std::path::Path::new(sid);
        if direct.exists() {
            direct.to_path_buf()
        } else {
            anyhow::bail!("Session not found: {}", sid);
        }
    } else {
        let sessions = oxi::store::session::SessionManager::list(&cwd, None).await?;
        let most_recent = sessions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No sessions found for this project"))?;
        let session_dir: std::path::PathBuf =
            oxi::store::session::get_default_session_dir(&cwd).into();
        session_dir.join(format!("{}.jsonl", most_recent.id))
    };

    if !session_path.exists() {
        anyhow::bail!("Session file not found: {}", session_path.display());
    }

    let sm = oxi::store::session::SessionManager::open(
        &session_path.to_string_lossy(),
        None,
        Some(&cwd),
    );
    let branch = sm.get_branch(None);
    let entries: Vec<oxi::store::session::SessionEntry> = branch.into_iter().collect();

    let meta = oxi::storage::export::ExportMeta {
        model: None,
        provider: None,
        exported_at: chrono::Utc::now().timestamp_millis(),
        total_user_tokens: None,
        total_assistant_tokens: None,
    };

    let html = oxi::storage::export::export_to_html(
        &entries,
        &meta,
        &oxi::storage::export::HtmlExportOptions::default(),
    )?;

    let temp_path = std::env::temp_dir().join("oxi-share-export.html");
    std::fs::write(&temp_path, &html)?;

    // Create gist
    let output = tokio::process::Command::new("gh")
        .args(["gist", "create", &temp_path.to_string_lossy()])
        .output()
        .await?;

    let _ = std::fs::remove_file(&temp_path);

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("Gist created: {}", url);
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create gist: {}", stderr.trim());
    }
    Ok(())
}
