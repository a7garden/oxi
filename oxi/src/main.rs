//! oxi CLI - CLI coding harness for oxi
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::{Parser, Subcommand};
use oxi::session::{SessionManager, AgentMessage};
use oxi::settings::Settings;
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
}

/// Parse thinking level from string
fn parse_thinking_level(s: &str) -> Option<oxi::settings::ThinkingLevel> {
    match s.to_lowercase().as_str() {
        "none" => Some(oxi::settings::ThinkingLevel::None),
        "minimal" => Some(oxi::settings::ThinkingLevel::Minimal),
        "standard" => Some(oxi::settings::ThinkingLevel::Standard),
        "thorough" => Some(oxi::settings::ThinkingLevel::Thorough),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Parse arguments
    let args = Args::parse();

    // Handle session commands
    if let Some(command) = &args.command {
        return handle_session_command(command).await;
    }

    // Load settings
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

    // Build initial prompt if provided
    let prompt = args.prompt.join(" ");

    // Create app
    let app = oxi::App::new(settings).await?;

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
async fn handle_session_command(command: &Commands) -> Result<()> {
    let manager = SessionManager::new().await?;

    match command {
        Commands::Sessions => {
            list_sessions(&manager).await?;
        }
        Commands::Tree { session_id } => {
            show_tree(&manager, session_id).await?;
        }
        Commands::Fork { parent_id, entry_id } => {
            fork_session(&manager, parent_id, entry_id).await?;
        }
        Commands::Delete { session_id } => {
            delete_session(&manager, session_id).await?;
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

    println!("oxi CLI - type your message and press Enter. Ctrl+C or 'exit' to quit.");
    println!("Commands: /sessions, /tree, /fork <entry_id>, /history, /help");
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
            match handle_command(line, &mut session_manager, &mut session, current_session_id).await? {
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

async fn handle_command(
    line: &str,
    manager: &mut SessionManager,
    session: &mut oxi::InteractiveLoop<'_>,
    current_session_id: Option<Uuid>,
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
        "/help" => {
            println!("Commands:");
            println!("  /sessions      - List all sessions");
            println!("  /tree           - Show current session tree");
            println!("  /fork <id>      - Fork from an entry");
            println!("  /history        - Show conversation history");
            println!("  /help           - Show this help");
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