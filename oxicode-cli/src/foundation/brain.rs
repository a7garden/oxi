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
//! oxibrain exposes its MCP tool surface over a Unix-domain socket via the
//! JSON-RPC client in `oxibrain-client`. The daemon's fifteen tools are
//! `search, recall, brief, navigate, ingest, declare, why, contradictions,
//! stats, traverse, review_merges, remember, retract, merge_entities, redact`.
//! The backend maps every `MemoryBackend` method onto that real surface:
//!
//! | `MemoryBackend` method | oxibrain tool | args                          |
//! |---|---|---|
//! | `put`                  | `remember`    | `{"content": ..., "space": ..., "source_path": "oxicode/<kind>/<subject>"}` |
//! | `search`               | `search`      | `{"query": ..., "space": ..., "limit": N}`      |
//! | `list`                 | `search`      | `{"query": <subject>, "space": ..., "limit": 50}` |
//! | `delete`               | `retract`     | `{"statement_id": ...}` (auditable retraction)  |
//!
//! `remember` = `ingest_note` + synchronous extraction on the daemon side, so
//! every `put` becomes a provenance-bearing episode. `search` returns entity
//! hits (`entity_id`, `entity_surface`, `entity_type`, `score`, `snippet`),
//! mapped into `MemoryItem`. Deletion is a statement-scoped retraction; ids
//! that are not statement ids surface a typed error steering toward `redact`
//! — never a silent local removal.
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

/// Default space passed to oxibrain when one is not provided. `personal` is
/// the daemon's conventional default space; override with [`Self::with_scope`]
/// (e.g. to route a project to its own bucket).
pub const DEFAULT_BRAIN_SCOPE: &str = "personal";

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

    /// Probe daemon liveness with a `ping`. Cheaper than a full tool call;
    /// used by the TUI health prober. Updates the cached health.
    pub async fn ping(&self) -> Result<(), ToolError> {
        self.with_client(|c| Box::pin(async move { c.ping().await }))
            .await
    }

    /// Space statistics from the daemon's `stats` tool
    /// (`episodes`, `entities`, `statements`, `contradictions`). Returned as
    /// the raw parsed JSON so callers render without a typed mirror of
    /// daemon fields.
    pub async fn stats(&self) -> Result<serde_json::Value, ToolError> {
        let space = self.scope.clone();
        self.with_client(|c| Box::pin(async move { c.stats(&space).await }))
            .await
    }

    /// Synchronous `stats` on a small current-thread runtime — same pattern
    /// as [`Self::put_sync`]. Used by the TUI `/memory` command.
    pub fn stats_sync(&self) -> Result<serde_json::Value, ToolError> {
        block_on_sync(self.stats())
    }

    /// Synchronous `search` wrapper for callers outside a tokio executor.
    pub fn search_sync(&self, query: &str, k: usize) -> Result<Vec<MemoryItem>, ToolError> {
        block_on_sync(<Self as MemoryBackend>::search(self, query, k))
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
        block_on_sync(<Self as MemoryBackend>::put(self, content, kind, subject))
    }

    /// Synchronous `ping` — used by flows that need a live health probe
    /// without an executor of their own.
    pub fn ping_sync(&self) -> Result<(), ToolError> {
        block_on_sync(self.ping())
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

/// Drive `fut` to completion on the current thread. Inside a tokio runtime
/// (e.g. the CLI's async `main`), park the worker first via
/// `block_in_place` — a nested `Runtime::block_on` would panic. Outside a
/// runtime, build a small current-thread executor.
fn block_on_sync<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .expect("build current-thread tokio runtime");
            rt.block_on(fut)
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
            let args = json!({
                "content": content,
                "space": self.scope,
                "source_path": format!("oxicode/{kind}/{subject}"),
                // `extract: false` keeps this call off the daemon's MCP
                // sampling path (§12.3): a sampling round-trip is a
                // server→client request the 0.2.0 client cannot answer, so
                // realtime extraction would stall every `put` by the
                // daemon's 120s sampling timeout. The note is durable as an
                // episode immediately; `recall` surfaces it via the
                // recent-episodes layer. Revisit with a sampling-capable
                // client (oxibrain-client 0.3).
                "extract": false,
            });
            let raw = self
                .with_client(|c| Box::pin(async move { c.call_tool("ingest", args).await }))
                .await?;
            // `ingest` answers "Ingested as episode: {id}" — keep the id so
            // a later `delete` can redact the exact episode.
            let id = raw
                .split_once("episode:")
                .map(|(_, tail)| tail.trim().to_string())
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
            // `recall` assembles a context bundle (episodes + statements +
            // entities); the `search` tool only returns extracted entity
            // hits, which misses unextracted notes entirely.
            let args = json!({
                "query": query,
                "space": self.scope,
                "token_budget": 4000,
            });
            let _ = k;
            let raw = self
                .with_client(|c| Box::pin(async move { c.call_tool("recall", args).await }))
                .await?;
            parse_memory_items(&raw)
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // `list(subject)` is a recall seeded with the subject so
            // boot-recall stays keyword-scoped; the daemon has no
            // enumerate op.
            let args = json!({
                "query": subject,
                "space": self.scope,
                "token_budget": 2000,
            });
            let raw = self
                .with_client(|c| Box::pin(async move { c.call_tool("recall", args).await }))
                .await?;
            parse_memory_items(&raw)
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move {
            if id.trim().is_empty() {
                return Err("backend unavailable: brain delete requires an episode id \
                     (a `put` return value or a recall provenance id)"
                    .to_string());
            }
            // Episodes are removed via `redact` (destructive, audited).
            // `retract` only withdraws statements produced by extraction.
            let args = json!({
                "target_kind": "episode",
                "target_id": id,
                "space": self.scope,
            });
            let _ = self
                .with_client(|c| Box::pin(async move { c.call_tool("redact", args).await }))
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

/// Parse a `recall` response into `Vec<MemoryItem>`. The daemon assembles a
/// context bundle `{"layers": [{"kind", "text", "provenance", …}]}`; each
/// layer's `text` may hold multiple lines (the `recent_episodes` layer packs
/// one line per episode, with `provenance` ids aligned by line). Map every
/// non-empty line to one `MemoryItem`, carrying the aligned provenance id
/// when the counts match so `delete` can redact the exact episode.
fn parse_memory_items(raw: &str) -> Result<Vec<MemoryItem>, ToolError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("backend unavailable: malformed memory response: {e}"))?;
    let layers = value
        .get("layers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "backend unavailable: unrecognized memory response shape".to_string())?;
    let mut out = Vec::new();
    for layer in layers {
        let kind = layer
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("layer");
        let text = layer.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let provenance: Vec<&str> = layer
            .get("provenance")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|p| p.as_str()).collect())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        for (i, line) in lines.iter().enumerate() {
            let id = provenance.get(i).map(|p| p.to_string()).unwrap_or_default();
            out.push(MemoryItem {
                id,
                kind: kind.to_string(),
                content: line.trim().to_string(),
                subject: String::new(),
            });
        }
    }
    Ok(out)
}

