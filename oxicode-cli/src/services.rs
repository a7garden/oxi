//! Composition root for oxicode-cli.
//!
//! Wires concrete file-based port implementations (from `oxicode-fs`) to
//! the `Oxicode` engine. Future run modes (TUI / print / RPC) build on
//! top of the `Oxicode` produced here.
//!
//! Migration note:
//! - Legacy `App` in `lib.rs` is the single-user interactive
//!   composition. This module is the port-based composition.
//! - Both paths coexist; new run modes consume `build_oxicode(...)` here.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};

use oxicode_sdk::Oxicode;
use oxicode_sdk::fs::{
    FileConfigStore, FileModelCatalog, FilePersonaProvider, FileSkillLoader, FileStateStore,
    SimpleAccessGate, TomlCapabilityResolver,
};
use oxicode_sdk::inmem::{
    CountingResourceMonitor, InMemoryCronScheduler, InMemoryMemoryStore, InProcessEventBus,
};
use oxicode_sdk::ports::InternalUrlRouter;
use oxicode_sdk::ports::catalog::CatalogEvent;
use oxicode_sdk::ports::fs::CatalogConfig;
use oxicode_sdk::ports::inmem::url_router::CompositeUrlRouter;

use crate::internal_urls::issue_handler::IssueProtocolHandler;
use crate::internal_urls::memory_handler::MemoryProtocolHandler;
use crate::internal_urls::pr_handler::PrProtocolHandler;
use crate::store::extracting_backend;
use crate::store::memory_summary;
use crate::store::memory_workers;

/// Resolved paths under the oxicode home directory.
#[derive(Debug, Clone)]
pub struct OxicodePaths {
    /// Root directory (`$OXICODE_HOME` or `$HOME/.oxicode`).
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

impl OxicodePaths {
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

    /// Default — uses `$OXICODE_HOME` or `$HOME/.oxicode`.
    pub fn default_paths() -> Result<Self> {
        oxicode_sdk::fs::home_dir()
            .map(Self::from_home)
            .context("could not resolve oxicode home directory")
    }
}

/// Build an `Oxicode` engine wired with file-based port implementations.
///
/// This is the **composition root** for oxicode-cli. The catalog port
/// performs network I/O during `init()`. Errors there fall back to
/// a noop catalog so the user can re-run `oxicode refresh` later.
pub async fn build_oxicode(
    paths: &OxicodePaths,
    embedding_provider: Option<Arc<dyn oxicode_sdk::ports::EmbeddingProvider>>,
) -> Result<Oxicode> {
    build_oxicode_with_catalog(paths, build_catalog_config(paths), embedding_provider).await
}

/// Build an `Oxicode` engine with a custom catalog config. Useful for
/// tests (e.g. pointing the catalog at a tempdir).
pub async fn build_oxicode_with_catalog(
    paths: &OxicodePaths,
    catalog_config: CatalogConfig,
    embedding_provider: Option<Arc<dyn oxicode_sdk::ports::EmbeddingProvider>>,
) -> Result<Oxicode> {
    ensure_parent(&paths.auth)?;
    ensure_parent(&paths.config)?;
    ensure_parent(&paths.sessions)?;

    let catalog: Arc<dyn oxicode_sdk::ports::catalog::ModelCatalog> =
        match FileModelCatalog::init(catalog_config).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "catalog init failed; continuing with noop");
                oxicode_sdk::NoopModelCatalog::new()
            }
        };

    let skill_loader = Arc::new(FileSkillLoader::single(&paths.skills));
    let rule_registry: Arc<dyn oxicode_sdk::ports::RuleRegistry> =
        Arc::new(oxicode_sdk::ports::NoopRuleRegistry);
    let agent_artifact_store = crate::internal_urls::agent_handler::AgentArtifactStore::new();
    let local_root = paths.home.join("local-artifacts");

    let mut builder = oxicode_sdk::OxicodeBuilder::new()
        .with_builtins()
        .with_state(Arc::new(FileStateStore::new(&paths.sessions)))
        .with_auth(crate::store::auth_storage::shared_auth_storage())
        .with_config(Arc::new(FileConfigStore::new(&paths.config)))
        .with_skills(skill_loader.clone())
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
        .with_url_router(build_url_router(
            paths,
            skill_loader,
            rule_registry,
            agent_artifact_store,
            local_root,
        ));

    if let Some(ep) = embedding_provider {
        builder = builder.with_embeddings(ep);
    }

    let oxicode = builder.build();

    Ok(oxicode)
}
fn build_url_router(
    paths: &OxicodePaths,
    skill_loader: Arc<dyn oxicode_sdk::ports::SkillLoader>,
    rule_registry: Arc<dyn oxicode_sdk::ports::RuleRegistry>,
    agent_store: crate::internal_urls::agent_handler::AgentArtifactStore,
    local_root: PathBuf,
) -> Arc<dyn InternalUrlRouter> {
    let memory_root = paths.home.join("memory");
    let router = CompositeUrlRouter::new();
    router.register(Arc::new(MemoryProtocolHandler::new(memory_root)));
    router.register(Arc::new(IssueProtocolHandler));
    router.register(Arc::new(PrProtocolHandler));
    router.register(Arc::new(
        crate::internal_urls::skill_handler::SkillProtocolHandler::new(skill_loader),
    ));
    router.register(Arc::new(
        crate::internal_urls::rule_handler::RuleProtocolHandler::new(rule_registry),
    ));
    router.register(Arc::new(
        crate::internal_urls::agent_handler::AgentProtocolHandler::new(agent_store),
    ));
    router.register(Arc::new(
        crate::internal_urls::local_handler::LocalProtocolHandler::new(local_root),
    ));
    Arc::new(router)
}

