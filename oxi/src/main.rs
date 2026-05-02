//! oxi CLI - CLI coding harness for oxi
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::Parser;
use oxi::settings::Settings;

/// CLI arguments
#[derive(Parser, Debug)]
#[command(name = "oxi")]
#[command(about = "CLI coding harness for oxi")]
#[command(version = "0.1.0")]
struct Args {
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

/// Interactive mode (simple readline-based)
async fn interactive_mode(app: oxi::App) -> Result<()> {
    use std::io::{self, Write};
    
    let mut session = app.run_interactive().await?;
    
    println!("oxi CLI - type your message and press Enter. Ctrl+C or 'exit' to quit.");
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
        
        // Send message and wait for response
        session.send_message(line.to_string()).await?;
        
        // Print assistant response
        for msg in session.messages() {
            if msg.role == "assistant" {
                println!("\n◉ {}\n", msg.content);
            }
        }
        
        // Reset thinking state for next turn
        print!("❯ ");
        io::stdout().flush()?;
    }
    
    Ok(())
}