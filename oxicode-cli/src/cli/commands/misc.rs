//! Miscellaneous subcommand handlers: completions, install, update, commit,
//! models, refresh, and the catalog builder used by `models` / `refresh`.

use crate::store::settings::Settings;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

/// Handle `oxicode completions <bash|zsh|fish>` — print shell completion script.
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

/// Handle `oxicode install <source>` — dispatch to `ext install` or `pkg install`.
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

/// Handle `oxicode update [--check]` — refresh the binary and converge it
/// into the ecosystem-standard managed layout
/// (`~/.oxi/oxicode/{bin,versions}`).
///
/// The fetch channel is `cargo install oxicode-cli --force` — crates.io and
/// `binstall` already sign and pre-build the binary. Once the new
/// binary is in place, [`managed_install::adopt_binary`] moves it under
/// `~/.oxi/oxicode/versions/<v>/`, flips the launcher, repoints the
/// cargo bin copy at the launcher, and prunes older versions (keep 2).
pub async fn handle_update(check: bool) -> Result<()> {
    #[cfg(feature = "self-update")]
    {
        use self_update::cargo_crate_version;

        let current = cargo_crate_version!();
        println!("Current version: v{current}");

        if check {
            report_layout_status();
            return Ok(());
        }

        // 1. Fetch through the supported distribution channel.
        println!("Updating oxicode via `cargo install oxicode-cli --force`...");
        let status = tokio::process::Command::new("cargo")
            .args(["install", "oxicode-cli", "--force"])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            anyhow::bail!("Update failed: cargo install exited {status}");
        }

        // 2. Adopt the freshly-installed binary into the managed layout.
        match adopt_into_managed_layout().await {
            Ok(Some(launcher)) => println!(
                "✅ oxicode converged into the managed layout at {} (cargo bin repointed).",
                launcher.display(),
            ),
            Ok(None) => {
                println!("✅ oxicode updated successfully. Restart to use the new version.")
            }
            Err(e) => println!(
                "warning: oxicode updated but adopt into the managed layout failed: {e} \
                 (the cargo-installed binary still works at its previous location)"
            ),
        }
        Ok(())
    }

    #[cfg(not(feature = "self-update"))]
    {
        let _ = check;
        anyhow::bail!("Self-update is not available (compiled without `self-update` feature)");
    }
}

