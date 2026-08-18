//! oxicode CLI - CLI coding harness for oxicode
//!
//! A command-line interface for interacting with AI models.

use anyhow::Result;
use clap::Parser;
use oxicode::cli::commands::*;
use oxicode::cli::{CliArgs, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("OXICODE_PORT_CHECK").is_ok() {
        oxicode::run_port_check().await?;
        std::process::exit(0);
    }
    oxicode::bootstrap::init_logging();
    let args = CliArgs::parse();
    if let Some(command) = &args.command {
        return handle_subcommand(command).await;
    }
    let exit_code = oxicode::bootstrap::run_with_args(args).await?;
    std::process::exit(exit_code);
}

/// Handle session-related commands
async fn handle_subcommand(command: &Commands) -> Result<()> {
    match command {
        Commands::Sessions => handle_sessions().await?,
        Commands::Tree { session_id } => handle_tree(session_id).await?,
        Commands::Fork {
            parent_id,
            entry_id,
        } => handle_fork(parent_id, entry_id).await?,
        Commands::Delete { session_id } => handle_delete(session_id).await?,
        Commands::Pkg { action } => handle_pkg(action)?,
        Commands::Issue { action } => handle_issue(action).await?,
        Commands::Config { action } => handle_config(action)?,
        Commands::Ext { action } => handle_ext(action).await?,
        Commands::Models { provider } => handle_models(provider).await?,
        Commands::Refresh {} => handle_refresh().await?,
        Commands::Setup { reset } => handle_setup(*reset).await?,
        Commands::Reset {
            yes,
            include_project,
        } => handle_reset(*yes, *include_project)?,
        Commands::Export { session_id, output } => {
            handle_export(session_id.as_deref(), output.as_deref())?
        }
        Commands::Import { path } => handle_import(path)?,
        Commands::Share { session_id } => handle_share(session_id.as_deref()).await?,
        Commands::Completions { shell } => handle_completions(shell)?,
        Commands::Install { source } => handle_install(source).await?,
        Commands::Update { check } => handle_update(*check).await?,
        Commands::Commit {
            push,
            dry_run,
            context,
        } => handle_commit(*push, *dry_run, context.as_deref()).await?,
        Commands::Migrate { action } => {
            let code = handle_migrate(action.clone()).await;
            if code != 0 {
                std::process::exit(code);
            }
        }
    }

    Ok(())
}
