//! Brain (oxibrain) memory backend.
//!
//! `BrainMemoryBackend` is the **only** durable-memory authority under the
//! Oxi Foundation v1 host. The legacy local memory backends
//! (`memory_sqlite`, `memory_summary`, `memory_mnemopi`, `memory_workers`,
//! `mnemopi`) stay compilable during the migration window but are no
//! longer durable: they are read-only mirrors, never write targets.
//!
//! ## Wire protocol
//!
//! oxibrain exposes its `memory.*` tool surface over a Unix-domain socket
//! via the JSON-RPC client in `oxibrain-client`. The backend translates
//! every `MemoryBackend` method into one oxibrain tool call:
//!
//! | `MemoryBackend` method | oxibrain tool          | args shape                            |
//! |---|---|---|
//! | `put`                  | `memory.put`           | `{"content": ..., "kind": ..., "subject": ...}` |
//! | `search`               | `memory.search`        | `{"query": ..., "k": N}`              |
//! | `list`                 | `memory.list`          | `{"subject": ...}`                    |
//! | `delete`               | `memory.delete`        | `{"id": ...}`                         |
//!
//! ## Degraded mode
//!
//! When the daemon is unreachable, every mutation returns
//! `ToolError(String)` carrying `"backend unavailable: oxibrain daemon unreachable"`.
//! The local file store is **never** consulted as a fallback, because doing
//! so would silently duplicate memory across two authorities and break
//! the Foundation contract. Tools surface `degraded` to the user instead
//! of pretending the store succeeded.
//!
//! ## Unix-only
//!
//! oxibrain-client uses Unix-domain sockets. On non-Unix targets this
//! module compiles to a stub that returns `BackendUnavailable` for every
//! call. The same `memory_info` constant is used in both targets so the
//! TUI's health banner reads "degraded" identically.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde_json::json;
use tokio::sync::Mutex;

use oxicode_agent::tools::{MemoryBackend, MemoryItem, ToolError};

// ───────────────────────────────────────────────────────────────────────────
// Health state
// ───────────────────────────────────────────────────────────────────────────

/// Health of the Brain connection. Surfaced via `memory_info` so the TUI
/// health banner reports the state without leaking the underlying
/// transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainHealth {
    /// Connected to the daemon and ready to serve requests.
    Connected,
    /// Daemon reachable but last call failed. Retries on next mutation.
    Degraded,
    /// Last `connect()` failed. Backend is in `Unavailable` mode.
    Unavailable,
}

impl BrainHealth {
    pub fn info(self) -> &'static str {
        match self {
            BrainHealth::Connected => "ok: oxibrain daemon connected",
            BrainHealth::Degraded => "degraded: oxibrain daemon unreachable",
            BrainHealth::Unavailable => "degraded: oxibrain daemon unreachable",
        }
    }
}

const HEALTH_CONNECTED: u8 = 0;
const HEALTH_DEGRADED: u8 = 1;
const HEALTH_UNAVAILABLE: u8 = 2;
fn encode_health(h: BrainHealth) -> u8 {
    match h {
        BrainHealth::Connected => HEALTH_CONNECTED,
        BrainHealth::Degraded => HEALTH_DEGRADED,
        BrainHealth::Unavailable => HEALTH_UNAVAILABLE,
    }
}
fn decode_health(b: u8) -> BrainHealth {
    match b {
        HEALTH_CONNECTED => BrainHealth::Connected,
        HEALTH_DEGRADED => BrainHealth::Degraded,
        _ => BrainHealth::Unavailable,
    }
}

/// Migration-specific error type. Distinguishes the cases the
/// migration core cares about: backend offline, write failure,
/// runtime failure.
#[derive(Debug, Clone)]
pub enum MigrationError {
    /// Brain daemon is known to be unreachable (handshake failed).
    BackendOffline,
    /// The write returned a `ToolError` (= `String`).
    Backend(String),
    /// The migration runtime failed to build or execute.
    Runtime(String),
    /// The resumable checkpoint could not be written.
    Checkpoint(String),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::BackendOffline => f.write_str("brain daemon offline"),
            MigrationError::Backend(e) => write!(f, "brain write failed: {e}"),
            MigrationError::Runtime(e) => write!(f, "migration runtime: {e}"),
            MigrationError::Checkpoint(e) => write!(f, "checkpoint write failed: {e}"),
        }
    }
}

impl std::error::Error for MigrationError {}
// ───────────────────────────────────────────────────────────────────────────
// Backend
// ───────────────────────────────────────────────────────────────────────────

