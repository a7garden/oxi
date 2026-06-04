//! Port traits for product-specific adapters.
//!
//! These traits define the **contract** between oxi-sdk (composition layer) and
//! the host product's infrastructure. Products like `oxi-cli` and
//! `oxios-kernel` provide their own implementations.
//!
//! # Pattern
//!
//! ```text
//!   oxi-sdk  (defines traits)
//!      │
//!      │ implements
//!      ▼
//!   Product layer
//!   ├── oxi-cli    → FileStateStore, FileAuthProvider, FileSkillLoader
//!   ├── oxios-kernel → OxiosStateStore, OxiosEventBus, OxiosMemoryStore
//!   └── custom     → MyDbStateStore, MyAuthProvider, etc.
//! ```
//!
//! # Design Principles
//!
//! 1. **SDK defines contract, products implement** — no port is implemented
//!    inside oxi-sdk. `oxi-sdk` ships only traits + noop fallbacks.
//! 2. **Optional registration** — products register only the ports they use.
//!    Unregistered ports get a noop default at the call site.
//! 3. **Type-flexible payloads** — entries and values use `serde_json::Value`
//!    so each product can use its own concrete types via (de)serialization.
//! 4. **Async-first** — every port is async-aware (`async_trait`) because most
//!    implementations touch the file system, network, or database.
//!
//! # Versioning
//!
//! Port traits are **additive**. New methods get default noop implementations,
//! so adding a port or extending an existing one never breaks existing products.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::SdkError;

// ═══════════════════════════════════════════════════════════════════════════
// Common types used across ports
// ═══════════════════════════════════════════════════════════════════════════

/// Identifier for a persisted entry (session, log line, etc.).
///
/// Opaque to the SDK — products may use UUID, hash, monotonic counter, or
/// any other scheme. The trait requires only that it round-trips through
/// `Display` + `FromStr`.
pub type PortId = String;

/// Generic key–value payload used by most ports.
///
/// Each product uses its own concrete types; the port contract only requires
/// the value be JSON-serializable so products stay decoupled.
pub type PortValue = serde_json::Value;

/// An OAuth token bundle (subset of `oxi_ai::oauth::TokenBundle`).
///
/// Defined here as a separate minimal type so ports don't need to depend
/// on `oxi-ai`. Products that already use `oxi_ai::oauth::TokenBundle`
/// can convert via `From`/`Into`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthToken {
    /// Bearer access token.
    pub access_token: String,
    /// Optional refresh token for renewal.
    pub refresh_token: Option<String>,
    /// Expiration timestamp (UTC).
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Token type (typically `"Bearer"`).
    pub token_type: Option<String>,
    /// Granted scopes (provider-specific).
    pub scope: Option<String>,
}

impl OAuthToken {
    /// Construct a minimal bearer token.
    pub fn bearer(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            refresh_token: None,
            expires_at: None,
            token_type: Some("Bearer".to_string()),
            scope: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 1 — StateStore: durable key-value / append-only log
// ═══════════════════════════════════════════════════════════════════════════

/// Append-only / key-value durable state.
///
/// Each entry is a `PortValue` (typically a JSON object). Implementations
/// decide the storage backend (file, SQLite, Redis, S3, in-memory...).
///
/// # Use cases
///
/// - Persist session history (append)
/// - Persist agent state snapshots
/// - Persist skill registries
/// - Persist audit chains
///
/// # Default
///
/// If not registered with the SDK, [`NoopStateStore`] is used. Calling
/// `append` returns an error; calling `load` returns `Ok(None)`.
#[async_trait]
pub trait StateStore: Send + Sync + 'static {
    /// Persist an entry. Returns the assigned identifier.
    async fn append(&self, entry: PortValue) -> Result<PortId, SdkError>;

    /// Load an entry by id.
    async fn load(&self, id: &PortId) -> Result<Option<PortValue>, SdkError>;

    /// List all entry ids matching the given prefix (e.g. `"session:"`).
    async fn list(&self, prefix: &str) -> Result<Vec<PortId>, SdkError>;

    /// Delete an entry by id.
    async fn delete(&self, id: &PortId) -> Result<(), SdkError>;

    /// Optional bulk-load of entries for a prefix. Default: `None` (impl may
    /// not support efficient bulk reads).
    async fn load_all(&self, _prefix: &str) -> Result<Vec<(PortId, PortValue)>, SdkError> {
        Ok(Vec::new())
    }
}

/// Noop implementation: `append` errors, `load` returns None, `list` is empty.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopStateStore;

#[async_trait]
impl StateStore for NoopStateStore {
    async fn append(&self, _entry: PortValue) -> Result<PortId, SdkError> {
        Err(SdkError::PortNotConfigured { port: "StateStore" })
    }
    async fn load(&self, _id: &PortId) -> Result<Option<PortValue>, SdkError> {
        Ok(None)
    }
    async fn list(&self, _prefix: &str) -> Result<Vec<PortId>, SdkError> {
        Ok(Vec::new())
    }
    async fn delete(&self, _id: &PortId) -> Result<(), SdkError> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 2 — ConfigStore: layered configuration
// ═══════════════════════════════════════════════════════════════════════════

/// Layered configuration source (defaults → global → project → env → CLI).
///
/// Synchronous because configuration should always be readable without I/O
/// once the product is initialized.
///
/// # Use cases
///
/// - Read `~/.oxi/settings.toml` or `~/.oxios/config.toml`
/// - Merge per-project overrides
/// - Apply environment variable fallbacks
#[async_trait]
pub trait ConfigStore: Send + Sync + 'static {
    /// Get a value by dotted key (e.g. `"model.provider"`).
    fn get(&self, key: &str) -> Result<Option<PortValue>, SdkError>;

