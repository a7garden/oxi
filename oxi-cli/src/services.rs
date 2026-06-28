//! Composition root for oxi-cli.
//!
//! Wires concrete file-based port implementations (from `oxi-fs`) to the
//! `Oxi` engine. Future run modes (TUI / print / RPC) build on top of
//! the `Oxi` produced here.
//!
//! # Migration note
//!
//! The legacy `App` struct in `lib.rs` is the **single-user interactive**
//! composition (TUI-driven, in-process). This module is the
//! **port-based** composition: a `Oxi` engine with persistence, auth,
//! config, and skills wired via `oxi_sdk::OxiBuilder::with_port_*`.
//!
//! Both paths are expected to coexist during the migration. New run modes
//! should consume `build_oxi(...)` from this module; the legacy `App`
//! path remains for the interactive TUI until it is fully migrated.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use oxi_sdk::Oxi;
use oxi_sdk::fs::{
    FileAuthProvider, FileConfigStore, FileModelCatalog, FilePersonaProvider, FileSkillLoader,
    FileStateStore, SimpleAccessGate, TomlCapabilityResolver,
};
use oxi_sdk::inmem::{
    CountingResourceMonitor, InMemoryCronScheduler, InMemoryMemoryStore, InProcessEventBus,
};
use oxi_sdk::ports::catalog::CatalogEvent;
use oxi_sdk::ports::fs::CatalogConfig;

/// Resolved paths under the oxi home directory.
#[derive(Debug, Clone)]
pub struct OxiPaths {
    /// Root directory (`$OXI_HOME` or `$HOME/.oxi`).
    pub home: PathBuf,
    /// `auth.json` location.
    pub auth: PathBuf,
    /// `settings.toml` location.
    pub config: PathBuf,
    /// Sessions directory.
    pub sessions: PathBuf,
    /// Skills root.
    pub skills: PathBuf,
}

impl OxiPaths {
    /// Resolve from the conventional home directory.
    pub fn from_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            auth: home.join("auth.json"),
            config: home.join("settings.toml"),
            sessions: home.join("sessions"),
            skills: home.join("skills"),
            home,
        }
    }

    /// Default — uses `$OXI_HOME` or `$HOME/.oxi`.
    pub fn default_paths() -> Result<Self> {
        oxi_sdk::fs::home_dir()
            .map(Self::from_home)
            .context("could not resolve oxi home directory")
    }
}

/// Build an `Oxi` engine wired with file-based port implementations.
///
/// This is the **composition root** for oxi-cli. The catalog port
/// (`FileModelCatalog`) performs network I/O during `init()` (cache check
/// + optional one refresh attempt).
///
/// Callers that want a fully sync composition should use the catalog's
/// noop default or pre-construct a `CatalogConfig` with
/// `fetch_enabled: false`.
pub async fn build_oxi(paths: &OxiPaths) -> Result<Oxi> {
    build_oxi_with_catalog(paths, build_catalog_config(paths)).await
}

/// Build an `Oxi` engine with a custom catalog config. Useful for tests
/// (e.g. pointing the catalog at a tempdir).
pub async fn build_oxi_with_catalog(
    paths: &OxiPaths,
    catalog_config: CatalogConfig,
) -> Result<Oxi> {
    ensure_parent(&paths.auth)?;
    ensure_parent(&paths.config)?;
    ensure_parent(&paths.sessions)?;

    // Initialize the catalog (loads embedded SNAP + cache + overrides).
    // Errors here are non-fatal — we fall back to noop and let the user
    // re-run `oxi refresh` to recover.
    let catalog: Arc<dyn oxi_sdk::ports::catalog::ModelCatalog> =
        match FileModelCatalog::init(catalog_config).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "catalog init failed; continuing with noop");
                oxi_sdk::NoopModelCatalog::new()
            }
        };

    let oxi = oxi_sdk::OxiBuilder::new()
        .with_builtins()
        .with_state(Arc::new(FileStateStore::new(&paths.sessions)))
        .with_auth(Arc::new(FileAuthProvider::new(&paths.auth)))
        .with_config(Arc::new(FileConfigStore::new(&paths.config)))
        .with_skills(Arc::new(FileSkillLoader::single(&paths.skills)))
        .with_personas(Arc::new(FilePersonaProvider::new(
            paths.home.join("personas"),
        )))
        .with_access(Arc::new(SimpleAccessGate::from_file(
            paths.home.join("access.toml"),
        )))
        .with_capabilities(Arc::new(TomlCapabilityResolver::from_file(
            paths.home.join("capabilities.toml"),
        )))
        .with_event_bus(InProcessEventBus::new(64))
        .with_memory(Arc::new(InMemoryMemoryStore::new()))
        .with_cron(Arc::new(InMemoryCronScheduler::new()))
        .with_resources(Arc::new(CountingResourceMonitor::new()))
        .with_catalog(catalog)
        .build();

    Ok(oxi)
}