// ───────────────────────────────────────────────────────────────────────────
// Factory used by the composition root
// ───────────────────────────────────────────────────────────────────────────

/// Resolves the default socket path for the oxibrain daemon. Canonical per
/// the Foundation discovery contract (mirror of `oxibrain-client`'s
/// `default_socket_path`, which ships in 0.3.x; we pin 0.2 so the
/// resolution lives here): `$OXIBRAIN_SOCKET` if set, else
/// `$HOME/.oxi/brain/oxibrain.sock`. Never creates directories.
pub fn default_socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("OXIBRAIN_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".oxi").join("brain").join("oxibrain.sock");
    }
    std::path::PathBuf::from(".oxi/brain/oxibrain.sock")
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
    fn parse_memory_items_maps_recall_layers() {
        let raw = r#"{"layers":[
            {"kind":"recent_episodes",
             "text":"first note\nsecond note\n",
             "provenance":["ep-1","ep-2"]},
            {"kind":"statements","text":"oxicode prefers Korean prose"}
        ]}"#;
        let items = parse_memory_items(raw).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "ep-1");
        assert_eq!(items[0].kind, "recent_episodes");
        assert_eq!(items[0].content, "first note");
        assert_eq!(items[1].id, "ep-2");
        assert_eq!(items[1].content, "second note");
        // Layer without provenance → empty id, text kept whole.
        assert_eq!(items[2].id, "");
        assert_eq!(items[2].kind, "statements");
        assert_eq!(items[2].content, "oxicode prefers Korean prose");
    }

    #[test]
    fn parse_memory_items_skips_blank_lines() {
        let raw = r#"{"layers":[{"kind":"recent_episodes",
             "text":"\n  \nonly line\n","provenance":["ep-9"]}]}"#;
        let items = parse_memory_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "ep-9");
        assert_eq!(items[0].content, "only line");
    }

    #[test]
    fn parse_memory_items_empty_layers_is_ok() {
        let items = parse_memory_items(r#"{"layers":[]}"#).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn parse_memory_items_rejects_unknown_wrapper() {
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