/// Build a `CatalogConfig` rooted at `paths.home`.
fn build_catalog_config(paths: &OxicodePaths) -> CatalogConfig {
    CatalogConfig {
        cache_path: paths.home.join("cache").join("models-dev.json"),
        etag_path: paths.home.join("cache").join("models-dev.json.etag"),
        override_path: paths.home.join("catalog").join("overrides.toml"),
        mtime_window: std::time::Duration::from_secs(60 * 60),
        fetch_enabled: std::env::var("OXICODE_MODELS_DEV_DISABLE_FETCH")
            .ok()
            .map(|v| !matches!(v.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(true),
        models_dev_url: std::env::var("OXICODE_MODELS_DEV_URL")
            .unwrap_or_else(|_| "https://models.dev".to_string()),
        user_agent: format!("oxicode-cli/{}", env!("CARGO_PKG_VERSION")),
        local_discovery_urls: local_discovery_from_env(),
        snapshot_path: paths.home.join("cache").join("models-dev.json"),
    }
}

/// Resolve local-discovery URLs from environment.
///
/// `OXICODE_LOCAL_DISCOVERY` is a comma-separated list of base URLs.
fn local_discovery_from_env() -> Vec<String> {
    std::env::var("OXICODE_LOCAL_DISCOVERY")
        .ok()
        .map(|s| {
            s.split(',')
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Spawn a background task that drains the catalog event channel and
/// logs at info level.
pub fn spawn_catalog_event_logger(
    catalog: Arc<dyn oxicode_sdk::ports::catalog::ModelCatalog>,
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

// ── Memory embedding provider (Hindsight ④ + Gap-1 wiring) ────

/// Build the embedding provider configured by the user.
///
/// Returns `None` when `settings.embedding_provider == "none"`,
/// when base URL / API key are missing, or when the remote provider
/// fails to construct. Failures are non-fatal.
pub fn build_embedding_provider(
    settings: &crate::store::settings::Settings,
) -> Option<Arc<dyn oxicode_mnemopi::EmbeddingProvider>> {
    match settings.embedding_provider.as_str() {
        "remote" => build_remote_embedding_provider(settings),
        _ => None,
    }
}

/// Construct a `RemoteEmbeddingProvider` from settings.
fn build_remote_embedding_provider(
    settings: &crate::store::settings::Settings,
) -> Option<Arc<dyn oxicode_mnemopi::EmbeddingProvider>> {
    let base_url = settings.embedding_base_url.as_deref()?.trim();
    if base_url.is_empty() {
        tracing::warn!("memory: embedding_provider='remote' but embedding_base_url is empty");
        return None;
    }
    let api_key = std::env::var(&settings.embedding_api_key_env).ok()?;
    if api_key.is_empty() {
        tracing::warn!(
            "memory: embedding_provider='remote' but env var {} is unset",
            settings.embedding_api_key_env
        );
        return None;
    }
    let model = if settings.embedding_model.is_empty() {
        "text-embedding-3-small".to_string()
    } else {
        settings.embedding_model.clone()
    };
    Some(Arc::new(oxicode_mnemopi::RemoteEmbeddingProvider::new(
        base_url, &api_key, &model,
    )))
}

// ── Embedding port bridge (mnemopi → SDK async port) ──────────────────

/// Bridges oxicode-mnemopi's synchronous [`oxicode_mnemopi::EmbeddingProvider`] to the SDK's
/// async [`oxicode_sdk::ports::EmbeddingProvider`] port trait. Each `embed()`
/// call runs on the blocking thread pool via `spawn_blocking`.
pub struct MnemopiEmbeddingBridge {
    inner: Arc<dyn oxicode_mnemopi::EmbeddingProvider>,
}

impl MnemopiEmbeddingBridge {
    /// Wrap a mnemopi embedding provider into the SDK port trait.
    pub fn new(inner: Arc<dyn oxicode_mnemopi::EmbeddingProvider>) -> Self {
        Self { inner }
    }
}

impl oxicode_sdk::ports::EmbeddingProvider for MnemopiEmbeddingBridge {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, oxicode_sdk::SdkError>> + Send + 'a>> {
        Box::pin(async move {
            let inner = Arc::clone(&self.inner);
            let text = text.to_string();
            let result = tokio::task::spawn_blocking(move || inner.embed(&[text]))
                .await
                .map_err(|e| {
                    oxicode_sdk::SdkError::Internal(anyhow::anyhow!("embedding task panicked: {e}"))
                })?;
            let mut vectors = result.map_err(|e| {
                oxicode_sdk::SdkError::Internal(anyhow::anyhow!("embedding failed: {e}"))
            })?;
            vectors.pop().ok_or_else(|| {
                oxicode_sdk::SdkError::Internal(anyhow::anyhow!("embedding returned no vectors"))
            })
        })
    }
}

// ── Memory backend helpers (Hindsight ④) ──────────────────────────────

/// Create a memory backend if memory is enabled in settings.
///
/// Returns `None` when `memory_enabled` is false or the database
/// cannot be opened.
pub fn create_memory_backend(
    settings: &crate::store::settings::Settings,
) -> Option<Arc<dyn oxicode_agent::tools::MemoryBackend>> {
    if !settings.memory_enabled {
        return None;
    }
    let db_path = settings.memory_db_path.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".oxicode")
            .join("memory")
            .join("project.db")
    });
    // Ensure the parent directory exists.
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if settings.mnemopi_engine {
        let embedding_provider = build_embedding_provider(settings);
        let embedding_model = settings.embedding_model.clone();
        match crate::store::memory_mnemopi::MnemopiMemoryBackend::open(
            &db_path,
            "default",
            embedding_provider,
            &embedding_model,
        ) {
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

/// Wrap a memory backend with the LLM/heuristic fact extractor.
pub fn wrap_extracting(
    backend: Arc<dyn oxicode_agent::tools::MemoryBackend>,
    settings: &crate::store::settings::Settings,
    oxicode: Option<&oxicode_sdk::Oxicode>,
) -> Arc<dyn oxicode_agent::tools::MemoryBackend> {
    extracting_backend::wrap_with_extractor(backend, settings, oxicode)
}

/// Build a project-memory recall block for injection into the system
/// prompt. Returns an empty string when no memories exist.
pub async fn build_memory_recall(
    backend: &dyn oxicode_agent::tools::MemoryBackend,
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

/// Build the autonomous-memory read-path block (omp `read-path.md`)
/// by reading `<memory-root>/memory_summary.md` if it exists.
pub fn read_path_block(home: &Path, cwd: &Path) -> Option<String> {
    let cwd_str = cwd.to_string_lossy().to_string();
    let memory_root = memory_summary::memory_root(home, &cwd_str);
    let (_, memory_summary_text) =
        memory_summary::load_consolidated_artifacts(&memory_root).ok()?;
    let summary = memory_summary_text?;
    Some(memory_summary::render_read_path(Some(&summary), None))
}

/// Store a session summary into the memory backend.
///
/// **NOTE: currently uncalled** — defined as a future hook point for
/// session-end reflection. Nothing wires it to session lifecycle yet.
/// See FINAL-ROADMAP.md §알려진 갭 (⑨ mental-models).
pub async fn session_reflect(
    backend: &dyn oxicode_agent::tools::MemoryBackend,
    subject: &str,
    summary: &str,
) {
    if let Err(e) = backend.put(summary, "summary", subject).await {
        tracing::warn!("Failed to store session memory: {e}");
    }
}

/// Open (or create) the autonomous-memory pipeline DB and spawn the
/// background Phase-1 / Phase-2 workers. Returns `None` when the
/// pipeline is disabled (default).
///
/// When `oxicode` is `Some`, the pipeline resolves a memory extraction
/// model from settings and creates a provider for actual LLM calls.
/// Without it, workers run but skip LLM-dependent work.
pub fn start_memory_pipeline(
    settings: &crate::store::settings::Settings,
    cwd: &Path,
    oxicode: Option<&oxicode_sdk::Oxicode>,
) -> Option<tokio::task::JoinHandle<()>> {
    let backend = settings.memory_backend.as_deref().unwrap_or("off");
    if backend != "local" {
        tracing::debug!("autonomous memory pipeline: backend='{backend}' — disabled");
        return None;
    }

    let home = crate::store::settings::Settings::settings_dir().ok()?;
    let db_path = memory_workers::pipeline_db_path(&home);
    let sessions_dir = home.join("sessions");

    let cwd_str = cwd.to_string_lossy().to_string();
    let memory_root = memory_summary::memory_root(&home, &cwd_str);

    // Resolve memory extraction model + provider from the Oxicode engine.
    let (provider, model) = if let Some(oxicode) = oxicode {
        let model_id = if settings.memory_llm_extract_model.is_empty() {
            settings.effective_model(None).unwrap_or_default()
        } else {
            settings.memory_llm_extract_model.clone()
        };
        if model_id.is_empty() {
            tracing::warn!("memory pipeline: no model configured for extraction");
            (None, None)
        } else {
            match oxicode.resolve_model(&model_id) {
                Ok(model) => match oxicode.create_provider(&model.provider) {
                    Ok(provider) => (Some(provider), Some(model)),
                    Err(e) => {
                        tracing::warn!("memory pipeline: provider creation failed: {e}");
                        (None, None)
                    }
                },
                Err(e) => {
                    tracing::warn!("memory pipeline: model resolution failed: {e}");
                    (None, None)
                }
            }
        }
    } else {
        tracing::warn!("memory pipeline: no Oxicode engine, LLM calls will be skipped");
        (None, None)
    };

    let poll_interval = std::time::Duration::from_secs(60);

    let handle = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("memory pipeline runtime");
        rt.block_on(async move {
            let conn = match memory_workers::open_db(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "autonomous memory pipeline: open_db({}) failed: {e}",
                        db_path.display()
                    );
                    return;
                }
            };

            tracing::info!(
                "autonomous memory pipeline: workers started (provider={})",
                if provider.is_some() { "wired" } else { "none" }
            );

            loop {
                let now = chrono::Utc::now().timestamp();

                match memory_workers::run_stage1_iteration(
                    &conn,
                    &sessions_dir,
                    &cwd_str,
                    now,
                    provider.as_ref(),
                    model.as_ref(),
                )
                .await
                {
                    Ok(true) => tracing::debug!("memory pipeline: stage 1 processed a job"),
                    Ok(false) => tracing::trace!("memory pipeline: stage 1 idle"),
                    Err(e) => tracing::warn!("memory pipeline: stage 1 error: {e}"),
                }

                match memory_workers::run_stage2_iteration(
                    &conn,
                    &memory_root,
                    &cwd_str,
                    now,
                    provider.as_ref(),
                    model.as_ref(),
                )
                .await
                {
                    Ok(true) => tracing::info!("memory pipeline: stage 2 consolidated"),
                    Ok(false) => tracing::trace!("memory pipeline: stage 2 idle"),
                    Err(e) => tracing::warn!("memory pipeline: stage 2 error: {e}"),
                }

                tokio::time::sleep(poll_interval).await;
            }
        });
    });
    Some(handle)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn paths_are_consistent() {
        let p = OxicodePaths::from_home("/tmp/oxicode-test");
        assert!(p.auth.starts_with("/tmp/oxicode-test"));
        assert!(p.config.starts_with("/tmp/oxicode-test"));
        assert!(p.sessions.starts_with("/tmp/oxicode-test"));
        assert!(p.skills.starts_with("/tmp/oxicode-test"));
    }

    #[tokio::test]
    async fn build_oxicode_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = OxicodePaths::from_home(tmp.path());
        let oxicode = build_oxicode(&paths, None).await.unwrap();
        let _ = oxicode.ports().state;
    }
}