/// Default scope identifier passed to oxibrain when one is not provided.
/// oxicode uses the project working directory when known; the fallback
/// is the literal `"default"` so the daemon can route to a project
/// bucket.
pub const DEFAULT_BRAIN_SCOPE: &str = "default";

/// Brain memory backend. The `Arc<Mutex<Option<…>>>` wrapper yields
/// interior mutability on the optional client while keeping the
/// `MemoryBackend` trait object signature (`Arc<…>`, no `&mut`).
pub struct BrainMemoryBackend {
    socket_path: std::path::PathBuf,
    client: Arc<Mutex<Option<oxibrain_client::BrainClient>>>,
    health: Arc<AtomicU8>,
    scope: String,
}

impl std::fmt::Debug for BrainMemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrainMemoryBackend")
            .field("socket_path", &self.socket_path)
            .field("scope", &self.scope)
            .field(
                "health",
                &decode_health(self.health.load(Ordering::SeqCst)).info(),
            )
            .finish()
    }
}

impl BrainMemoryBackend {
    /// Build a backend pointing at the given socket path. The client
    /// is not eagerly connected; the first call attempts to attach.
    pub fn new(socket_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            client: Arc::new(Mutex::new(None)),
            health: Arc::new(AtomicU8::new(HEALTH_UNAVAILABLE)),
            scope: DEFAULT_BRAIN_SCOPE.to_string(),
        }
    }

    /// Set the default scope. Tools that do not pass one explicitly use
    /// this value.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    /// Current health. Read by the TUI health banner.
    pub fn health(&self) -> BrainHealth {
        decode_health(self.health.load(Ordering::SeqCst))
    }

    /// Number of times the connection is currently held. Always 0 or 1
    /// in production; useful for tests that assert reconnect logic.
    pub async fn connected(&self) -> bool {
        self.client.lock().await.is_some()
    }

    /// Synchronous helper that drives the `MemoryBackend::put` trait
    /// method on a small current-thread runtime. Used by the
    /// `migrate` flow and by callers that don't already run inside
    /// a tokio executor.
    pub fn put_sync(&self, content: &str, kind: &str, subject: &str) -> Result<String, ToolError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .map_err(|e| format!("backend unavailable: tokio runtime: {e}"))?;
        let future = <Self as MemoryBackend>::put(self, content, kind, subject);
        rt.block_on(future)
    }

    async fn ensure_connected(&self) -> Result<(), ToolError> {
        let mut guard = self.client.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        match oxibrain_client::BrainClient::connect(&self.socket_path).await {
            Ok(client) => {
                *guard = Some(client);
                self.health.store(HEALTH_CONNECTED, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                self.health.store(HEALTH_UNAVAILABLE, Ordering::SeqCst);
                Err(format!(
                    "backend unavailable: oxibrain daemon unreachable at {}: {e}",
                    self.socket_path.display()
                ))
            }
        }
    }

    /// Run `f` against the connected client. On a connection-level
    /// failure, clear the cached client and mark the backend as
    /// `Unavailable` so the next call retries the handshake.
    async fn with_client<R>(
        &self,
        f: impl FnOnce(
            &mut oxibrain_client::BrainClient,
        ) -> futures::future::BoxFuture<'_, anyhow::Result<R>>,
    ) -> Result<R, ToolError> {
        self.ensure_connected().await?;
        let mut guard = self.client.lock().await;
        let client = guard.as_mut().ok_or_else(|| {
            "backend unavailable: oxibrain client missing after handshake".to_string()
        })?;
        match f(client).await {
            Ok(v) => {
                self.health.store(HEALTH_CONNECTED, Ordering::SeqCst);
                Ok(v)
            }
            Err(e) => {
                self.health.store(HEALTH_DEGRADED, Ordering::SeqCst);
                *guard = None;
                Err(format!("backend unavailable: oxibrain call failed: {e}"))
            }
        }
    }
}

