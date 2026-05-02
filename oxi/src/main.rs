//! oxi CLI - CLI coding harness for oxi
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::{Parser, Subcommand};
use oxi::extensions::ExtensionRegistry;
use oxi::packages::PackageManager;
use oxi::session::{SessionManager, AgentMessage};
use oxi::settings::Settings;
use oxi::skills::SkillManager;
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
        // Interactive mode
        interactive_mode(app).await?;
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
        Commands::Fork { parent_id, entry_id } => {
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
                println!(
                    "Installed {} v{} from npm",
                    manifest.name, manifest.version
                );
            } else {
                let manifest = mgr.install(source)?;
                println!("Installed {} v{}", manifest.name, manifest.version);
            }
        }
        PkgCommands::List => {
            let packages = mgr.list();
            if packages.is_empty() {
                println!("No packages installed.");
            } else {
                println!("{:<30} {:<10} {}", "NAME", "VERSION", "RESOURCES");
                println!("{:-<30} {:-<10} {:-<20}", "", "", "");
                for pkg in packages {
                    let mut resources = Vec::new();
                    if !pkg.extensions.is_empty() {
                        resources.push(format!("{} ext", pkg.extensions.len()));
                    }
                    if !pkg.skills.is_empty() {
                        resources.push(format!("{} skill", pkg.skills.len()));
                    }
                    if !pkg.prompts.is_empty() {
                        resources.push(format!("{} prompt", pkg.prompts.len()));
                    }
                    if !pkg.themes.is_empty() {
                        resources.push(format!("{} theme", pkg.themes.len()));
                    }
                    println!(
                        "{:<30} {:<10} {}",
                        pkg.name,
                        pkg.version,
                        if resources.is_empty() {
                            "-".to_string()
                        } else {
                            resources.join(", ")
                        }
                    );
                }
            }
        }
        PkgCommands::Uninstall { name } => {
            mgr.uninstall(name)?;
            println!("Uninstalled {}", name);
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

    let tree = manager.get_tree(id).await?;
    let branch_info = manager.get_branch_info(id).await?;

    if let Some(info) = branch_info {
        println!("Session: {} (branched from {})", id, info.parent_session_id.unwrap());
    } else {
        println!("Session: {} (root)", id);
    }
    println!();

    // Show tree structure
    for (session_id, entry) in &tree {
        let role_marker = match &entry.message {
            AgentMessage::User { .. } => "👤",
            AgentMessage::Assistant { .. } => "🤖",
            AgentMessage::System { .. } => "⚙️",
        };

        let content_preview = truncate(&entry.message.content(), 60);
        let prefix = if entry.parent_id.is_some() { "├─" } else { "└─" };

        println!("  {}{} [{:.8}] {}", prefix, role_marker, entry.id, content_preview);
    }

    Ok(())
}

async fn fork_session(manager: &SessionManager, parent_id_str: &str, entry_id_str: &str) -> Result<()> {
    let parent_id = Uuid::parse_str(parent_id_str)?;
    let entry_id = Uuid::parse_str(entry_id_str)?;

    let (new_id, entries) = manager.branch_from(parent_id, entry_id).await?;

    println!("Created forked session: {}", new_id);
    println!("Copied {} entries from {}", entries.len(), parent_id);

    Ok(())
}

async fn delete_session(manager: &SessionManager, session_id: &str) -> Result<()> {
    let id = Uuid::parse_str(session_id)?;
    manager.delete(id).await?;
    println!("Deleted session: {}", id);
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

enum CommandResult {
    Handled,
    NewSession(Uuid),
    Quit,
}

/// Interactive mode (simple readline-based)
async fn interactive_mode(app: oxi::App) -> Result<()> {
    use std::io::{self, Write};

    let mut session_manager = SessionManager::new().await?;
    let mut session = app.run_interactive().await?;
    let mut current_session_id: Option<Uuid> = None;

    // Load prompt templates from ~/.oxi/templates/
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let templates_dir = home.join(".oxi").join("templates");
    let template_manager = TemplateManager::load_from_dir(&templates_dir)?;
    if !template_manager.is_empty() {
        tracing::info!(count = template_manager.len(), "loaded prompt templates");
    }

    println!("oxi CLI - type your message and press Enter. Ctrl+C or 'exit' to quit.");
    println!("Commands: /sessions, /tree, /fork <entry_id>, /model, /skill, /template, /history, /help");
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
            match handle_command(line, &mut session_manager, &mut session, current_session_id, &template_manager, &app).await? {
                CommandResult::Handled => continue,
                CommandResult::NewSession(id) => current_session_id = Some(id),
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

async fn handle_command(
    line: &str,
    manager: &mut SessionManager,
    session: &mut oxi::InteractiveLoop<'_>,
    current_session_id: Option<Uuid>,
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
                for meta in &sessions {
                    let branch = if meta.parent_id.is_some() { "fork" } else { "root" };
                    println!(
                        "  {:.8}  {}  {}",
                        meta.id,
                        branch,
                        chrono::DateTime::from_timestamp_millis(meta.updated_at)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default()
                    );
                }
            }
            Ok(CommandResult::Handled)
        }
        "/tree" | "/tree view" => {
            if let Some(id) = current_session_id {
                let tree = manager.get_tree(id).await?;
                if tree.is_empty() {
                    println!("No entries in session.");
                } else {
                    println!("Session tree:");
                    for (_, entry) in &tree {
                        let role = match &entry.message {
                            AgentMessage::User { .. } => "👤",
                            AgentMessage::Assistant { .. } => "🤖",
                            AgentMessage::System { .. } => "⚙️",
                        };
                        let preview = truncate(&entry.message.content(), 50);
                        println!("  {} [{:.8}]: {}", role, entry.id, preview);
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
                    if let Some(session_id) = current_session_id {
                        match manager.branch_from(session_id, entry_id).await {
                            Ok((new_id, entries)) => {
                                println!("Created forked session: {}", new_id);
                                println!("Copied {} entries", entries.len());
                                return Ok(CommandResult::NewSession(new_id));
                            }
                            Err(e) => println!("Error forking: {}", e),
                        }
                    } else {
                        println!("No active session to fork from.");
                    }
                } else {
                    println!("Invalid entry ID: {}", parts[1]);
                }
            } else {
                println!("Usage: /fork <entry_id>");
            }
            Ok(CommandResult::Handled)
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