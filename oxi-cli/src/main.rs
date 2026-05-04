//! oxi CLI - CLI coding harness for oxi
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::{Parser, Subcommand};
use oxi::extensions::ExtensionRegistry;
use oxi::packages::{PackageManager, ResourceKind};
use oxi::session::{AgentMessage, SessionManager};
use oxi::settings::Settings;
use oxi::templates::TemplateManager;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// CLI arguments
#[derive(Parser, Debug)]
#[command(name = "oxi")]
#[command(about = "CLI coding harness for oxi")]
#[command(version = "0.1.0")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Provider to use (e.g., anthropic, openai, google, deepseek)
    #[arg(short, long)]
    provider: Option<String>,

    /// Model to use (e.g., claude-sonnet-4-20250514, gpt-4o)
    #[arg(short, long)]
    model: Option<String>,

    /// Initial prompt (non-interactive mode)
    #[arg(default_value = "")]
    prompt: Vec<String>,

    /// Interactive mode (default when no prompt is given)
    #[arg(short, long)]
    interactive: bool,

    /// Thinking level (none, minimal, standard, thorough)
    #[arg(long)]
    thinking: Option<String>,

    /// Load an extension from a shared library (.so / .dll / .dylib).
    /// Can be specified multiple times.
    #[arg(short = 'e', long = "extension", value_name = "PATH")]
    extensions: Vec<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List all sessions
    Sessions,
    /// Show session tree structure
    Tree {
        /// Session ID to show tree for (default: current/last session)
        #[arg(default_value = "")]
        session_id: String,
    },
    /// Fork a new session from a specific entry
    Fork {
        /// Parent session ID
        parent_id: String,
        /// Entry ID to branch from
        entry_id: String,
    },
    /// Delete a session
    Delete {
        /// Session ID to delete
        session_id: String,
    },
    /// Package management
    Pkg {
        #[command(subcommand)]
        action: PkgCommands,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },
}