/// Print the current managed-layout status: launcher, current version,
/// known shadow roots. Used by `oxicode update --check`.
fn report_layout_status() {
    use crate::managed_install;
    let Some(home) = oxicode_catalog::oxi_home::oxicode_home() else {
        println!("managed layout: (no resolvable Oxi home — set $OXI_HOME or $OXICODE_HOME)");
        return;
    };
    let launcher = managed_install::launcher_path(&home);
    if launcher.is_file() {
        let target = std::fs::read_link(&launcher).unwrap_or_default();
        println!(
            "managed layout: {} → {}",
            launcher.display(),
            target.display()
        );
        let versions = managed_install::versions_dir(&home);
        if let Ok(entries) = std::fs::read_dir(&versions) {
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_string_lossy().into_owned().into())
                .filter(|n: &String| managed_install::parse_version_dir(n).is_some())
                .collect();
            names.sort();
            println!(
                "versions:       {}",
                if names.is_empty() {
                    "(none)".into()
                } else {
                    names.join(", ")
                }
            );
        }
    } else {
        println!("managed layout: (no launcher at {})", launcher.display());
    }
    let shadows = shadow_roots();
    if !shadows.is_empty() {
        println!(
            "shadowed by:    {}",
            shadows
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

async fn adopt_into_managed_layout() -> Result<Option<PathBuf>> {
    use crate::managed_install;
    let Some(home) = oxicode_catalog::oxi_home::oxicode_home() else {
        return Ok(None);
    };
    let cargo_bin = managed_install::cargo_oxicode_bin();
    let Some(cargo_bin) = cargo_bin else {
        return Ok(None);
    };
    if !cargo_bin.is_file() {
        return Ok(None);
    }
    let Some(version) = managed_install::version_of(&cargo_bin) else {
        return Ok(None);
    };
    let launcher = tokio::task::spawn_blocking({
        let home = home.clone();
        let cargo_bin = cargo_bin.clone();
        let relink = cargo_bin.clone();
        move || managed_install::adopt_binary(&home, &cargo_bin, &version, Some(&relink))
    })
    .await
    .context("managed layout adopt task panicked")??;
    Ok(Some(launcher))
}

/// Other recognized `oxicode` locations (cargo bin, alternate PATH
/// entries, legacy `~/.oxicode` if it held a binary) — for shadow
/// diagnostics. Excludes the managed launcher.
fn shadow_roots() -> Vec<PathBuf> {
    use crate::managed_install;
    let mut out = Vec::new();
    let winner = managed_install::launcher_path(
        &oxicode_catalog::oxi_home::oxicode_home().unwrap_or_else(|| std::path::PathBuf::from(".")),
    );
    let mut seen = Vec::new();
    let push = |path: PathBuf, seen: &mut Vec<PathBuf>, out: &mut Vec<PathBuf>| {
        if path == winner || !path.is_file() || seen.contains(&path) {
            return;
        }
        seen.push(path.clone());
        out.push(path);
    };
    if let Some(cargo_bin) = managed_install::cargo_oxicode_bin() {
        push(cargo_bin, &mut seen, &mut out);
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            push(dir.join("oxicode"), &mut seen, &mut out);
        }
    }
    if let Some(legacy) = oxicode_catalog::oxi_home::legacy_home_dir() {
        push(legacy.join("bin/oxicode"), &mut seen, &mut out);
    }
    out
}

/// Handle `oxicode commit [--push] [--dry-run] [-c <context>]`.
pub async fn handle_commit(push: bool, dry_run: bool, context: Option<&str>) -> Result<()> {
    use oxicode_agent::AgentTool;
    use oxicode_agent::tools::ToolContext;
    use oxicode_agent::tools::commit::CommitTool;
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

/// Handle `oxicode refresh` — force-refresh the model catalog from models.dev.
///
/// Performs a conditional GET (ETag). The refreshed cache takes effect
/// on the next process start (the in-memory catalog is immutable).
///
/// Uses the catalog port (`FileModelCatalog`) directly. The `App`'s
/// catalog would be equivalent but this command runs standalone (no App).
pub async fn handle_refresh() -> Result<()> {
    use oxicode_sdk::ModelCatalog;
    use oxicode_sdk::ports::catalog::RefreshOutcome;
    use oxicode_sdk::ports::fs::{CatalogConfig, FileModelCatalog};

    let paths = crate::services::OxicodePaths::default_paths()?;
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

/// Handle `oxicode models [--provider <name>]`
pub async fn handle_models(provider: &Option<String>) -> Result<()> {
    use oxicode_sdk::ModelCatalog;

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
                match oxicode_sdk::fetch_models_blocking(&cp.base_url, key) {
                    Ok(model_ids) => {
                        let api_type = match cp.api.to_lowercase().as_str() {
                            "openai-responses" | "responses" => oxicode_sdk::Api::OpenAiResponses,
                            _ => oxicode_sdk::Api::OpenAiCompletions,
                        };
                        for model_id in &model_ids {
                            // Cross-fill real metadata from models.dev when
                            // the id matches an upstream model (see
                            // bootstrap::fetch_and_register_models).
                            let known = oxicode_sdk::find_entry_by_model_id(model_id);
                            let model = oxicode_sdk::Model {
                                id: model_id.clone(),
                                name: model_id.clone(),
                                api: api_type,
                                provider: cp.name.clone(),
                                base_url: cp.base_url.clone(),
                                reasoning: known.map(|e| e.reasoning).unwrap_or(false),
                                input: vec![oxicode_sdk::InputModality::Text],
                                cost: known
                                    .map(|e| oxicode_sdk::Cost {
                                        input: e.cost_input.max(0.0),
                                        output: e.cost_output.max(0.0),
                                        cache_read: e.cost_cache_read.max(0.0),
                                        cache_write: e.cost_cache_write.max(0.0),
                                    })
                                    .unwrap_or_default(),
                                context_window: known
                                    .map(|e| e.context_window as usize)
                                    .unwrap_or(128_000),
                                max_tokens: known.map(|e| e.max_tokens as usize).unwrap_or(8_192),
                                headers: Default::default(),
                                compat: None,
                            };
                            oxicode_sdk::register_model(model);
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
                            "[oxicode] warning: failed to resolve models for {}: {}",
                            provider_name, e
                        );
                    }
                }
            } else {
                eprintln!(
                    "[oxicode] API key not set for provider '{}' (expected: {})",
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

/// Build a catalog port for `oxicode models` / `oxicode refresh` standalone commands.
///
/// These commands run without an `App`; we construct a fresh `FileModelCatalog`
/// rooted at the conventional oxicode home directory. The result is a short-lived
/// catalog used only for this one command.
pub(crate) async fn build_catalog_for_cli() -> Result<Arc<oxicode_sdk::FileModelCatalog>> {
    use oxicode_sdk::ports::fs::CatalogConfig;
    let paths = crate::services::OxicodePaths::default_paths()?;
    let config = CatalogConfig {
        cache_path: paths.home.join("cache").join("models-dev.json"),
        etag_path: paths.home.join("cache").join("models-dev.json.etag"),
        override_path: paths.home.join("catalog").join("overrides.toml"),
        // Don't trigger a refresh during `oxicode models`; users who want fresh
        // data should run `oxicode refresh` first.
        fetch_enabled: false,
        ..Default::default()
    };
    Ok(oxicode_sdk::FileModelCatalog::init(config).await?)
}
