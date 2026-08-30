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
    /// Oxi Foundation root. Independent from `home` and resolved
    /// via `$OXI_FOUNDATION_HOME` or `~/.oxi/foundation/v1`. Set
    /// to `None` when the foundation is not installed; the
    /// composition root enters offline mode in that case.
    pub foundation: Option<PathBuf>,
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
            foundation: crate::foundation::foundation_root(),
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
///
/// `hook_runner` registers the user's configured [`HookRunner`](oxicode_sdk::ports::HookRunner)
/// (global + approved-project `[[hooks]]`) on the SDK's port registry. Pass
/// `None` to keep the noop runner (default).
pub async fn build_oxicode(
    paths: &OxicodePaths,
    embedding_provider: Option<Arc<dyn oxicode_sdk::ports::EmbeddingProvider>>,
    hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>,
) -> Result<Oxicode> {
    build_oxicode_with_catalog(
        paths,
        build_catalog_config(paths),
        embedding_provider,
        hook_runner,
    )
    .await
}

/// Build an `Oxicode` engine with a custom catalog config. Useful for
/// tests (e.g. pointing the catalog at a tempdir).
///
/// `hook_runner` follows the same semantics as [`build_oxicode`]: `Some`
/// installs the runner on the SDK's port registry, `None` keeps the noop
/// default.
pub async fn build_oxicode_with_catalog(
    paths: &OxicodePaths,
    catalog_config: CatalogConfig,
    embedding_provider: Option<Arc<dyn oxicode_sdk::ports::EmbeddingProvider>>,
    hook_runner: Option<Arc<dyn oxicode_sdk::ports::HookRunner>>,
) -> Result<Oxicode> {
    ensure_parent(&paths.auth)?;
    ensure_parent(&paths.config)?;
    ensure_parent(&paths.sessions)?;

    // Foundation v1 host: when a foundation installation is present,
    // resolve the profile (explicit id → role → env override →
    // one-time compatibility import), look up the Keychain credential,
    // and register ONLY the selected provider with the resolved key.
    // Provider/model registration is gated on profile + credential
    // validation succeeding — plan §3.b, §3.f. Other built-in
    // providers remain constructable but cannot be invoked because
    // they carry no credentials. The same pattern handles the
    // `OXICODE_PROVIDER`/`OXICODE_MODEL` automation override.
    let foundation_provider: Option<Arc<dyn oxicode_ai::Provider>> = if let Some(froot) = paths
        .foundation
        .clone()
        .or_else(crate::foundation::foundation_root)
    {
        if crate::foundation::foundation_present(&froot) {
            match resolve_and_register_profile(&froot).await {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!(
                        "Foundation v1 profile resolution failed: {e}; \
                             engine will start without a registered provider"
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

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
    if let Some(runner) = hook_runner {
        builder = builder.with_hooks(runner.clone());
    }
    if let Some(provider) = foundation_provider {
        builder = builder.provider_arc("<foundation>", provider);
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
    // Foundation v1 host: `memory://` resolves through the
    // brain-backed handler when the foundation installation is
    // present. When no foundation is present (test fixtures, host
    // without oxibrain yet), the handler falls back to the legacy
    // disk-rooted resolver so pre-Foundation callers continue to
    // work.
    let handler: Arc<dyn oxicode_sdk::ports::ProtocolHandler> =
        if crate::foundation::foundation_present(
            &crate::foundation::foundation_root()
                .unwrap_or_else(|| std::path::PathBuf::from("~/.oxi/foundation/v1")),
        ) {
            let socket = crate::foundation::brain::default_socket_path();
            let brain = Arc::new(crate::foundation::brain::BrainMemoryBackend::new(socket));
            Arc::new(MemoryProtocolHandler::new(brain))
        } else {
            // Legacy disk-rooted fallback. NOT used under the
            // Foundation v1 host — see
            // `resolve_memory_url_legacy` for the deprecation
            // context.
            struct LegacyHandler {
                memory_root: PathBuf,
            }
            #[async_trait::async_trait]
            impl oxicode_sdk::ports::ProtocolHandler for LegacyHandler {
                fn scheme(&self) -> &str {
                    "memory"
                }
                async fn resolve(
                    &self,
                    url: &str,
                    _selector: Option<&str>,
                    _ctx: &oxicode_sdk::ports::ResolveContext,
                ) -> Result<oxicode_sdk::ports::ResolvedUrl, oxicode_sdk::SdkError>
                {
                    let content = crate::internal_urls::memory_handler::resolve_memory_url_legacy(
                        url,
                        &self.memory_root,
                    )
                    .ok_or_else(|| oxicode_sdk::SdkError::PortNotConfigured { port: "memory" })?;
                    let size = content.len();
                    Ok(oxicode_sdk::ports::ResolvedUrl {
                        url: url.to_string(),
                        content,
                        content_type: "text/markdown".to_string(),
                        size: Some(size),
                        source_path: None,
                        notes: vec![],
                        immutable: true,
                    })
                }
            }
            Arc::new(LegacyHandler { memory_root })
        };
    router.register(handler);
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

// ── Memory backend helpers ──────────────────────────────────────────────

/// `true` when a Unix-domain socket file exists at `path` (best-effort:
/// stat only, no connect). This is the "daemon installed" gate for durable
/// memory — the Foundation layout gates profiles/packages, not the memory
/// authority.
pub(crate) fn brain_socket_present(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_socket())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

/// Create a memory backend if memory is enabled in settings.
///
/// Under the Oxi Foundation v1 host, the only durable-memory authority is the
/// oxibrain daemon (plan §5). Local SQLite/Mnemopi/JSON/file-summary
/// fallbacks are explicitly forbidden (§5.h, §6.f): the Foundation host MUST
/// NOT silently run a second durable store.
///
/// Returns a [`crate::foundation::brain::BrainMemoryBackend`] when
/// `memory_enabled` is set and the daemon's socket exists at the canonical
/// path (or `$OXIBRAIN_SOCKET`). The backend is not eagerly connected — the
/// first call attaches and surfaces `degraded` per call when the daemon is
/// unreachable. When the socket is absent, returns `None`: the agent memory
/// tools surface a typed "backend unavailable" result naming the socket and
/// the recovery command (`oxibrain serve`). Code work continues; only
/// durable-memory tool calls fail visibly.
pub fn create_memory_backend(
    settings: &crate::store::settings::Settings,
) -> Option<Arc<dyn oxicode_agent::tools::MemoryBackend>> {
    if !settings.memory_enabled {
        return None;
    }
    let socket = crate::foundation::brain::default_socket_path();
    if brain_socket_present(&socket) {
        let backend = crate::foundation::brain::BrainMemoryBackend::new(socket.clone());
        tracing::info!(
            "durable memory authority is oxibrain at {}",
            socket.display()
        );
        return Some(Arc::new(backend));
    }
    tracing::warn!(
        "memory_enabled but no oxibrain socket at {} — durable-memory tools \
         will return typed unavailable results. Start the daemon with \
         `oxibrain serve` or set OXIBRAIN_SOCKET.",
        socket.display()
    );
    None
}

/// Initial status-bar chip value before the prober's first tick lands.
pub(crate) fn initial_brain_chip(
    settings: &crate::store::settings::Settings,
) -> crate::tui_vt::main_loop::BrainChip {
    use crate::tui_vt::main_loop::BrainChip;
    if !settings.memory_enabled {
        return BrainChip::Off;
    }
    if brain_socket_present(&crate::foundation::brain::default_socket_path()) {
        // Socket present; the immediate first probe will confirm or degrade.
        BrainChip::Degraded
    } else {
        BrainChip::Down
    }
}
#[cfg(test)]
mod memory_backend_tests {
    use super::*;

    /// These tests mutate the process-global `OXIBRAIN_SOCKET` env var;
    /// cargo runs tests on parallel threads, so they must serialize or they
    /// observe each other's sockets (flaky `is_some`/`is_none` failures).
    static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn test_settings() -> crate::store::settings::Settings {
        let mut s = crate::store::settings::Settings::default();
        s.memory_enabled = true;
        s
    }

    /// Run `f` with `OXIBRAIN_SOCKET` set to `value` (removed when `None`),
    /// holding the env lock and cleaning the variable up afterwards.
    #[cfg(unix)]
    fn with_socket_env<T>(value: Option<&std::path::Path>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock();
        unsafe {
            match value {
                Some(p) => std::env::set_var("OXIBRAIN_SOCKET", p),
                None => std::env::remove_var("OXIBRAIN_SOCKET"),
            }
        }
        let out = f();
        unsafe {
            std::env::remove_var("OXIBRAIN_SOCKET");
        }
        out
    }

    /// Bind a real unix socket in a leaked tempdir so the socket file
    /// outlives the helper (the daemon-present gate stats the path).
    #[cfg(unix)]
    fn bind_real_socket() -> std::path::PathBuf {
        let dir = tempfile::TempDir::new().unwrap();
        let sock = dir.path().join("oxibrain.sock");
        std::os::unix::net::UnixListener::bind(&sock).unwrap();
        std::mem::forget(dir);
        sock
    }

    #[cfg(unix)]
    #[test]
    fn brain_backend_returned_when_socket_present() {
        // A real unix socket must pass the daemon-present gate.
        let sock = bind_real_socket();
        with_socket_env(Some(&sock), || {
            let backend = create_memory_backend(&test_settings());
            assert!(backend.is_some(), "socket present ⇒ brain backend");
        });
    }

    #[cfg(unix)]
    #[test]
    fn absent_socket_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("missing.sock");
        with_socket_env(Some(&missing), || {
            let backend = create_memory_backend(&test_settings());
            assert!(
                backend.is_none(),
                "absent socket ⇒ no local durable fallback (plan §5.h)"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn regular_file_is_not_a_socket() {
        let dir = tempfile::TempDir::new().unwrap();
        let fake = dir.path().join("oxibrain.sock");
        std::fs::write(&fake, b"not a socket").unwrap();
        with_socket_env(Some(&fake), || {
            let backend = create_memory_backend(&test_settings());
            assert!(
                backend.is_none(),
                "regular file must not pass the socket gate"
            );
        });
    }

    #[test]
    fn memory_disabled_returns_none() {
        // `memory_enabled` defaults to true (the daemon is the authority);
        // disabling it must yield no backend regardless of socket state.
        let mut s = crate::store::settings::Settings::default();
        assert!(s.memory_enabled, "memory_enabled default flipped to true");
        s.memory_enabled = false;
        assert!(create_memory_backend(&s).is_none());
    }
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

// ── Foundation profile → provider registration ────────────────────────────

/// Resolve a Foundation profile and register the selected provider.
///
/// Precedence (plan §2.c):
///   1. `OXICODE_PROVIDER` + `OXICODE_MODEL` env override.
///   2. Explicit `--profile` / `OXICODE_PROFILE` id.
///   3. Role-compatible Foundation profile.
///   4. One-time compatibility import (gated by `OXICODE_FOUNDATION_MIGRATION=1`).
///
/// The resolved credential is read from the OS Keychain; the
/// provider/model is registered only when profile + credential
/// validation succeeds. Errors are reported but never silently
/// replaced by another remote provider (plan §3.f).
async fn resolve_and_register_profile(
    foundation_root: &Path,
) -> Result<Arc<dyn oxicode_ai::Provider>, crate::foundation::FoundationError> {
    use crate::foundation::profiles::{
        EnvironmentOverride, ResolveInput, read as read_profiles, resolve_profile,
    };

    let profiles_path = foundation_root.join(crate::foundation::files::PROFILES);
    let profiles = read_profiles(&profiles_path)?;
    let explicit_profile = std::env::var("OXICODE_PROFILE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let compat_import_path = foundation_root.join("compatibility.json");
    let compat_import =
        crate::foundation::compat_import::read_compatibility_shim(&compat_import_path)?;

    let env_override = EnvironmentOverride::from_env();

    let resolved = resolve_profile(ResolveInput {
        explicit_profile: explicit_profile.as_deref(),
        explicit_environment_override: env_override.as_ref(),
        requested_role: None,
        foundation_profiles: &profiles,
        compatibility_import: compat_import.as_ref(),
    })?;

    let resolver = crate::foundation::credentials::KeychainCredentialResolver::default();
    let credential = resolver.resolve(&resolved.profile);
    let api_key = match credential {
        crate::foundation::credentials::Credential::Keychain(s)
        | crate::foundation::credentials::Credential::Environment(s) => s,
        crate::foundation::credentials::Credential::Unavailable(e) => {
            return Err(crate::foundation::FoundationError::KeychainUnavailable(
                e.to_string(),
            ));
        }
    };
    let provider_name = resolved.profile.provider.as_str();
    let provider: Arc<dyn oxicode_ai::Provider> = Arc::from(
        oxicode_ai::register_builtins::create_builtin_provider_with_options(
            provider_name,
            Some(&api_key),
            None,
        )
        .ok_or_else(|| {
            crate::foundation::FoundationError::IncompatibleHost(provider_name.to_string())
        })?,
    );

    tracing::info!(
        provider = provider_name,
        model = %resolved.profile.model,
        source = ?resolved.source,
        "Foundation profile resolved with Keychain credential"
    );
    Ok(provider)
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
        let oxicode = build_oxicode(&paths, None, None).await.unwrap();
        let _ = oxicode.ports().state;
    }
}