/// Build a `CatalogConfig` rooted at `paths.home` (default cache, ETag,
/// and override file locations).
fn build_catalog_config(paths: &OxiPaths) -> CatalogConfig {
    CatalogConfig {
        cache_path: paths.home.join("cache").join("models-dev.json"),
        etag_path: paths.home.join("cache").join("models-dev.json.etag"),
        override_path: paths.home.join("catalog").join("overrides.toml"),
        mtime_window: std::time::Duration::from_secs(60 * 60),
        fetch_enabled: std::env::var("OXI_MODELS_DEV_DISABLE_FETCH")
            .ok()
            .map(|v| !matches!(v.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(true),
        models_dev_url: std::env::var("OXI_MODELS_DEV_URL")
            .unwrap_or_else(|_| "https://models.dev".to_string()),
        user_agent: format!("oxi-cli/{}", env!("CARGO_PKG_VERSION")),
        local_discovery_urls: local_discovery_from_env(),
        snapshot_path: paths.home.join("cache").join("models-dev.json"),
    }
}

/// Resolve local-discovery URLs from environment.
///
/// `OXI_LOCAL_DISCOVERY` is a comma-separated list of base URLs (e.g.
/// `http://localhost:11434/v1,http://localhost:1234/v1`). Empty default.
fn local_discovery_from_env() -> Vec<String> {
    std::env::var("OXI_LOCAL_DISCOVERY")
        .ok()
        .map(|s| {
            s.split(',')
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Spawn a background task that drains the catalog event channel and logs
/// at info level. The returned `JoinHandle` can be dropped (the task runs
/// for the lifetime of the program — it owns its Receiver clone).
///
/// This is a thin convenience — production consumers should subscribe to
/// `oxi.catalog().subscribe()` directly and react to events (UI refresh,
/// cache invalidation, etc.).
pub fn spawn_catalog_event_logger(
    catalog: Arc<dyn oxi_sdk::ports::catalog::ModelCatalog>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = catalog.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match event {
                CatalogEvent::Updated {
                    provider_count,
                    model_count,
                } => {
                    tracing::info!(provider_count, model_count, "catalog refreshed");
                }
                CatalogEvent::RefreshFailed { reason, .. } => {
                    tracing::warn!(reason, "catalog refresh failed");
                }
                CatalogEvent::OverrideApplied {
                    path,
                    provider_overrides,
                    model_overrides,
                } => {
                    tracing::info!(
                        path = %path.display(),
                        provider_overrides,
                        model_overrides,
                        "catalog overrides applied"
                    );
                }
                CatalogEvent::LocalDiscovered {
                    base_url,
                    model_count,
                } => {
                    tracing::info!(base_url, model_count, "local models discovered");
                }
            }
        }
    })
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }
    Ok(())
}

// ── Memory backend helpers (Hindsight ④) ──────────────────────────────

/// Create a memory backend if memory is enabled in settings.
/// Returns `None` when `memory_enabled` is false or the database cannot
/// be opened.
pub fn create_memory_backend(
    settings: &crate::store::settings::Settings,
) -> Option<Arc<dyn oxi_agent::tools::MemoryBackend>> {
    if !settings.memory_enabled {
        return None;
    }
    let db_path = settings.memory_db_path.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".oxi")
            .join("memory")
            .join("project.db")
    });
    // Ensure the parent directory exists.
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if settings.mnemopi_engine {
        match crate::store::memory_mnemopi::MnemopiMemoryBackend::open(&db_path, "default") {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                tracing::warn!(
                    "Failed to open Mnemopi engine at {}: {e}",
                    db_path.display()
                );
                None
            }
        }
    } else {
        match crate::store::memory_sqlite::SqliteMemoryStore::open(&db_path) {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                tracing::warn!(
                    "Failed to open memory database at {}: {e}",
                    db_path.display()
                );
                None
            }
        }
    }
}

/// Build a project-memory recall block for injection into the system prompt.
/// Returns an empty string when no memories exist.
pub async fn build_memory_recall(
    backend: &dyn oxi_agent::tools::MemoryBackend,
    subject: &str,
) -> String {
    match backend.list(subject).await {
        Ok(items) if !items.is_empty() => {
            let mut block = String::from(
                "\n\n## Project Memory\n\nThe following facts were learned in previous sessions:\n",
            );
            for item in &items {
                block.push_str(&format!("- [{}] {}\n", item.kind, item.content));
            }
            block
        }
        _ => String::new(),
    }
}

/// Store a session summary into the memory backend.
pub async fn session_reflect(
    backend: &dyn oxi_agent::tools::MemoryBackend,
    subject: &str,
    summary: &str,
) {
    if let Err(e) = backend.put(summary, "summary", subject).await {
        tracing::warn!("Failed to store session memory: {e}");
    }
}
#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn paths_are_consistent() {
        let p = OxiPaths::from_home("/tmp/oxi-test");
        assert!(p.auth.starts_with("/tmp/oxi-test"));
        assert!(p.config.starts_with("/tmp/oxi-test"));
        assert!(p.sessions.starts_with("/tmp/oxi-test"));
        assert!(p.skills.starts_with("/tmp/oxi-test"));
    }

    #[tokio::test]
    async fn build_oxi_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = OxiPaths::from_home(tmp.path());
        let oxi = build_oxi(&paths).await.unwrap();
        // State is wired (even if we don't call it).
        let _ = oxi.ports().state;
    }
}
