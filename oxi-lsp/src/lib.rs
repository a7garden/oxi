//! `oxi-lsp` — Thin LSP protocol adapter.
//!
//! This crate wraps [`async_lsp`] + [`lsp_types`] + [`async_process`]
//! to provide a single [`LspClient`] that owns a language server
//! process and a JSON-RPC correlation loop. **It does not** do:
//!
//! - config discovery / layering (user > project > plugin)
//! - folder-trust gating (project `lsp.json` skipped in untrusted folders)
//! - crash recovery / lifetime restart budget
//! - multi-server lifecycle / extension conflict resolution
//!
//! Those concerns belong in `oxi-cli's` LSP adapter (see
//! `oxi-cli/src/lsp/manager.rs`). Keeping `oxi-lsp` thin lets the
//! adapter own policy without re-implementing the JSON-RPC loop.
//!
//! # Pattern
//!
//! Ported from grok's `LspManager` (see
//! `docs/designs/2026-07-18-stub-completion.md` §2): a single
//! `LspClient` per server, with `diagnostics_ready: Arc<Notify>`
//! gating `drain_diagnostics`, and `lifecycle_id: AtomicU64` marking
//! restart epochs so cached diagnostics from a dead epoch can be
//! evicted.

use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_lsp::router::Router;
use async_lsp::{LanguageServer, MainLoop, ServerSocket};
use async_process::Command;
use dashmap::DashMap;
use futures::AsyncReadExt;
use lsp_types::notification::{Notification, PublishDiagnostics};
use lsp_types::{
    self as types, ClientCapabilities, InitializeParams, ServerCapabilities, Url, WorkspaceFolder,
};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Default timeout for an individual `request → response` RPC.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Crate-wide error type.
#[derive(Debug, Error)]
pub enum LspError {
    #[error("failed to spawn LSP server `{server}`: {source}")]
    SpawnFailed {
        server: String,
        #[source]
        source: io::Error,
    },
    #[error("LSP server `{server}` failed to initialize: {message}")]
    InitFailed { server: String, message: String },
    #[error("LSP `{server}` request `{method}` timed out after {timeout:?}")]
    Timeout {
        server: String,
        method: String,
        timeout: Duration,
    },
    #[error("LSP `{server}` request `{method}` failed: {message}")]
    RequestFailed {
        server: String,
        method: String,
        message: String,
    },
    #[error("LSP `{server}` transport error in `{method}`: {source}")]
    Transport {
        server: String,
        method: String,
        #[source]
        source: async_lsp::Error,
    },
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

/// Per-file `textDocument/publishDiagnostics` snapshot.
#[derive(Debug, Clone)]
pub struct PublishedDiagnostics {
    pub uri: String,
    pub diagnostics: serde_json::Value,
}

/// Process-handle and runtime knobs for one LSP server.
#[derive(Debug, Clone)]
pub struct LspClientConfig {
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
}

impl LspClientConfig {
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

/// One process-spawned language server.
pub struct LspClient {
    server_name: String,
    workspace_root: PathBuf,
    lifecycle_id: Arc<AtomicU64>,
    diagnostics: Arc<DashMap<String, PublishedDiagnostics>>,
    diagnostics_ready: Arc<Notify>,
    server: Option<ServerSocket>,
    main_loop: Option<JoinHandle<async_lsp::Result<()>>>,
    child: Option<async_process::Child>,
    #[allow(dead_code)]
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

        // async-process Child::stdout/stdin implement futures AsyncRead/AsyncWrite.
        let stdout = child.stdout.take().ok_or_else(|| LspError::SpawnFailed {
            server: config.server_name.clone(),
            source: io::Error::other("child stdout unavailable"),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| LspError::SpawnFailed {
            server: config.server_name.clone(),
            source: io::Error::other("child stdin unavailable"),
        })?;
        let stderr = child.stderr.take();

        let diagnostics: Arc<DashMap<String, PublishedDiagnostics>> = Arc::new(DashMap::new());
        let diagnostics_for_handler = diagnostics.clone();
        let server_name_for_handler = config.server_name.clone();

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
            router
        });

