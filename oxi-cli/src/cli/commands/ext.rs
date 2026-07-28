//! Extension subcommand handler.

use crate::cli::ExtCommands;
use anyhow::Result;

/// Handle `oxi ext …` subcommands.
pub async fn handle_ext(action: &ExtCommands) -> Result<()> {
    use crate::extensions::ext_cli;

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