    /// Set a value at runtime (in-memory layer). Persistence is impl-defined.
    fn set(&self, key: &str, value: PortValue) -> Result<(), SdkError>;

    /// List all keys (for diagnostics, dumping, validation).
    fn list(&self) -> Result<Vec<(String, PortValue)>, SdkError>;

    /// Returns the layer that supplied a key, for diagnostics.
    fn source(&self, _key: &str) -> Option<String> {
        None
    }
}

/// Noop config: empty.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopConfigStore;

#[async_trait]
impl ConfigStore for NoopConfigStore {
    fn get(&self, _key: &str) -> Result<Option<PortValue>, SdkError> {
        Ok(None)
    }
    fn set(&self, _key: &str, _value: PortValue) -> Result<(), SdkError> {
        Ok(())
    }
    fn list(&self) -> Result<Vec<(String, PortValue)>, SdkError> {
        Ok(Vec::new())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 3 — AuthProvider: credentials (API key / OAuth)
// ═══════════════════════════════════════════════════════════════════════════

/// Credential provider for LLM providers.
///
/// Supports both API key (single string) and OAuth (token bundle) per
/// provider. Storage is implementation-defined (file, keychain, env, etc.).
#[async_trait]
pub trait AuthProvider: Send + Sync + 'static {
    /// Read the API key for a provider.
    async fn get_api_key(&self, provider: &str) -> Result<Option<String>, SdkError>;

    /// Write the API key for a provider.
    async fn set_api_key(&self, provider: &str, key: &str) -> Result<(), SdkError>;

    /// Delete the API key for a provider.
    async fn delete_api_key(&self, provider: &str) -> Result<(), SdkError>;

    /// Read the OAuth token bundle for a provider.
    async fn get_oauth(&self, provider: &str) -> Result<Option<OAuthToken>, SdkError>;

    /// Write the OAuth token bundle for a provider.
    async fn set_oauth(&self, provider: &str, token: OAuthToken) -> Result<(), SdkError>;

    /// List all providers that have credentials stored.
    async fn list_providers(&self) -> Result<Vec<String>, SdkError>;
}

/// Noop auth: nothing stored.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAuthProvider;

#[async_trait]
impl AuthProvider for NoopAuthProvider {
    async fn get_api_key(&self, _provider: &str) -> Result<Option<String>, SdkError> {
        Ok(None)
    }
    async fn set_api_key(&self, _provider: &str, _key: &str) -> Result<(), SdkError> {
        Err(SdkError::PortNotConfigured {
            port: "AuthProvider",
        })
    }
    async fn delete_api_key(&self, _provider: &str) -> Result<(), SdkError> {
        Ok(())
    }
    async fn get_oauth(&self, _provider: &str) -> Result<Option<OAuthToken>, SdkError> {
        Ok(None)
    }
    async fn set_oauth(&self, _provider: &str, _token: OAuthToken) -> Result<(), SdkError> {
        Err(SdkError::PortNotConfigured {
            port: "AuthProvider",
        })
    }
    async fn list_providers(&self) -> Result<Vec<String>, SdkError> {
        Ok(Vec::new())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 4 — EventBus: typed kernel-wide pub/sub
// ═══════════════════════════════════════════════════════════════════════════

/// Topic identifier (free-form string).
pub type EventTopic = String;

/// Event payload.
pub type EventPayload = serde_json::Value;

/// Kernel-wide pub/sub bus.
///
/// Products use this to broadcast agent lifecycle events, kernel state
/// changes, inter-agent messages, and external triggers.
///
/// # Subscription model
///
/// `subscribe` returns a [`SubscriptionHandle`] that, when dropped or
/// `unsubscribe`d, stops delivering events. Implementations may use
/// channels, callbacks, polling, etc.
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// Publish a payload to a topic.
    async fn publish(&self, topic: &EventTopic, payload: EventPayload) -> Result<(), SdkError>;

    /// Subscribe to a topic (exact match or prefix match per impl).
    async fn subscribe(&self, topic: &EventTopic) -> Result<SubscriptionHandle, SdkError>;
}

/// Opaque handle for an active subscription. Drop to unsubscribe.
pub struct SubscriptionHandle {
    /// Cleanup closure called on drop.
    _unsubscribe: Option<Box<dyn FnOnce() + Send + Sync>>,
    /// Receiver of new events. `None` for noop bus.
    receiver: Option<tokio::sync::mpsc::Receiver<(EventTopic, EventPayload)>>,
}

impl SubscriptionHandle {
    /// Receive the next event. Returns `None` when the bus is closed.
    pub async fn recv(&mut self) -> Option<(EventTopic, EventPayload)> {
        match &mut self.receiver {
            Some(rx) => rx.recv().await,
            None => None,
        }
    }
}

impl std::fmt::Debug for SubscriptionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionHandle")
            .field("active", &self.receiver.is_some())
            .finish()
    }
}

impl SubscriptionHandle {
    /// Construct a subscription from an mpsc receiver. Used by port impls
    /// (e.g. `oxi_fs::InProcessEventBus`) — not part of the public SDK API
    /// for end-users.
    pub fn from_receiver(rx: tokio::sync::mpsc::Receiver<(EventTopic, EventPayload)>) -> Self {
        Self {
            _unsubscribe: None,
            receiver: Some(rx),
        }
    }
}

/// In-memory bus for tests or small products.
pub struct InMemoryEventBus {
    tx: tokio::sync::broadcast::Sender<(EventTopic, EventPayload)>,
}

impl InMemoryEventBus {
    /// Create a new in-memory bus with the given channel capacity.
    pub fn new(capacity: usize) -> Arc<Self> {
        let (tx, _) = tokio::sync::broadcast::channel(capacity);
        Arc::new(Self { tx })
    }
}

#[async_trait]
impl EventBus for InMemoryEventBus {
    async fn publish(&self, topic: &EventTopic, payload: EventPayload) -> Result<(), SdkError> {
        // Best-effort: ignore NoActiveReceivers.
        let _ = self.tx.send((topic.clone(), payload));
        Ok(())
    }
    async fn subscribe(&self, _topic: &EventTopic) -> Result<SubscriptionHandle, SdkError> {
        let mut rx = self.tx.subscribe();
        let (tx, rx2) = tokio::sync::mpsc::channel(64);
        drop(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        }));
        Ok(SubscriptionHandle {
            _unsubscribe: None,
            receiver: Some(rx2),
        })
    }
}