        let main_loop_handle =
            tokio::spawn(async move { mainloop.run_buffered(stdout, stdin).await });

        // Forward child stderr to oxi's tracing.
        if let Some(stderr) = stderr {
            let srv = config.server_name.clone();
            tokio::spawn(async move {
                let mut stderr = stderr;
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    match stderr.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            while let Some(idx) = buf.iter().position(|b| *b == b'\n') {
                                let line: Vec<u8> = buf.drain(..=idx).collect();
                                tracing::debug!(server = %srv, stderr = ?line);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        let client = Arc::new(Self {
            server_name: server_name.clone(),
            workspace_root,
            lifecycle_id: Arc::new(AtomicU64::new(1)),
            diagnostics,
            diagnostics_ready: Arc::new(Notify::new()),
            server: Some(server),
            main_loop: Some(main_loop_handle),
            child: Some(child),
            request_timeout: config.request_timeout,
            shutdown_timeout: config.shutdown_timeout,
        });

        client.spawn_diagnostics_watcher();

        client
            .initialize_with_timeout(config.startup_timeout)
            .await?;

        Ok(client)
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn lifecycle_id(&self) -> u64 {
        self.lifecycle_id.load(Ordering::Acquire)
    }

    pub fn bump_lifecycle_id(&self) -> u64 {
        self.lifecycle_id.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub async fn initialize_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<ServerCapabilities, LspError> {
        let mut server = self
            .server
            .as_ref()
            .ok_or_else(|| LspError::shut_down(&self.server_name))?
            .clone();

        let workspace_uri =
            Url::from_file_path(&self.workspace_root).map_err(|_| LspError::InitFailed {
                server: self.server_name.clone(),
                message: "workspace root is not absolute or has no URI form".into(),
            })?;

        let params = InitializeParams {
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: workspace_uri,
                name: self.workspace_root.display().to_string(),
            }]),
            capabilities: ClientCapabilities::default(),
            ..InitializeParams::default()
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
            Ok(Ok(v)) => v,
        };

        server
            .initialized(types::InitializedParams {})
            .map_err(|e| LspError::Transport {
                server: self.server_name.clone(),
                method: "initialized".into(),
                source: e,
            })?;

        Ok(resp.capabilities)
    }

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
            .ok_or_else(|| LspError::shut_down(&self.server_name))?
            .clone();

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
            Ok(Ok(v)) => v,
        };
        Ok(value)
    }

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

    pub async fn drain_diagnostics(&self, timeout: Duration) -> Option<Vec<PublishedDiagnostics>> {
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

    pub fn read_diagnostics(&self, uris: &[String]) -> Vec<PublishedDiagnostics> {
        uris.iter()
            .filter_map(|u| self.diagnostics.get(u).map(|kv| kv.value().clone()))
            .collect()
    }

    pub fn diagnostics_file_count(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn clear_diagnostics(&self) {
        self.diagnostics.clear();
    }

    pub async fn shutdown(&mut self) -> Result<(), LspError> {
        use types::notification::Exit;

        if let Some(server) = self.server.as_ref() {
            let mut server = server.clone();
            let _ = tokio::time::timeout(self.shutdown_timeout, async {
                let _ = server.shutdown(()).await;
                let _ = server.notify::<Exit>(());
            })
            .await;
        }
        self.server = None;

        if let Some(handle) = self.main_loop.as_ref() {
            handle.abort();
            self.main_loop = None;
        }

        let _ = self.child.take();

        Ok(())
    }

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
}

/// Helper: build a `file://` URI from a path.
pub fn uri_for(path: &Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

/// Tracked-document replay helper used by `oxi-cli's` restart monitor.
#[derive(Debug, Default)]
pub struct ReplayState {
    inner: Mutex<std::collections::HashMap<PathBuf, String>>,
}

impl ReplayState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, path: PathBuf, content: String) {
        self.inner.lock().insert(path, content);
    }

    pub fn forget(&self, path: &Path) {
        self.inner.lock().remove(path);
    }

    pub fn snapshot(&self) -> Vec<(PathBuf, String)> {
        self.inner
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

pub use async_lsp::Error as AsyncLspError;

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