#[derive(Subcommand, Debug)]
enum PkgCommands {
    /// Install a package from a local path or npm:@scope/name
    Install {
        /// Package source: a local directory path or npm:@scope/name
        source: String,
    },
    /// List installed packages
    List,
    /// Uninstall a package by name
    Uninstall {
        /// Package name to uninstall
        name: String,
    },
    /// Update a package to the latest version
    Update {
        /// Package name to update (updates all if omitted)
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommands {
    /// Show current configuration
    Show,
    /// List all enabled resources
    List {
        /// Resource type filter (extensions, skills, prompts, themes)
        resource_type: Option<String>,
    },
    /// Enable a resource (extension, skill, prompt, or theme)
    Enable {
        /// Resource type: extension, skill, prompt, or theme
        resource_type: String,
        /// Resource path or name
        name: String,
    },
    /// Disable a resource
    Disable {
        /// Resource type: extension, skill, prompt, or theme
        resource_type: String,
        /// Resource path or name
        name: String,
    },
    /// Set a configuration value
    Set {
        /// Setting key (e.g. theme, default_model, thinking_level)
        key: String,
        /// Setting value
        value: String,
    },
    /// Get a configuration value
    Get {
        /// Setting key
        key: String,
    },
}

/// Parse thinking level from string (delegates to settings module)
fn parse_thinking_level(s: &str) -> Option<oxi::settings::ThinkingLevel> {
    oxi::settings::parse_thinking_level(s)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Parse arguments
    let args = Args::parse();

    // Handle subcommands
    if let Some(command) = &args.command {
        return handle_subcommand(command).await;
    }

    // Load settings (global + project + env layers)
    let mut settings = Settings::load().unwrap_or_default();

    // Apply CLI overrides
    settings.merge_cli(args.model.clone(), args.provider.clone());

    // Apply thinking level if specified
    if let Some(ref level_str) = args.thinking {
        if let Some(level) = parse_thinking_level(level_str) {
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

    // Register extension tools with the agent
    let tools = app.agent_tools();
    for tool in ext_registry.all_tools() {
        tools.register_arc(tool);
    }

    if prompt.is_empty() || args.interactive {
        // TUI interactive mode
        oxi::tui_interactive::run_tui_interactive(app).await?;
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
    }

    Ok(())
}

fn handle_pkg_command(action: &PkgCommands) -> Result<()> {
    let mut mgr = PackageManager::new()?;

    match action {
        PkgCommands::Install { source } => {
            if source.starts_with("npm:") {
                let name = source.strip_prefix("npm:").unwrap();
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
                _ => {
                    anyhow::bail!(
                        "Unknown setting: '{}'. Valid keys: theme, default_model, default_provider, \
                         thinking_level, extensions_enabled, stream_responses, auto_compaction, \
                         tool_timeout, max_tokens, temperature, session_history_size, \
                         extensions, skills, prompts, themes",
                        key
                    );
                }
            };

            println!("{} = {}", key, value);
        }
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
        let branch_str = if meta.parent_id.is_some() {
            format!("forked from {}", &meta.parent_id.unwrap().to_string()[..8])
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

    let tree = manager.get_tree(id);
    let branch_info = manager.get_branch_info(id).await?;

    if let Some(info) = branch_info {
        println!(
            "Session: {} (branched from {})",
            id,
            info.parent_session_id.unwrap()
        );
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

#[allow(dead_code)]
enum CommandResult {
    Handled,
    NewSession(Uuid),
    #[allow(dead_code)]
    Quit,
}

/// Interactive mode (simple readline-based fallback)
#[allow(dead_code)]
async fn interactive_mode(app: oxi::App) -> Result<()> {
    use std::io::{self, Write};

    let mut session_manager = SessionManager::new().await?;
    let mut session = app.run_interactive().await?;
    let mut current_session_id: Option<String> = None;

    // Load prompt templates from ~/.oxi/templates/
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let templates_dir = home.join(".oxi").join("templates");
    let template_manager = TemplateManager::load_from_dir(&templates_dir)?;
    if !template_manager.is_empty() {
        tracing::info!(count = template_manager.len(), "loaded prompt templates");
    }

    println!("oxi CLI - type your message and press Enter. Ctrl+C or 'exit' to quit.");
    println!(
        "Commands: /sessions, /tree, /fork <entry_id>, /model, /skill, /template, /history, /help"
    );
    println!("---");

    loop {
        print!("❯ ");
        io::stdout().flush()?;

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            break;
        }

        let line = line.trim();
        if line.is_empty() || line == "exit" || line == "quit" {
            break;
        }

        // Handle commands
        if line.starts_with('/') {
            match handle_command(
                line,
                &mut session_manager,
                &mut session,
                &mut current_session_id,
                &template_manager,
                &app,
            )
            .await?
            {
                CommandResult::Handled => continue,
                CommandResult::NewSession(id) => current_session_id = Some(id.to_string()),
                CommandResult::Quit => break,
            }
        }

        // Send message and wait for response
        session.send_message(line.to_string()).await?;

        // Print assistant response
        for msg in session.messages() {
            if msg.role == "assistant" {
                println!("\n◉ {}\n", msg.content);
            }
        }
    }

    Ok(())
}

/// Handle `/template <name> [key=value ...]` — expand a template and send as a message.
#[allow(dead_code)]
async fn handle_template_expand(
    line: &str,
    templates: &TemplateManager,
    session: &mut oxi::InteractiveLoop<'_>,
) -> Result<CommandResult> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        println!("Usage: /template <name> [key=value ...]");
        return Ok(CommandResult::Handled);
    }

    let name = parts[1];

    // Parse key=value pairs from remaining args
    let mut vars: HashMap<&str, &str> = HashMap::new();
    for part in &parts[2..] {
        if let Some((key, value)) = part.split_once('=') {
            vars.insert(key, value);
        } else {
            println!("Invalid variable format: '{}' (expected key=value)", part);
            return Ok(CommandResult::Handled);
        }
    }

    match templates.render(name, vars) {
        Ok(rendered) => {
            println!("Expanded template '{}':", name);
            println!("---");
            println!("{}", rendered);
            println!("---");

            // Send the rendered template as a user message
            session.send_message(rendered).await?;

            // Print assistant response
            for msg in session.messages() {
                if msg.role == "assistant" {
                    println!("\n◉ {}\n", msg.content);
                }
            }
        }
        Err(e) => {
            println!("Template error: {}", e);
        }
    }

    Ok(CommandResult::Handled)
}

#[allow(dead_code)]
async fn handle_command(
    line: &str,
    manager: &mut SessionManager,
    session: &mut oxi::InteractiveLoop<'_>,
    current_session_id: &mut Option<String>,
    templates: &TemplateManager,
    app: &oxi::App,
) -> Result<CommandResult> {
    match line {
        "/sessions" | "/sessions list" => {
            let sessions = manager.list_sessions().await?;
            if sessions.is_empty() {
                println!("No sessions found.");
            } else {
                println!("Sessions:");
                for info in &sessions {
                    let modified = chrono::DateTime::from_timestamp_millis(info.updated_at)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "N/A".to_string());
                    println!(
                        "  {:.8}  {}  [root: {}]",
                        info.id,
                        modified,
                        if info.root_id.is_some() {
                            "branched"
                        } else {
                            "root"
                        }
                    );
                }
            }
            Ok(CommandResult::Handled)
        }
        "/tree" | "/tree view" => {
            if let Some(ref id) = current_session_id {
                let session_uuid = Uuid::parse_str(id)
                    .map_err(|_| anyhow::anyhow!("Invalid session ID: {}", id))?;
                let tree = manager.get_tree(session_uuid);
                if tree.is_empty() {
                    println!("No entries in session.");
                } else {
                    println!("Session tree:");
                    for node in &tree {
                        let role = match &node.entry.message {
                            AgentMessage::User { .. } => "👤",
                            AgentMessage::Assistant { .. } => "🤖",
                            AgentMessage::System { .. } => "⚙️",
                                    _ => "•",
        };
                        let preview = truncate(&node.entry.content(), 50);
                        println!("  {} [{:.8}]: {}", role, node.entry.id, preview);
                    }
                }
            } else {
                println!("No active session.");
            }
            Ok(CommandResult::Handled)
        }
        "/template" | "/templates" => {
            let names = templates.template_names();
            if names.is_empty() {
                println!("No templates found. Add .md files to ~/.oxi/templates/");
            } else {
                println!("Templates:");
                for name in &names {
                    let tmpl = templates.get(name).unwrap();
                    if tmpl.variables.is_empty() {
                        println!("  {} (no variables)", name);
                    } else {
                        println!("  {} {{{}}}", name, tmpl.variables.join(", "));
                    }
                }
                println!();
                println!("Usage: /template <name> [key=value ...]");
            }
            Ok(CommandResult::Handled)
        }
        "/skill" | "/skills" => {
            let skills = app.skills();
            let all_skills = skills.all();
            if all_skills.is_empty() {
                println!("No skills found. Add skill directories to ~/.oxi/skills/<name>/SKILL.md");
            } else {
                let active = app.active_skills();
                println!("Skills:");
                for skill in &all_skills {
                    let marker = if active.iter().any(|a| a == &skill.name) {
                        "✓"
                    } else {
                        " "
                    };
                    println!("  {} {} — {}", marker, skill.name, skill.description);
                }
                println!();
                println!("Usage: /skill <name>     — activate a skill");
                println!("       /skill off <name> — deactivate a skill");
            }
            Ok(CommandResult::Handled)
        }
        "/help" => {
            println!("Commands:");
            println!("  /sessions       - List all sessions");
            println!("  /tree            - Show current session tree");
            println!("  /fork <id>       - Fork from an entry");
            println!("  /model           - Show current model");
            println!("  /model <id>      - Switch model (e.g. openai/gpt-4o, anthropic/claude-sonnet-4-20250514)");
            println!("  /models          - List available models");
            println!("  /skill           - List available skills");
            println!("  /skill <name>    - Activate a skill");
            println!("  /skill off <name> - Deactivate a skill");
            println!("  /template        - List prompt templates");
            println!("  /template <name> [key=val ...] - Expand a template");
            println!("  /history         - Show conversation history");
            println!("  /help            - Show this help");
            Ok(CommandResult::Handled)
        }
        _ if line.starts_with("/skill off ") => {
            let name = line["/skill off ".len()..].trim();
            if name.is_empty() {
                println!("Usage: /skill off <name>");
            } else {
                app.deactivate_skill(name);
                println!("Deactivated skill: {}", name);
                let active = app.active_skills();
                if active.is_empty() {
                    println!("No active skills.");
                } else {
                    println!("Active skills: {}", active.join(", "));
                }
            }
            Ok(CommandResult::Handled)
        }
        _ if line.starts_with("/skill ") => {
            let name = line["/skill ".len()..].trim();
            if name.is_empty() {
                println!("Usage: /skill <name>");
            } else {
                match app.activate_skill(name) {
                    Ok(()) => {
                        println!("Activated skill: {}", name);
                        let active = app.active_skills();
                        println!("Active skills: {}", active.join(", "));
                    }
                    Err(e) => println!("Error: {}", e),
                }
            }
            Ok(CommandResult::Handled)
        }
        _ if line.starts_with("/template ") => {
            handle_template_expand(line, templates, session).await
        }
        "/model" => {
            let current = session.model_id();
            println!("Current model: {}", current);
            Ok(CommandResult::Handled)
        }
        _ if line.starts_with("/model ") => {
            let model_id = line["/model ".len()..].trim();
            if model_id.is_empty() {
                println!("Usage: /model <provider/model>");
                println!("Example: /model openai/gpt-4o");
            } else {
                match session.switch_model(model_id) {
                    Ok(()) => {
                        println!("Switched model to: {}", model_id);
                    }
                    Err(e) => {
                        println!("Error switching model: {}", e);
                    }
                }
            }
            Ok(CommandResult::Handled)
        }
        "/models" => {
            let providers = oxi_ai::get_providers();
            for provider in &providers {
                let models = oxi_ai::get_models(provider);
                if !models.is_empty() {
                    println!("\n{}:", provider);
                    for model in models {
                        println!("  {}/{}", provider, model.id);
                    }
                }
            }
            Ok(CommandResult::Handled)
        }
        _ if line.starts_with("/fork ") => {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(entry_id) = Uuid::parse_str(parts[1]) {
                    if let Some(ref session_id) = current_session_id {
                        let sessions = manager.list_sessions().await?;
                        if let Some(info) = sessions
                            .iter()
                            .find(|s| s.id.to_string().starts_with(session_id.as_str()))
                        {
                            match manager.branch_from(info.id, entry_id).await {
                                Ok((new_id, _)) => {
                                    println!("Created forked session: {}", new_id);
                                    return Ok(CommandResult::NewSession(new_id));
                                }
                                Err(e) => println!("Error forking: {}", e),
                            }
                        } else {
                            println!("Session not found.");
                        }
                    } else {
                        println!("No active session to fork from.");
                    }
                } else {
                    println!("Usage: /fork <entry_id>");
                }
                Ok(CommandResult::Handled)
            } else {
                Ok(CommandResult::Handled)
            }
        }
        "/history" => {
            for msg in session.messages() {
                let prefix = match msg.role.as_str() {
                    "user" => "👤",
                    "assistant" => "◉",
                    _ => "",
                };
                println!("{} {}", prefix, msg.content);
                println!();
            }
            Ok(CommandResult::Handled)
        }
        _ => {
            println!("Unknown command. Type /help for available commands.");
            Ok(CommandResult::Handled)
        }
    }
}
