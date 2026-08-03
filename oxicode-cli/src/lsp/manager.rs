//! LSP manager — multi-server lifecycle and config layering.
//!
//! Owns one [`oxicode_lsp::LspClient`] per language server. The manager
//! routes file-based requests to the right server based on file
//! extension and exposes a unified surface via [`super::CliLspProvider`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oxicode_lsp::{LspClient, LspClientConfig, LspError};
use parking_lot::RwLock;
use tracing::info;

/// Configuration for a single language server.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    /// Display name (e.g. `"rust-analyzer"`).
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// File extensions this server owns (without leading dot).
    /// e.g. `["rs"]` for rust-analyzer.
    pub extensions: Vec<String>,
}

impl LspServerConfig {
    /// Construct a config for a single-extension server.
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        extension: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            extensions: vec![extension.into()],
        }
    }

    /// Construct a config covering multiple extensions.
    pub fn multi(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        extensions: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            extensions,
        }
    }

    fn matches(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| self.extensions.iter().any(|x| x == ext))
            .unwrap_or(false)
    }
}

/// Default server table: rust-analyzer for `.rs` files when on PATH.
///
/// Returns an empty vec when `rust-analyzer` is not installed, so
/// callers can unconditionally feed the result to [`LspManager::new`]
/// and gracefully get "no servers configured" behavior.
pub fn default_servers() -> Vec<LspServerConfig> {
    let mut out = Vec::new();
    if which("rust-analyzer").is_some() {
        out.push(LspServerConfig::new(
            "rust-analyzer",
            "rust-analyzer",
            vec![],
            "rs",
        ));
    }
    out
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

/// Multi-server LSP manager.
///
/// Holds zero or more [`LspServerConfig`]s plus the lazily-spawned
/// [`LspClient`] for each. Thread-safe via `parking_lot::RwLock`.
pub struct LspManager {
    /// Static server config table (set at construction time).
    servers: Vec<LspServerConfig>,
    /// Live clients keyed by server name. Populated lazily on first
    /// request that routes to that server.
    clients: Arc<RwLock<HashMap<String, Arc<LspClient>>>>,
    /// Workspace root passed to every spawned server.
    workspace_root: PathBuf,
}

impl std::fmt::Debug for LspManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspManager")
            .field("servers", &self.servers)
            .field("workspace_root", &self.workspace_root)
            .field(
                "active_clients",
                &self.clients.read().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl LspManager {
    /// Construct with the given server table and workspace root.
    pub fn new(servers: Vec<LspServerConfig>, workspace_root: PathBuf) -> Self {
        Self {
            servers,
            clients: Arc::new(RwLock::new(HashMap::new())),
            workspace_root,
        }
    }

    /// Construct using [`default_servers()`] for the configured
    /// workspace root.
    pub fn with_defaults(workspace_root: PathBuf) -> Self {
        Self::new(default_servers(), workspace_root)
    }

    /// Number of configured (static) servers — does not reflect
    /// lazy-spawned live clients.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Whether any configured server handles this file extension.
    pub fn handles_path(&self, path: &Path) -> bool {
        self.servers.iter().any(|s| s.matches(path))
    }

    /// Borrow the workspace root.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Look up the server name owning `path`, if any.
    fn server_for_path(&self, path: &Path) -> Option<&LspServerConfig> {
        self.servers.iter().find(|s| s.matches(path))
    }

    /// Get-or-spawn the live client for `path`'s owning server.
    ///
    /// Returns `None` when no server is configured for `path`'s
    /// extension. Returns `Err` when the server fails to spawn or
    /// initialize.
    pub async fn client_for_path(&self, path: &Path) -> Result<Option<Arc<LspClient>>, LspError> {
        let Some(config) = self.server_for_path(path) else {
            return Ok(None);
        };
        let name = config.name.clone();

        // Fast path: already running.
        if let Some(client) = self.clients.read().get(&name).cloned() {
            return Ok(Some(client));
        }

        // Spawn.
        let lsp_config = LspClientConfig::new(
            config.name.clone(),
            config.command.clone(),
            config.args.clone(),
        );
        let client = LspClient::start(lsp_config, self.workspace_root.clone()).await?;
        info!(server = %name, "LSP server started");
        self.clients.write().insert(name, client.clone());
        Ok(Some(client))
    }

    /// Look up the client by server name (must already be running).
    pub fn running_client(&self, name: &str) -> Option<Arc<LspClient>> {
        self.clients.read().get(name).cloned()
    }

    /// Iterate live clients.
    pub fn live_clients(&self) -> Vec<(String, Arc<LspClient>)> {
        self.clients
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Best-effort shutdown of every live client.
    pub async fn shutdown_all(&self) {
        let clients: Vec<(String, Arc<LspClient>)> = self.clients.write().drain().collect();
        for (name, client) in clients {
            // `LspClient::shutdown` takes `&mut self`, but we hold the
            // client through `Arc`. Drop the strong ref to get exclusive
            // ownership via `Arc::try_unwrap`; if other handles still
            // hold it (rare during teardown), skip the graceful
            // shutdown — `kill_on_drop` on the child still reaps the
            // process when the last ref drops.
            if let Ok(mut exclusive) = Arc::try_unwrap(client) {
                let _ = exclusive.shutdown().await;
                tracing::debug!(server = %name, "LSP server shut down");
            } else {
                tracing::debug!(server = %name, "LSP server has external refs; skipping graceful shutdown");
            }
        }
    }

    /// Diagnostic drain timeout for `drain_diagnostics`. 50ms is
    /// short enough to not stall the agent loop but long enough to
    /// catch a fresh `publishDiagnostics` after a write.
    pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_matches_by_extension() {
        let cfg = LspServerConfig::new("rust-analyzer", "rust-analyzer", vec![], "rs");
        assert!(cfg.matches(Path::new("src/main.rs")));
        assert!(!cfg.matches(Path::new("src/main.ts")));
        assert!(!cfg.matches(Path::new("README")));
    }

    #[test]
    fn manager_handles_path() {
        let mgr = LspManager::new(
            vec![LspServerConfig::new(
                "rust-analyzer",
                "rust-analyzer",
                vec![],
                "rs",
            )],
            PathBuf::from("."),
        );
        assert!(mgr.handles_path(Path::new("foo.rs")));
        assert!(!mgr.handles_path(Path::new("foo.ts")));
    }

    #[test]
    fn manager_no_servers_returns_zero() {
        let mgr = LspManager::new(vec![], PathBuf::from("."));
        assert_eq!(mgr.server_count(), 0);
        assert!(!mgr.handles_path(Path::new("anything.rs")));
    }

    #[test]
    fn manager_server_for_path_returns_owner() {
        let mgr = LspManager::new(
            vec![
                LspServerConfig::new("rust-analyzer", "rust-analyzer", vec![], "rs"),
                LspServerConfig::new("tsserver", "typescript-language-server", vec![], "ts"),
            ],
            PathBuf::from("."),
        );
        let p = Path::new("foo.ts");
        let owner = mgr.server_for_path(p).unwrap();
        assert_eq!(owner.name, "tsserver");
    }
}
