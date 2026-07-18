//! `oxi-lsp` — Thin LSP protocol adapter.
//!
//! This crate wraps [`async_lsp`] + [`lsp_types`] to provide a single
//! [`LspClient`] that owns a language server process and a JSON-RPC
//! correlation loop. **It does not** do:
//!
//! - config discovery / layering (user > project > plugin)
//! - folder-trust gating (project `lsp.json` skipped in untrusted folders)
//! - crash recovery / lifetime restart budget
//! - multi-server lifecycle / extension conflict resolution
//!
//! Those concerns belong in `oxi-cli's` LSP adapter (see
//! `oxi-cli/src/lsp/manager.rs`). Keeping `oxi-lsp` thin lets the adapter
//! own policy without re-implementing the JSON-RPC loop.
//!
//! # Pattern
//!
//! Ported from grok's `LspManager` (see
//! `docs/designs/2026-07-18-stub-completion.md` §2): a single `LspClient`
//! per server, with `diagnostics_ready: Arc<Notify>` gating
//! `drain_diagnostics`, and `lifecycle_id: AtomicU64` marking restart
//! epochs so cached diagnostics from a dead epoch can be evicted on
//! read. `async_lsp`'s `MainLoop` already implements the JSON-RPC
//! framing, so this crate only owns per-server state and exposes typed
//! request helpers on top.

use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_lsp::client_monitor::ClientProcessMonitor;
use async_lsp::router::Router;
use async_lsp::{LanguageClient, LanguageServer, MainLoop, ServerSocket};
use dashmap::DashMap;
use lsp_types as types;
use lsp_types::notification::{Notification, PublishDiagnostics};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Default timeout for an individual `request → response` RPC.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Sentinel error used when the caller asks for an operation after the
/// `ServerSocket` has already been closed (e.g. after `shutdown`).
const SERVICE_STOPPED_SENTINEL: async_lsp::Error = async_lsp::Error::ServiceStopped;

/// Crate-wide error type. Each variant maps to one failure class the
/// manager in `oxi-cli` decides how to react to (retry budget, restart,
/// user-facing error).
#[derive(Debug, Error)]
pub enum LspError {
    /// Could not spawn the configured language server process.
    #[error("failed to spawn LSP server `{server}`: {source}")]
    SpawnFailed {
        server: String,
        #[source]
        source: io::Error,
    },
    /// `initialize` / `initialized` handshake failed (server returned
    /// an error or the socket closed before completion).
    #[error("LSP server `{server}` failed to initialize: {message}")]
    InitFailed { server: String, message: String },
    /// Request timed out before the server responded.
    #[error("LSP `{server}` request `{method}` timed out after {timeout:?}")]
    Timeout {
        server: String,
        method: String,
        timeout: Duration,
    },
    /// A specific server request returned an error.
    #[error("LSP `{server}` request `{method}` failed: {message}")]
    RequestFailed {
        server: String,
        method: String,
        message: String,
    },
    /// Underlying `async_lsp` transport error.
    #[error("LSP `{server}` transport error in `{method}`: {source}")]
    Transport {
        server: String,
        method: String,
        #[source]
        source: async_lsp::Error,
    },
    /// Operation attempted on a closed `ServerSocket`.
    #[error("LSP `{server}` is shut down")]
    ShutDown { server: String },
}

impl LspError {
    fn shut_down(server: &str) -> Self {
        LspError::ShutDown {
            server: server.to_string(),
        }
    }
}

/// Per-file `textDocument/publishDiagnostics` snapshot. The inner JSON
/// mirrors the LSP spec verbatim so callers can render without re-typing
/// the type.
#[derive(Debug, Clone)]
pub struct PublishedDiagnostics {
    /// Document URI as the server reported it.
    pub uri: String,
    /// LSP-versioned `PublishDiagnosticsParams.diagnostics` payload.
    pub diagnostics: serde_json::Value,
}