impl MemoryBackend for BrainMemoryBackend {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let content = content.to_string();
            let kind = kind.to_string();
            let subject = subject.to_string();
            let scope = self.scope.clone();
            let args = json!({
                "content": content,
                "kind": kind,
                "subject": subject,
                "scope": scope,
            });
            let raw = self
                .with_client(|c| Box::pin(async move { c.call_tool("memory.put", args).await }))
                .await?;
            // Response is the new ID; if the daemon returns a struct,
            // extract `id`. Otherwise treat the raw response as the ID.
            let id = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| {
                    v.get("id")
                        .and_then(|i| i.as_str().map(|s| s.to_string()))
                        .or_else(|| v.get("id").and_then(|i| i.as_u64().map(|n| n.to_string())))
                })
                .unwrap_or_else(|| raw.trim().to_string());
            Ok(id)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let query = query.to_string();
            let scope = self.scope.clone();
            let args = json!({
                "query": query,
                "k": k,
                "scope": scope,
            });
            let raw = self
                .with_client(|c| Box::pin(async move { c.call_tool("memory.search", args).await }))
                .await?;
            parse_memory_items(&raw)
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let subject = subject.to_string();
            let scope = self.scope.clone();
            let args = json!({
                "subject": subject,
                "scope": scope,
            });
            let raw = self
                .with_client(|c| Box::pin(async move { c.call_tool("memory.list", args).await }))
                .await?;
            parse_memory_items(&raw)
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let id = id.to_string();
            let args = json!({ "id": id });
            let _ = self
                .with_client(|c| Box::pin(async move { c.call_tool("memory.delete", args).await }))
                .await?;
            Ok(())
        })
    }

    fn memory_info(&self) -> Option<String> {
        Some(self.health().info().to_string())
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Parsing helpers
// ───────────────────────────────────────────────────────────────────────────

/// Parse the JSON-RPC response from the daemon into a `Vec<MemoryItem>`.
/// Accepts both the bare `[ {...} ]` shape and the wrapped `{ "items": [...] }`
/// shape (the legacy oxibrain response style).
fn parse_memory_items(raw: &str) -> Result<Vec<MemoryItem>, ToolError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("backend unavailable: malformed memory response: {e}"))?;
    let items = value
        .get("items")
        .and_then(|i| i.as_array())
        .or_else(|| value.as_array())
        .ok_or_else(|| "backend unavailable: memory response missing 'items' array".to_string())?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let id = match item.get("id") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        };
        let kind = item
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("fact")
            .to_string();
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let subject = item
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(MemoryItem {
            id,
            kind,
            content,
            subject,
        });
    }
    Ok(out)
}

// ───────────────────────────────────────────────────────────────────────────
// Factory used by the composition root
// ───────────────────────────────────────────────────────────────────────────

/// Resolves the default socket path for the oxibrain daemon. Honors
/// `OXIBRAIN_SOCKET` if set; otherwise `$XDG_RUNTIME_DIR/oxibrain.sock`
/// (Linux) or `~/.oxi/run/oxibrain.sock` (macOS).
pub fn default_socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("OXIBRAIN_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(runtime).join("oxibrain.sock");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".oxi").join("run").join("oxibrain.sock");
    }
    std::path::PathBuf::from("/tmp/oxibrain.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_info_strings_match_spec() {
        assert_eq!(
            BrainHealth::Connected.info(),
            "ok: oxibrain daemon connected"
        );
        assert_eq!(
            BrainHealth::Degraded.info(),
            "degraded: oxibrain daemon unreachable"
        );
        assert_eq!(
            BrainHealth::Unavailable.info(),
            "degraded: oxibrain daemon unreachable"
        );
    }

    #[test]
    fn health_round_trip() {
        for h in [
            BrainHealth::Connected,
            BrainHealth::Degraded,
            BrainHealth::Unavailable,
        ] {
            assert_eq!(decode_health(encode_health(h)), h);
        }
    }

    #[test]
    fn parse_memory_items_accepts_bare_array() {
        let raw = r#"[{"id":"a","kind":"fact","content":"hello","subject":"proj"}]"#;
        let items = parse_memory_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "a");
        assert_eq!(items[0].kind, "fact");
        assert_eq!(items[0].content, "hello");
        assert_eq!(items[0].subject, "proj");
    }

    #[test]
    fn parse_memory_items_accepts_wrapped_array() {
        let raw = r#"{"items":[{"id":"a","kind":"fact","content":"hello","subject":"proj"}]}"#;
        let items = parse_memory_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "a");
    }

    #[test]
    fn parse_memory_items_rejects_missing_items() {
        let raw = r#"{"unexpected":"shape"}"#;
        let err = parse_memory_items(raw).unwrap_err();
        assert!(err.starts_with("backend unavailable"));
    }

    #[test]
    fn backend_starts_unavailable() {
        let backend = BrainMemoryBackend::new("/tmp/does-not-exist.sock");
        assert_eq!(backend.health(), BrainHealth::Unavailable);
        assert_eq!(
            backend.memory_info().as_deref(),
            Some("degraded: oxibrain daemon unreachable")
        );
    }

    #[test]
    fn backend_with_scope_keeps_scope() {
        let backend = BrainMemoryBackend::new("/tmp/x.sock").with_scope("oxicode/main");
        assert_eq!(backend.scope, "oxicode/main");
    }
}
