//! Miscellaneous subcommand handlers: completions, install, update, commit,
//! models, refresh, and the catalog builder used by `models` / `refresh`.

use crate::store::settings::Settings;
use anyhow::{Context, Result};
use std::sync::Arc;

/// Handle `oxi completions <bash|zsh|fish>` — print shell completion script.
pub fn handle_completions(shell: &str) -> Result<()> {
    use clap::CommandFactory;

    let shell = match shell {
        "bash" => clap_complete::Shell::Bash,
        "zsh" => clap_complete::Shell::Zsh,
        "fish" => clap_complete::Shell::Fish,
        "elvish" => clap_complete::Shell::Elvish,
        "powershell" => clap_complete::Shell::PowerShell,
        _ => {
            anyhow::bail!("Unknown shell: {shell}. Supported: bash, zsh, fish, elvish, powershell")
        }
    };

    let mut cmd = crate::cli::CliArgs::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(())
}

/// Handle `oxi install <source>` — dispatch to `ext install` or `pkg install`.
pub async fn handle_install(source: &str) -> Result<()> {
    use crate::cli::{ExtCommands, PkgCommands};

    // Local paths or npm: prefix → pkg install
    if source.starts_with('.')
        || source.starts_with('/')
        || source.starts_with('~')
        || source.starts_with("npm:")
    {
        super::pkg::handle_pkg(&PkgCommands::Install {
            source: source.to_string(),
        })?;
    } else {
        // GitHub repo spec (owner/repo) → ext install
        super::ext::handle_ext(&ExtCommands::Install {
            source: source.to_string(),
            prerelease: false,
        })
        .await?;
    }
    Ok(())
}

/// Handle `oxi update [--check]` — check for and install updates.
pub async fn handle_update(check: bool) -> Result<()> {
    #[cfg(feature = "self-update")]
    {
        use self_update::cargo_crate_version;

        let current = cargo_crate_version!();
        println!("Current version: v{current}");

        if check {
            return Ok(());
        }

        // Use cargo install via `cargo install oxi-cli --force`
        println!("Updating oxi...");
        let status = tokio::process::Command::new("cargo")
            .args(["install", "oxi-cli", "--force"])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            anyhow::bail!("Update failed: cargo install exited with {status}");
        }

        println!("✅ oxi updated successfully. Restart to use the new version.");
        Ok(())
    }

    #[cfg(not(feature = "self-update"))]
    {
        let _ = check;
        anyhow::bail!("Self-update is not available (compiled without `self-update` feature)");
    }
}

/// Handle `oxi commit [--push] [--dry-run] [-c <context>]`.
pub async fn handle_commit(push: bool, dry_run: bool, context: Option<&str>) -> Result<()> {
    use oxi_agent::AgentTool;
    use oxi_agent::tools::ToolContext;
    use oxi_agent::tools::commit::CommitTool;
    use serde_json::json;

    // Check for staged changes
    let diff_output = tokio::process::Command::new("git")
        .args(["diff", "--cached"])
        .output()
        .await
        .with_context(|| "Failed to run git diff --cached")?;

    if diff_output.stdout.is_empty() && diff_output.stderr.is_empty() {
        let has_changes = tokio::process::Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .await
            .with_context(|| "Failed to run git status")?;
        if has_changes.stdout.is_empty() {
            anyhow::bail!("Nothing to commit. Working tree is clean.");
        }
        anyhow::bail!(
            "No staged changes. Use `git add` to stage files, or include unstaged changes with `git commit -a`."
        );
    }

    // Run CommitTool (deterministic-only in CLI mode — no agent context)
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let ctx = ToolContext::new(cwd.clone());
    let tool = CommitTool::unconfigured();
    let params = json!({
        "dry_run": dry_run,
        "push": push,
        "context": context.unwrap_or(""),
    });
    let result = tool
        .execute("cli", params, None, &ctx)
        .await
        .map_err(|e| anyhow::anyhow!("Commit tool failed: {e}"))?;

    if dry_run {
        println!("{}", result.output);
        return Ok(());
    }

    // Actual commit: run `git commit -m "<message>"`
    let message = result
        .output
        .lines()
        .find(|l| {
            l.starts_with("feat")
                || l.starts_with("fix")
                || l.starts_with("chore")
                || l.starts_with("docs")
                || l.starts_with("refactor")
                || l.starts_with("test")
                || l.starts_with("perf")
                || l.starts_with("build")
                || l.starts_with("ci")
                || l.starts_with("style")
                || l.starts_with("revert")
        })
        .unwrap_or("feat: commit")
        .to_string();

    let status = tokio::process::Command::new("git")
        .args(["commit", "-m", &message])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .with_context(|| "Failed to run git commit")?;

    if !status.success() {
        anyhow::bail!("Commit failed.\nProposed message was:\n{message}");
    }

    println!("Committed: {message}");

    if push {
        let push_status = tokio::process::Command::new("git")
            .args(["push"])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await
            .with_context(|| "Failed to run git push")?;
        if !push_status.success() {
            anyhow::bail!("Commit succeeded but push failed.");
        }
        println!("Pushed.");
    }

    Ok(())
}