/// Process-handle and runtime knobs for one LSP server. Cheap to clone
/// (internally `Arc`-wrapped fields only).
#[derive(Debug, Clone)]
pub struct LspClientConfig {
    /// Stable, human-readable name (e.g. `rust-analyzer`).
    pub server_name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// Server-side startup timeout (used by `initialize_with_timeout`).
    pub startup_timeout: Duration,
    /// Default per-request timeout.
    pub request_timeout: Duration,
    /// Graceful shutdown timeout (force-kill after this).
    pub shutdown_timeout: Duration,
}

impl LspClientConfig {
    /// Build a new config with sensible defaults; the manager layer
    /// supplies `startup_timeout` from its lifecycle budget.
    pub fn new(
        server_name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            command: command.into(),
            args,
            startup_timeout: Duration::from_secs(10),
            request_timeout: REQUEST_TIMEOUT,
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

/// One process-spawned language server. Holds the `Child` handle, the
/// async_lsp `ServerSocket`, and shared state for diagnostics +
/// lifecycle.
///
/// Drop the client to drop the child (via `kill_on_drop`); a graceful
/// path is `shutdown()` which sends `shutdown`/`exit` first.
pub struct LspClient {
    server_name: String,
    workspace_root: PathBuf,
    lifecycle_id: Arc<AtomicU64>,
    diagnostics: Arc<DashMap<String, PublishedDiagnostics>>,
    diagnostics_ready: Arc<Notify>,
    server: Option<ServerSocket>,
    main_loop: Option<JoinHandle<async_lsp::Result<()>>>,
    child: Option<Child>,
    request_timeout: Duration,
    shutdown_timeout: Duration,
}

impl std::fmt::Debug for LspClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspClient")
            .field("server_name", &self.server_name)
            .field("workspace_root", &self.workspace_root)
            .field("lifecycle_id", &self.lifecycle_id.load(Ordering::Acquire))
            .field("child_running", &self.child.is_some())
            .finish()
    }
}