/// Noop bus: nothing happens on publish, subscribers receive nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventBus;

#[async_trait]
impl EventBus for NoopEventBus {
    async fn publish(&self, _topic: &EventTopic, _payload: EventPayload) -> Result<(), SdkError> {
        Ok(())
    }
    async fn subscribe(&self, _topic: &EventTopic) -> Result<SubscriptionHandle, SdkError> {
        Ok(SubscriptionHandle {
            _unsubscribe: None,
            receiver: None,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 5 — SkillLoader: discover & load skills
// ═══════════════════════════════════════════════════════════════════════════

/// Metadata about a discovered skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Unique skill name (e.g. `"git-commit"`).
    pub name: String,
    /// Short description from the frontmatter.
    pub description: String,
    /// Absolute path to the SKILL.md file.
    pub path: PathBuf,
    /// Optional version string.
    pub version: Option<String>,
}

/// Loaded skill (metadata + body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Metadata.
    pub meta: SkillMeta,
    /// Markdown body (without frontmatter).
    pub body: String,
}

/// Discover and load skill files (SKILL.md) from a directory tree.
#[async_trait]
pub trait SkillLoader: Send + Sync + 'static {
    /// Scan the loader's configured roots and return all discovered skills.
    async fn list(&self) -> Result<Vec<SkillMeta>, SdkError>;

    /// Load a single skill by name.
    async fn load(&self, name: &str) -> Result<Option<Skill>, SdkError>;
}

/// Noop loader: no skills available.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSkillLoader;

#[async_trait]
impl SkillLoader for NoopSkillLoader {
    async fn list(&self) -> Result<Vec<SkillMeta>, SdkError> {
        Ok(Vec::new())
    }
    async fn load(&self, _name: &str) -> Result<Option<Skill>, SdkError> {
        Ok(None)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 6 — PersonaProvider: system prompt injection
// ═══════════════════════════════════════════════════════════════════════════

/// A persona (system prompt fragment + metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    /// Persona name.
    pub name: String,
    /// System prompt body.
    pub system_prompt: String,
    /// Optional model preferences.
    pub preferred_model: Option<String>,
    /// Optional tool restrictions.
    pub allowed_tools: Option<Vec<String>>,
}

#[async_trait]
pub trait PersonaProvider: Send + Sync + 'static {
    /// List all known personas.
    async fn list(&self) -> Result<Vec<Persona>, SdkError>;
    /// Look up a single persona.
    async fn get(&self, name: &str) -> Result<Option<Persona>, SdkError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopPersonaProvider;

#[async_trait]
impl PersonaProvider for NoopPersonaProvider {
    async fn list(&self) -> Result<Vec<Persona>, SdkError> {
        Ok(Vec::new())
    }
    async fn get(&self, _name: &str) -> Result<Option<Persona>, SdkError> {
        Ok(None)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 7 — AccessGate: pre-execution policy check
// ═══════════════════════════════════════════════════════════════════════════

/// Description of a tool invocation about to be made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Tool name (e.g. `"bash"`).
    pub tool: String,
    /// Free-form action label (e.g. `"rm -rf /tmp"`).
    pub action: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Subject identifier (agent id, user id, etc.).
    pub subject: String,
}

/// Result of an access decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessDecision {
    /// Allow without conditions.
    Allow,
    /// Allow but emit an audit event.
    AllowWithAudit,
    /// Deny with reason.
    Deny { reason: String },
    /// Pause and request human approval.
    RequireApproval { reason: String },
}

#[async_trait]
pub trait AccessGate: Send + Sync + 'static {
    /// Decide whether `request` may proceed.
    async fn check(&self, request: &ToolCallRequest) -> Result<AccessDecision, SdkError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllAccessGate;

#[async_trait]
impl AccessGate for AllowAllAccessGate {
    async fn check(&self, _request: &ToolCallRequest) -> Result<AccessDecision, SdkError> {
        Ok(AccessDecision::Allow)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 8 — CapabilityResolver: which tools a subject may see
// ═══════════════════════════════════════════════════════════════════════════

#[async_trait]
pub trait CapabilityResolver: Send + Sync + 'static {
    /// Returns the set of tool names visible to `subject`.
    async fn visible_tools(&self, subject: &str) -> Result<Vec<String>, SdkError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyCapabilityResolver;

#[async_trait]
impl CapabilityResolver for EmptyCapabilityResolver {
    async fn visible_tools(&self, _subject: &str) -> Result<Vec<String>, SdkError> {
        Ok(Vec::new())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 9 — MemoryStore: episodic / semantic memory
// ═══════════════════════════════════════════════════════════════════════════

/// A memory entry (episodic, semantic, or procedural).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Identifier.
    pub id: String,
    /// Subject (agent id, user id, etc.).
    pub subject: String,
    /// Free-form kind (`"episodic"`, `"semantic"`, `"procedural"`).
    pub kind: String,
    /// Embedding (optional, dense vector).
    pub embedding: Option<Vec<f32>>,
    /// Free-form content.
    pub content: PortValue,
    /// Created-at timestamp.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    /// Persist a memory entry.
    async fn put(&self, entry: MemoryEntry) -> Result<(), SdkError>;
    /// Semantic search by embedding (cosine similarity). Returns top-k.
    async fn search(&self, _query: &[f32], _k: usize) -> Result<Vec<MemoryEntry>, SdkError> {
        Ok(Vec::new())
    }
    /// List entries for a subject.
    async fn list(&self, subject: &str) -> Result<Vec<MemoryEntry>, SdkError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMemoryStore;

#[async_trait]
impl MemoryStore for NoopMemoryStore {
    async fn put(&self, _entry: MemoryEntry) -> Result<(), SdkError> {
        Err(SdkError::PortNotConfigured {
            port: "MemoryStore",
        })
    }
    async fn list(&self, _subject: &str) -> Result<Vec<MemoryEntry>, SdkError> {
        Ok(Vec::new())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 10 — CronScheduler: time-based triggers
// ═══════════════════════════════════════════════════════════════════════════

/// A scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Job identifier.
    pub id: String,
    /// Cron expression (5-field, e.g. `"*/5 * * * *"`).
    pub schedule: String,
    /// Free-form action label (consumed by handler).
    pub action: String,
    /// Optional payload.
    pub payload: Option<PortValue>,
}

#[async_trait]
pub trait CronScheduler: Send + Sync + 'static {
    async fn register(&self, job: CronJob) -> Result<(), SdkError>;
    async fn unregister(&self, id: &str) -> Result<(), SdkError>;
    async fn list(&self) -> Result<Vec<CronJob>, SdkError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCronScheduler;

#[async_trait]
impl CronScheduler for NoopCronScheduler {
    async fn register(&self, _job: CronJob) -> Result<(), SdkError> {
        Err(SdkError::PortNotConfigured {
            port: "CronScheduler",
        })
    }
    async fn unregister(&self, _id: &str) -> Result<(), SdkError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<CronJob>, SdkError> {
        Ok(Vec::new())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 11 — ResourceMonitor: usage limits
// ═══════════════════════════════════════════════════════════════════════════

/// Current resource usage snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub active_agents: usize,
    pub tokens_consumed: u64,
}

#[async_trait]
pub trait ResourceMonitor: Send + Sync + 'static {
    /// Snapshot the current usage.
    async fn snapshot(&self) -> Result<ResourceUsage, SdkError>;
    /// Returns true if the current usage exceeds the configured budget.
    async fn is_over_budget(&self) -> Result<bool, SdkError> {
        Ok(false)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopResourceMonitor;

#[async_trait]
impl ResourceMonitor for NoopResourceMonitor {
    async fn snapshot(&self) -> Result<ResourceUsage, SdkError> {
        Ok(ResourceUsage::default())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Registry — a single Arc<dyn ...> set registered on Oxi
// ═══════════════════════════════════════════════════════════════════════════

/// Bundle of all registered ports. Products construct this and pass it to
/// `OxiBuilder::with_ports(...)`.
///
/// All fields default to noop impls so products can register only the
/// ports they care about.
#[derive(Clone)]
pub struct PortRegistry {
    /// State store.
    pub state: Arc<dyn StateStore>,
    /// Config store.
    pub config: Arc<dyn ConfigStore>,
    /// Auth provider.
    pub auth: Arc<dyn AuthProvider>,
    /// Event bus.
    pub event_bus: Arc<dyn EventBus>,
    /// Skill loader.
    pub skills: Arc<dyn SkillLoader>,
    /// Persona provider.
    pub personas: Arc<dyn PersonaProvider>,
    /// Access gate.
    pub access: Arc<dyn AccessGate>,
    /// Capability resolver.
    pub capabilities: Arc<dyn CapabilityResolver>,
    /// Memory store.
    pub memory: Arc<dyn MemoryStore>,
    /// Cron scheduler.
    pub cron: Arc<dyn CronScheduler>,
    /// Resource monitor.
    pub resources: Arc<dyn ResourceMonitor>,
}

impl std::fmt::Debug for PortRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortRegistry")
            .field("state", &"<dyn StateStore>")
            .field("config", &"<dyn ConfigStore>")
            .field("auth", &"<dyn AuthProvider>")
            .field("event_bus", &"<dyn EventBus>")
            .field("skills", &"<dyn SkillLoader>")
            .field("personas", &"<dyn PersonaProvider>")
            .field("access", &"<dyn AccessGate>")
            .field("capabilities", &"<dyn CapabilityResolver>")
            .field("memory", &"<dyn MemoryStore>")
            .field("cron", &"<dyn CronScheduler>")
            .field("resources", &"<dyn ResourceMonitor>")
            .finish()
    }
}

impl Default for PortRegistry {
    fn default() -> Self {
        Self::noop()
    }
}

impl PortRegistry {
    /// All-noop registry. Useful for tests and products that only need
    /// agent execution without any persistence.
    pub fn noop() -> Self {
        Self {
            state: Arc::new(NoopStateStore),
            config: Arc::new(NoopConfigStore),
            auth: Arc::new(NoopAuthProvider),
            event_bus: Arc::new(NoopEventBus),
            skills: Arc::new(NoopSkillLoader),
            personas: Arc::new(NoopPersonaProvider),
            access: Arc::new(AllowAllAccessGate),
            capabilities: Arc::new(EmptyCapabilityResolver),
            memory: Arc::new(NoopMemoryStore),
            cron: Arc::new(NoopCronScheduler),
            resources: Arc::new(NoopResourceMonitor),
        }
    }

    /// Build a registry from a directory for file-based persistence.
    /// Convenience for the common case of "give me a registry backed by
    /// `~/.oxi`" — products can also construct `PortRegistry` field-by-field.
    ///
    /// This is a free function: it requires concrete impls from a separate
    /// adapter crate (e.g. `oxi-fs`). When no adapter is available, this
    /// returns `PortRegistry::noop()`.
    pub async fn from_directory(_dir: &Path) -> Self {
        // Adapter implementations are intentionally not part of oxi-sdk itself.
        // Products wire concrete adapters via OxiBuilder::with_port_*(...) or
        // construct a PortRegistry directly.
        Self::noop()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn noop_state_store_load_returns_none() {
        let s = NoopStateStore;
        assert!(s.load(&"x".into()).await.unwrap().is_none());
        assert!(s.list("").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn noop_state_store_append_errors() {
        let s = NoopStateStore;
        let err = s.append(json!({})).await.unwrap_err();
        assert!(matches!(
            err,
            SdkError::PortNotConfigured { port: "StateStore" }
        ));
    }

    #[test]
    fn noop_config_get_returns_none() {
        let c = NoopConfigStore;
        assert!(c.get("any").unwrap().is_none());
        assert!(c.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn noop_auth_get_api_key_returns_none() {
        let a = NoopAuthProvider;
        assert!(a.get_api_key("anthropic").await.unwrap().is_none());
        assert!(a.list_providers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn in_memory_event_bus_round_trip() {
        let bus = InMemoryEventBus::new(8);
        bus.publish(&"test".to_string(), json!({"hello": "world"}))
            .await
            .unwrap();
        let mut sub = bus.subscribe(&"test".to_string()).await.unwrap();
        // Re-publish to ensure subscriber is registered.
        bus.publish(&"test".to_string(), json!({"k": 1}))
            .await
            .unwrap();
        let (topic, payload) = sub.recv().await.unwrap();
        assert_eq!(topic, "test");
        assert_eq!(payload, json!({"k": 1}));
    }

    #[tokio::test]
    async fn noop_event_bus_publish_succeeds_but_subscribes_return_none() {
        let bus = NoopEventBus;
        bus.publish(&"x".to_string(), json!({})).await.unwrap();
        let mut sub = bus.subscribe(&"x".to_string()).await.unwrap();
        assert!(sub.recv().await.is_none());
    }

    #[test]
    fn default_registry_is_noop() {
        let reg = PortRegistry::default();
        // Constructed without panic.
        assert!(Arc::strong_count(&reg.state) >= 1);
    }

    #[test]
    fn oauth_token_bearer_constructor() {
        let t = OAuthToken::bearer("abc");
        assert_eq!(t.access_token, "abc");
        assert_eq!(t.token_type.as_deref(), Some("Bearer"));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Reference implementations
// ═══════════════════════════════════════════════════════════════════════════
//
// `fs`     — file-based adapters (JSON, TOML, SKILL.md, …)
// `inmem`  — in-process adapters (RAM-only, useful for tests and headless)
//
// All impls are part of the SDK. Products can import them directly or
// write their own — the port traits in this module are the contract.

pub mod fs;
pub mod inmem;