/// Handle `oxi refresh` — force-refresh the model catalog from models.dev.
///
/// Performs a conditional GET (ETag). The refreshed cache takes effect
/// on the next process start (the in-memory catalog is immutable).
///
/// Uses the catalog port (`FileModelCatalog`) directly. The `App`'s
/// catalog would be equivalent but this command runs standalone (no App).
pub async fn handle_refresh() -> Result<()> {
    use oxi_sdk::ModelCatalog;
    use oxi_sdk::ports::catalog::RefreshOutcome;
    use oxi_sdk::ports::fs::{CatalogConfig, FileModelCatalog};

    let paths = crate::services::OxiPaths::default_paths()?;
    let config = CatalogConfig {
        cache_path: paths.home.join("cache").join("models-dev.json"),
        etag_path: paths.home.join("cache").join("models-dev.json.etag"),
        override_path: paths.home.join("catalog").join("overrides.toml"),
        // Bypass the mtime window so we always issue a conditional GET.
        mtime_window: std::time::Duration::ZERO,
        ..Default::default()
    };
    // We don't run `init`'s optional pre-refresh — load SNAP+cache, then
    // explicitly call refresh to issue a conditional GET.
    let cat = FileModelCatalog::init(config).await?;

    println!("Refreshing model catalog from models.dev...");
    match cat.refresh().await? {
        RefreshOutcome::Updated {
            provider_count,
            model_count,
        } => {
            println!(
                "✓ Catalog updated: {} providers, {} models.",
                provider_count, model_count
            );
        }
        RefreshOutcome::Unchanged => {
            println!("✓ Catalog already up to date.");
        }
        RefreshOutcome::Offline { reason } => {
            println!("⚠ Catalog refresh skipped (offline: {reason}).");
        }
        RefreshOutcome::Failed { reason } => {
            println!("✗ Catalog refresh failed: {reason}.");
        }
    }
    Ok(())
}

/// Handle `oxi models [--provider <name>]`
pub async fn handle_models(provider: &Option<String>) -> Result<()> {
    use oxi_sdk::ModelCatalog;

    // If a custom provider is specified, also try to fetch models dynamically
    if let Some(ref provider_name) = *provider {
        let settings = Settings::load().unwrap_or_default();
        if let Some(cp) = settings
            .custom_providers
            .iter()
            .find(|cp| cp.name == *provider_name)
        {
            let auth = crate::store::auth_storage::shared_auth_storage();
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

        // Fallback: show catalog models for this provider via the port.
        let cat = build_catalog_for_cli().await?;
        let models = cat.list_models(provider_name).await?;
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
                println!("  {} ({})", m.model_id, m.name);
            }
        }
        return Ok(());
    }

    // No provider filter: show everything via the catalog port.
    let cat = build_catalog_for_cli().await?;
    let all = cat.search("").await?;
    let count = cat.model_count().await?;
    println!("Available models ({} total):", count);
    for entry in &all {
        println!("  {}/{} — {}", entry.provider, entry.model_id, entry.name);
    }
    Ok(())
}

/// Build a catalog port for `oxi models` / `oxi refresh` standalone commands.
///
/// These commands run without an `App`; we construct a fresh `FileModelCatalog`
/// rooted at the conventional oxi home directory. The result is a short-lived
/// catalog used only for this one command.
pub(crate) async fn build_catalog_for_cli() -> Result<Arc<oxi_sdk::FileModelCatalog>> {
    use oxi_sdk::ports::fs::CatalogConfig;
    let paths = crate::services::OxiPaths::default_paths()?;
    let config = CatalogConfig {
        cache_path: paths.home.join("cache").join("models-dev.json"),
        etag_path: paths.home.join("cache").join("models-dev.json.etag"),
        override_path: paths.home.join("catalog").join("overrides.toml"),
        // Don't trigger a refresh during `oxi models`; users who want fresh
        // data should run `oxi refresh` first.
        fetch_enabled: false,
        ..Default::default()
    };
    Ok(oxi_sdk::FileModelCatalog::init(config).await?)
}