impl LspClient {
    /// Spawn a fresh `LspClient`. Returns an error if the process can't
    /// launch or `initialize` fails within `config.startup_timeout`.
    pub async fn start(
        config: LspClientConfig,
        workspace_root: PathBuf,
    ) -> Result<Arc<Self>, LspError> {
        let server_name = config.server_name.clone();

        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|source| LspError::SpawnFailed {
            server: config.server_name.clone(),
            source,
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::SpawnFailed {
                server: config.server_name.clone(),
                source: io::Error::new(io::ErrorKind::Other, "child stdout unavailable"),
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::SpawnFailed {
                server: config.server_name.clone(),
                source: io::Error::new(io::ErrorKind::Other, "child stdin unavailable"),
            })?;
        let stderr = child.stderr.take();

        // Per-file diagnostics map shared with the notification handler.
        let diagnostics: Arc<DashMap<String, PublishedDiagnostics>> = Arc::new(DashMap::new());
        let diagnostics_for_handler = diagnostics.clone();

        let server_name_for_handler = config.server_name.clone();

        // Build the typed LanguageClient (Router wrapping
        // ClientProcessMonitor) inside `new_client`'s closure.
        let (mainloop, server) = MainLoop::new_client(move |_server| {
            let mut router = Router::new(());
            router.notification::<PublishDiagnostics>(move |_st, params| {
                let uri = params.uri.to_string();
                let payload = serde_json::to_value(&params.diagnostics)
                    .unwrap_or(serde_json::Value::Array(vec![]));
                diagnostics_for_handler.insert(
                    uri.clone(),
                    PublishedDiagnostics {
                        uri,
                        diagnostics: payload,
                    },
                );
                tracing::debug!(
                    server = %server_name_for_handler,
                    "publishDiagnostics stored",
                );
                ControlFlow::Continue(())
            });
            ClientProcessMonitor::new(router)
        });

        let main_loop = tokio::spawn(async move {
            // Drive the JSON-RPC framing. If the child exits the future
            // resolves with `Error::Io` or `Error::Eof`; the manager
            // surfaces those as `LspError::ServerExited`-shaped errors.
            mainloop.run_buffered(stdout, stdin).await
        });

        // Forward child stderr to oxi's tracing at debug level so server
        // logs land in `~/.cache/oxi/oxi.log`.
        if let Some(mut stderr) = stderr {
            let srv = config.server_name.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, BufReader};
                let mut reader = BufReader::new(&mut stderr);
                let mut buf = [0u8; 1024];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let line = String::from_utf8_lossy(&buf[..n]);
                            for l in line.lines() {
                                tracing::debug!(server = %srv, stderr = %l);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        let client = Arc::new(Self {
            server_name: server_name.clone(),
            workspace_root: workspace_root.clone(),
            lifecycle_id: Arc::new(AtomicU64::new(1)),
            diagnostics,
            diagnostics_ready: Arc::new(Notify::new()),
            server: Some(server),
            main_loop: Some(main_loop),
            child: Some(child),
            request_timeout: config.request_timeout,
            shutdown_timeout: config.shutdown_timeout,
        });

        // Pulse `diagnostics_ready` whenever the map gains a new entry.
        client.spawn_diagnostics_watcher();

        if let Err(e) = client.initialize_with_timeout(config.startup_timeout).await {
            // Initialization failed — drop the client so the child gets
            // killed via `kill_on_drop`. We swallow the original handle
            // (the spawned `Arc` is only held by us here).
            return Err(e);
        }

        Ok(client)
    }

    /// Server name (e.g. `rust-analyzer`).
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Workspace root passed at start time.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Current lifecycle generation. Bumped on every restart so adapters
    /// can invalidate cached state from a prior epoch.
    pub fn lifecycle_id(&self) -> u64 {
        self.lifecycle_id.load(Ordering::Acquire)
    }

    /// Bump the lifecycle id. Called by `oxi-cli's` restart monitor
    /// immediately after a re-`start` so cached state from a prior
    /// epoch is treated as stale.
    pub fn bump_lifecycle_id(&self) -> u64 {
        self.lifecycle_id.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Wait for `initialize` + `initialized` to complete within `timeout`.
    ///
    /// Returns the server's `ServerCapabilities` so the caller can probe
    /// capabilities (`definitionProvider`, `referencesProvider`, …).
    pub async fn initialize_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<types::ServerCapabilities, LspError> {
        let server = self.server.as_ref().ok_or_else(|| LspError::shut_down(&self.server_name))?;

        let workspace_uri = types::Url::from_file_path(&self.workspace_root).map_err(|_| {
            LspError::InitFailed {
                server: self.server_name.clone(),
                message: "workspace root is not absolute or has no URI form".into(),
            }
        })?;

        let params = types::InitializeParams {
            workspace_folders: Some(vec![types::WorkspaceFolder {
                uri: workspace_uri,
                name: self.workspace_root.display().to_string(),
            }]),
            capabilities: types::ClientCapabilities::default(),
            ..types::InitializeParams::default()
        };

        let init_fut = server.initialize(params);
        let resp = match tokio::time::timeout(timeout, init_fut).await {
            Err(_) => {
                return Err(LspError::Timeout {
                    server: self.server_name.clone(),
                    method: "initialize".into(),
                    timeout,
                });
            }
            Ok(Err(e)) => {
                return Err(LspError::Transport {
                    server: self.server_name.clone(),
                    method: "initialize".into(),
                    source: e,
                });
            }
            Ok(Ok(Err(e))) => {
                return Err(LspError::InitFailed {
                    server: self.server_name.clone(),
                    message: format!(
                        "server returned error: code={}, message={}",
                        u32::from(e.code),
                        e.message
                    ),
                });
            }
            Ok(Ok(Ok(v))) => v,
        };

        // Send the `initialized` notification (required by the spec).
        server
            .initialized(types::InitializedParams {})
            .map_err(|e| LspError::Transport {
                server: self.server_name.clone(),
                method: "initialized".into(),
                source: e,
            })?;

        Ok(resp.capabilities)
    }

    /// Send a typed request to the server with a timeout, returning the
    /// deserialized response. Used by the manager (`oxi-cli`) for
    /// `textDocument/definition`, `…`, `workspace/symbol`, etc.
    pub async fn request<R>(
        &self,
        params: R::Params,
        timeout: Duration,
    ) -> Result<R::Result, LspError>
    where
        R: types::request::Request,
        R::Params: serde::Serialize,
    {
        let server = self
            .server
            .as_ref()
            .ok_or_else(|| LspError::shut_down(&self.server_name))?;

        let method = R::METHOD.to_string();
        let fut = server.request::<R>(params);
        let value = match tokio::time::timeout(timeout, fut).await {
            Err(_) => {
                return Err(LspError::Timeout {
                    server: self.server_name.clone(),
                    method: method.clone(),
                    timeout,
                });
            }
            Ok(Err(e)) => {
                return Err(LspError::Transport {
                    server: self.server_name.clone(),
                    method: method.clone(),
                    source: e,
                });
            }
            Ok(Ok(Err(e))) => {
                return Err(LspError::RequestFailed {
                    server: self.server_name.clone(),
                    method: method.clone(),
                    message: format!("code={}, message={}", u32::from(e.code), e.message),
                });
            }
            Ok(Ok(Ok(v))) => v,
        };
    }

    /// Send a fire-and-forget notification (`workspace/didChangeWatchedFiles`, …).
    pub fn notify<N>(&self, params: N::Params) -> Result<(), LspError>
    where
        N: Notification,
        N::Params: serde::Serialize,
    {
        let server = self
            .server
            .as_ref()
            .ok_or_else(|| LspError::shut_down(&self.server_name))?;
        server.notify::<N>(params).map_err(|e| LspError::Transport {
            server: self.server_name.clone(),
            method: N::METHOD.into(),
            source: e,
        })
    }

    /// Drain any diagnostics published since the last call. Returns
    /// `None` when no notification arrived within `timeout`; otherwise
    /// returns every per-file entry currently in the cache.
    pub async fn drain_diagnostics(
        &self,
        timeout: Duration,
    ) -> Option<Vec<PublishedDiagnostics>> {
        let notified = tokio::time::timeout(timeout, self.diagnostics_ready.notified()).await;
        if notified.is_err() {
            return None;
        }
        let entries: Vec<PublishedDiagnostics> = self
            .diagnostics
            .iter()
            .map(|kv| kv.value().clone())
            .collect();
        if entries.is_empty() {
            None
        } else {
            Some(entries)
        }
    }

    /// Snapshot diagnostics for the given URIs. Empty result means "no
    /// fresh diagnostics for these URIs"; this never blocks.
    pub fn read_diagnostics(&self, uris: &[String]) -> Vec<PublishedDiagnostics> {
        uris.iter()
            .filter_map(|u| self.diagnostics.get(u).map(|kv| kv.value().clone()))
            .collect()
    }

    /// Number of files for which diagnostics have been received since
    /// the client started. Used by the manager for status reporting.
    pub fn diagnostics_file_count(&self) -> usize {
        self.diagnostics.len()
    }

    /// Clear all cached diagnostics. Called when the manager's
    /// lifecycle id bumps (e.g. after a restart) so that stale state
    /// from a prior epoch doesn't leak into the fresh client.
    pub fn clear_diagnostics(&self) {
        self.diagnostics.clear();
    }

    /// Graceful shutdown: `shutdown` then `exit`, force-kill on timeout.
    pub async fn shutdown(&self) -> Result<(), LspError> {
        use types::notification::Exit;

        let graceful = match self.server.as_ref() {
            None => Ok(()),
            Some(server) => {
                let _ = tokio::time::timeout(self.shutdown_timeout, async {
                    let _ = server.shutdown(()).await;
                    let _ = server.notify::<Exit>(());
                })
                .await;
                Ok(())
            }
        };
        graceful?;

        // The main loop will exit on its own once the child closes
        // stdin/stdout (kill_on_drop handles the rest).
        if let Some(handle) = self.main_loop.as_ref() {
            // `handle.abort_handle()`: not stable API in newer tokio.
            // Cheap: just abort the JoinHandle.
            handle.abort();
            let _ = handle.await;
        }

        // Drop the child via the explicit Option so the comment in
        // `kill_on_drop` is observed (the trait is configured on the
        // Command at spawn time, but we also force-take here so the
        // child handle is released before we return).
        drop(self.child.take());

        Ok(())
    }

    /// Polling watcher that pulses `diagnostics_ready` whenever the
    /// diagnostics map gains a new key. 50ms interval matches `omp`
    /// `settleMs(250)` order-of-magnitude — tight enough to feel
    /// responsive but loose enough not to thrash.
    fn spawn_diagnostics_watcher(&self) {
        let diag_map = self.diagnostics.clone();
        let notify = self.diagnostics_ready.clone();
        tokio::spawn(async move {
            let mut last: std::collections::HashSet<String> =
                diag_map.iter().map(|kv| kv.key().clone()).collect();
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let current: std::collections::HashSet<String> =
                    diag_map.iter().map(|kv| kv.key().clone()).collect();
                if current != last {
                    last = current;
                    notify.notify_waiters();
                }
            }
        });
    }

    /// Reserved for tests; the manager calls this to construct an
    /// already-initialized client when re-attaching to a child spawned
    /// externally (debug only — production paths go through `start`).
    #[doc(hidden)]
    pub fn _test_bump_lifecycle(&self) -> u64 {
        self.bump_lifecycle_id()
    }
}

/// Helper: build a `file://` URI from a path. Returns `None` for
/// relative paths (the LSP spec requires absolute URIs).
pub fn uri_for(path: &Path) -> Option<types::Url> {
    types::Url::from_file_path(path).ok()
}

/// Tracked-document replay helper used by `oxi-cli's` restart monitor.
///
/// The manager keeps a `HashMap<PathBuf, String>` of "last known
/// contents" for files that have been opened in this epoch. After a
/// crash + restart, this helper drives the replay sequence of LSP
/// notifications needed to bring the new client to parity with the
/// prior one.
#[derive(Debug, Default)]
pub struct ReplayState {
    inner: Mutex<std::collections::HashMap<PathBuf, String>>,
}

impl ReplayState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `content` for `path`. Overwrites any prior entry.
    pub fn record(&self, path: PathBuf, content: String) {
        self.inner.lock().insert(path, content);
    }

    /// Forget `path`. Called after the manager confirms the server
    /// accepted the `didClose`.
    pub fn forget(&self, path: &Path) {
        self.inner.lock().remove(path);
    }

    /// Snapshot all currently-tracked (path, content) pairs.
    pub fn snapshot(&self) -> Vec<(PathBuf, String)> {
        self.inner
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Number of files tracked.
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// True when no files are tracked.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

// Re-export for `oxi-cli`'s adapter.
pub use async_lsp::Error as AsyncLspError;

/// Compile-time guard — pulls the sentinel error variant into a non-`let`
/// binding so unused-import lints don't fire when this crate's lib is
/// built without exercising the shutdown path (e.g. in tests).
#[doc(hidden)]
pub const _SERVICE_STOPPED_SENTINEL: async_lsp::Error = SERVICE_STOPPED_SENTINEL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_timeout_constant_is_sane() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(15));
    }

    #[test]
    fn uri_for_absolute_path() {
        let uri = uri_for(Path::new("/tmp/foo.rs")).unwrap();
        assert!(uri.to_string().starts_with("file:///"));
    }

    #[test]
    fn uri_for_relative_path_is_none() {
        assert!(uri_for(Path::new("foo.rs")).is_none());
    }

    #[test]
    fn config_defaults_match_spec() {
        let cfg = LspClientConfig::new("rust-analyzer", "rust-analyzer", vec![]);
        assert_eq!(cfg.server_name, "rust-analyzer");
        assert_eq!(cfg.startup_timeout, Duration::from_secs(10));
        assert_eq!(cfg.request_timeout, REQUEST_TIMEOUT);
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(5));
    }

    #[test]
    fn replay_state_records_and_forgets() {
        let st = ReplayState::new();
        st.record(PathBuf::from("/tmp/foo.rs"), "fn main() {}".into());
        let snap = st.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, PathBuf::from("/tmp/foo.rs"));
        st.forget(Path::new("/tmp/foo.rs"));
        assert!(st.is_empty());
        assert_eq!(st.len(), 0);
    }

    #[test]
    fn error_shut_down_carries_server_name() {
        let e = LspError::shut_down("rust-analyzer");
        assert!(matches!(e, LspError::ShutDown { ref server } if server == "rust-analyzer"));
    }
}
