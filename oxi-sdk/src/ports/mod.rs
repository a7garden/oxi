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
//!   ├── oxi-cli    → FileStateStore, FileAuthProvider, FileSkillLoader, FileModelCatalog
//!   ├── oxios-kernel → OxiosStateStore, OxiosEventBus, OxiosMemoryStore
//!   └── custom     → MyDbStateStore, MyAuthProvider, MyModelCatalog, etc.
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
//! 4. **Async-first** — every port is async-aware because most
//!    implementations touch the file system, network, or database.
//!
//! # Versioning
//!
//! Port traits are **additive**. New methods get default noop implementations,
//! so adding a port or extending an existing one never breaks existing products.

pub mod catalog;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
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

/// How a provider passes its API key in HTTP headers.
///
/// Port-level enum (lives in oxi-sdk so the catalog port's `default_auth()`
/// can return it). Mirrors the existing `oxi_ai::catalog::AuthMethod` and
/// `oxi_ai::providers::AuthMethod` — those will be reconciled in PR 4 when
/// `BuiltinProviderEntry` is removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// `Authorization: Bearer <key>` — most OpenAI-compatible providers.
    #[default]
    Bearer,
    /// `x-api-key: <key>` — Anthropic and Anthropic-compatible providers.
    #[serde(rename = "x-api-key")]
    XApiKey,
    /// `api-key: <key>` — Azure OpenAI.
    #[serde(rename = "api-key")]
    ApiKey,
    /// No API key header (uses other auth like OAuth, SigV4).
    None,
}

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
pub trait StateStore: Send + Sync + 'static {
    /// Persist an entry. Returns the assigned identifier.
    fn append(
        &self,
        entry: PortValue,
    ) -> Pin<Box<dyn Future<Output = Result<PortId, SdkError>> + Send + '_>>;

    /// Load an entry by id.
    fn load(
        &self,
        id: &PortId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PortValue>, SdkError>> + Send + '_>>;

    /// List all entry ids matching the given prefix (e.g. `"session:"`).
    fn list(
        &self,
        prefix: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PortId>, SdkError>> + Send + '_>>;

    /// Delete an entry by id.
    fn delete(
        &self,
        id: &PortId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;

    /// Optional bulk-load of entries for a prefix. Default: `None` (impl may
    /// not support efficient bulk reads).
    #[allow(clippy::type_complexity)]
    fn load_all(
        &self,
        _prefix: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<(PortId, PortValue)>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Noop implementation: `append` errors, `load` returns None, `list` is empty.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopStateStore;

impl StateStore for NoopStateStore {
    fn append(
        &self,
        _entry: PortValue,
    ) -> Pin<Box<dyn Future<Output = Result<PortId, SdkError>> + Send + '_>> {
        Box::pin(async { Err(SdkError::PortNotConfigured { port: "StateStore" }) })
    }
    fn load(
        &self,
        _id: &PortId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PortValue>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
    fn list(
        &self,
        _prefix: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PortId>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn delete(
        &self,
        _id: &PortId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
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
pub trait AuthProvider: Send + Sync + 'static {
    /// Read the API key for a provider.
    fn get_api_key(
        &self,
        provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, SdkError>> + Send + '_>>;

    /// Write the API key for a provider.
    fn set_api_key(
        &self,
        provider: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;

    /// Delete the API key for a provider.
    fn delete_api_key(
        &self,
        provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;

    /// Read the OAuth token bundle for a provider.
    fn get_oauth(
        &self,
        provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<OAuthToken>, SdkError>> + Send + '_>>;

    /// Write the OAuth token bundle for a provider.
    fn set_oauth(
        &self,
        provider: &str,
        token: OAuthToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;

    /// List all providers that have credentials stored.
    fn list_providers(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SdkError>> + Send + '_>>;
}

/// Noop auth: nothing stored.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAuthProvider;

impl AuthProvider for NoopAuthProvider {
    fn get_api_key(
        &self,
        _provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
    fn set_api_key(
        &self,
        _provider: &str,
        _key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async {
            Err(SdkError::PortNotConfigured {
                port: "AuthProvider",
            })
        })
    }
    fn delete_api_key(
        &self,
        _provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
    fn get_oauth(
        &self,
        _provider: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<OAuthToken>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
    fn set_oauth(
        &self,
        _provider: &str,
        _token: OAuthToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async {
            Err(SdkError::PortNotConfigured {
                port: "AuthProvider",
            })
        })
    }
    fn list_providers(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
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
pub trait EventBus: Send + Sync + 'static {
    /// Publish a payload to a topic.
    fn publish(
        &self,
        topic: &EventTopic,
        payload: EventPayload,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;

    /// Subscribe to a topic (exact match or prefix match per impl).
    fn subscribe(
        &self,
        topic: &EventTopic,
    ) -> Pin<Box<dyn Future<Output = Result<SubscriptionHandle, SdkError>> + Send + '_>>;
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

impl EventBus for InMemoryEventBus {
    fn publish(
        &self,
        topic: &EventTopic,
        payload: EventPayload,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        // Best-effort: ignore NoActiveReceivers.
        let _ = self.tx.send((topic.clone(), payload));
        Box::pin(async { Ok(()) })
    }
    fn subscribe(
        &self,
        _topic: &EventTopic,
    ) -> Pin<Box<dyn Future<Output = Result<SubscriptionHandle, SdkError>> + Send + '_>> {
        let mut rx = self.tx.subscribe();
        let (tx, rx2) = tokio::sync::mpsc::channel(64);
        drop(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        }));
        Box::pin(async {
            Ok(SubscriptionHandle {
                _unsubscribe: None,
                receiver: Some(rx2),
            })
        })
    }
}

/// Noop bus: nothing happens on publish, subscribers receive nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventBus;

impl EventBus for NoopEventBus {
    fn publish(
        &self,
        _topic: &EventTopic,
        _payload: EventPayload,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
    fn subscribe(
        &self,
        _topic: &EventTopic,
    ) -> Pin<Box<dyn Future<Output = Result<SubscriptionHandle, SdkError>> + Send + '_>> {
        Box::pin(async {
            Ok(SubscriptionHandle {
                _unsubscribe: None,
                receiver: None,
            })
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
pub trait SkillLoader: Send + Sync + 'static {
    /// Scan the loader's configured roots and return all discovered skills.
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<SkillMeta>, SdkError>> + Send + '_>>;

    /// Load a single skill by name.
    fn load(
        &self,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Skill>, SdkError>> + Send + '_>>;
}

/// Noop loader: no skills available.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSkillLoader;

impl SkillLoader for NoopSkillLoader {
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<SkillMeta>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn load(
        &self,
        _name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Skill>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(None) })
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

/// Source of personas (system prompt fragments) selectable by name.
pub trait PersonaProvider: Send + Sync + 'static {
    /// List all known personas.
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Persona>, SdkError>> + Send + '_>>;
    /// Look up a single persona.
    fn get(
        &self,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Persona>, SdkError>> + Send + '_>>;
}

/// Noop provider: lists nothing, lookups return `None`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopPersonaProvider;

impl PersonaProvider for NoopPersonaProvider {
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Persona>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn get(
        &self,
        _name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Persona>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(None) })
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
    Deny {
        /// Why access was denied.
        reason: String,
    },
    /// Pause and request human approval.
    RequireApproval {
        /// Why human approval is required.
        reason: String,
    },
}

/// Pre-execution policy check for tool invocations.
pub trait AccessGate: Send + Sync + 'static {
    /// Decide whether `request` may proceed.
    fn check(
        &self,
        request: &ToolCallRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AccessDecision, SdkError>> + Send + '_>>;
}

/// Permissive gate: every request is `Allow`ed.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllAccessGate;

impl AccessGate for AllowAllAccessGate {
    fn check(
        &self,
        _request: &ToolCallRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AccessDecision, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(AccessDecision::Allow) })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 8 — CapabilityResolver: which tools a subject may see
// ═══════════════════════════════════════════════════════════════════════════

/// Resolves the set of tools visible to a given subject.
pub trait CapabilityResolver: Send + Sync + 'static {
    /// Returns the set of tool names visible to `subject`.
    fn visible_tools(
        &self,
        subject: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SdkError>> + Send + '_>>;
}

/// Resolver that exposes no tools to any subject.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyCapabilityResolver;

impl CapabilityResolver for EmptyCapabilityResolver {
    fn visible_tools(
        &self,
        _subject: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
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

/// Episodic / semantic / procedural memory store with optional vector search.
pub trait MemoryStore: Send + Sync + 'static {
    /// Persist a memory entry.
    fn put(
        &self,
        entry: MemoryEntry,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;
    /// Semantic search by embedding (cosine similarity). Returns top-k.
    fn search(
        &self,
        _query: &[f32],
        _k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// List entries for a subject.
    fn list(
        &self,
        subject: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + '_>>;
}

/// Noop store: `put` errors, `list` and `search` return empty.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMemoryStore;

impl MemoryStore for NoopMemoryStore {
    fn put(
        &self,
        _entry: MemoryEntry,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async {
            Err(SdkError::PortNotConfigured {
                port: "MemoryStore",
            })
        })
    }
    fn list(
        &self,
        _subject: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
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

/// Registers and introspects time-based jobs.
pub trait CronScheduler: Send + Sync + 'static {
    /// Register a new job (replaces any existing job with the same id).
    fn register(
        &self,
        job: CronJob,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;
    /// Remove a previously registered job by id.
    fn unregister(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>>;
    /// List all currently registered jobs.
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<CronJob>, SdkError>> + Send + '_>>;
}

/// Noop scheduler: `register` errors, `list` is empty.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCronScheduler;

impl CronScheduler for NoopCronScheduler {
    fn register(
        &self,
        _job: CronJob,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async {
            Err(SdkError::PortNotConfigured {
                port: "CronScheduler",
            })
        })
    }
    fn unregister(
        &self,
        _id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<CronJob>, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 11 — ResourceMonitor: usage limits
// ═══════════════════════════════════════════════════════════════════════════

/// Current resource usage snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// CPU usage percentage (0–100).
    pub cpu_percent: f32,
    /// Resident memory in bytes.
    pub memory_bytes: u64,
    /// Disk usage in bytes.
    pub disk_bytes: u64,
    /// Number of currently running agents.
    pub active_agents: usize,
    /// Total tokens consumed across all agents.
    pub tokens_consumed: u64,
}

/// Reports current resource usage and whether the budget is exceeded.
pub trait ResourceMonitor: Send + Sync + 'static {
    /// Snapshot the current usage.
    fn snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceUsage, SdkError>> + Send + '_>>;
    /// Returns true if the current usage exceeds the configured budget.
    fn is_over_budget(&self) -> Pin<Box<dyn Future<Output = Result<bool, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(false) })
    }
}

/// Noop monitor: reports zero usage and never exceeds budget.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopResourceMonitor;

impl ResourceMonitor for NoopResourceMonitor {
    fn snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceUsage, SdkError>> + Send + '_>> {
        Box::pin(async { Ok(ResourceUsage::default()) })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 12 — InternalUrlRouter: protocol-scheme virtual path resolution.
// ═══════════════════════════════════════════════════════════════════════════

/// A resolved virtual URL result. Consumed by `read`/`search` tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedUrl {
    /// Normalized original URL (debug/logging).
    pub url: String,
    /// Resolved text content.
    pub content: String,
    /// MIME type: "text/markdown" | "application/json" | "text/plain".
    pub content_type: String,
    /// Byte size (optional).
    pub size: Option<usize>,
    /// Debug source path (not exposed to model).
    pub source_path: Option<String>,
    /// Extra notes (resolution warnings, etc.).
    pub notes: Vec<String>,
    /// true → uneditable (hashline anchor suppression).
    pub immutable: bool,
}

/// Router call context (identifies the calling session).
#[derive(Debug, Clone, Default)]
pub struct ResolveContext {
    /// Working directory of the calling session.
    pub cwd: Option<PathBuf>,
    /// Identifier of the calling session.
    pub session_id: Option<String>,
}

/// Resolves `scheme://path` URIs (issue://, pr://, agent://, etc.) into text.
pub trait InternalUrlRouter: Send + Sync + 'static {
    /// Resolve a `scheme://path` URI to text content.
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
        ctx: &'a ResolveContext,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedUrl, SdkError>> + Send + 'a>>;

    /// Schemes this router handles. Empty = handles none.
    fn schemes(&self) -> &[&str] {
        &[]
    }

    /// Currently registered schemes (for diagnostics). Default: empty.
    fn registered_schemes(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Noop router: `resolve` always errors with `PortNotConfigured`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopInternalUrlRouter;

impl InternalUrlRouter for NoopInternalUrlRouter {
    fn resolve<'a>(
        &'a self,
        _uri: &'a str,
        _ctx: &'a ResolveContext,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedUrl, SdkError>> + Send + 'a>> {
        Box::pin(async {
            Err(SdkError::PortNotConfigured {
                port: "InternalUrlRouter",
            })
        })
    }
}

/// Single-scheme handler contract. Products implement one per scheme
/// (issue://, pr://, agent://, etc.) and register them with the router.
#[async_trait]
pub trait ProtocolHandler: Send + Sync {
    /// Lowercase scheme this handler serves ("issue", "pr", …).
    fn scheme(&self) -> &str;
    /// When true, the resolved content is immutable (hashline anchor suppressed).
    fn immutable(&self) -> bool {
        false
    }
    /// Resolve a URL path (scheme already stripped) to text content.
    async fn resolve(
        &self,
        url: &str,
        selector: Option<&str>,
        ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError>;
}

/// Auto-completion entry returned by a handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlCompletion {
    /// Completion text to insert.
    pub value: String,
    /// Short label for the completion menu.
    pub label: Option<String>,
    /// Longer description shown alongside the label.
    pub description: Option<String>,
}

/// Line map metadata for selector processing (read tool delegates to this).
#[derive(Debug, Clone, Default)]
pub struct LineMap {
    /// Total number of lines in the source.
    pub total_lines: u32,
    /// 1-indexed displayable ranges (gaps represent elided regions).
    pub displayable: Option<Vec<(u32, u32)>>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 13 — RuleRegistry: TTSR rules source.
// ═══════════════════════════════════════════════════════════════════════════

/// A TTSR rule. Condition is a regex matched against streaming output.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Rule name (unique identifier).
    pub name: String,
    /// Rule body injected into the system prompt when conditions match.
    pub content: String,
    /// Human-readable summary of what the rule does.
    pub description: Option<String>,
    /// Regex patterns to match against stream text.
    pub condition: Vec<regex::Regex>,
    /// Scope tokens limiting which stream sources trigger.
    pub scope: Vec<ScopeToken>,
    /// When (if ever) this rule interrupts the agent loop.
    pub interrupt_mode: InterruptMode,
    /// File globs that further restrict the rule's applicability.
    pub globs: Vec<String>,
    /// If true, always included in system prompt.
    pub always_apply: bool,
    /// Where the rule was loaded from.
    pub source: RuleSource,
}

/// Stream-source scope a TTSR rule can match against.
#[derive(Debug, Clone)]
pub enum ScopeToken {
    /// Matches assistant prose output.
    Text,
    /// Matches model thinking/reasoning output.
    Thinking,
    /// Matches tool-call arguments, optionally filtered by tool name and globs.
    Tool {
        /// Tool name to match.
        name: String,
        /// File globs that restrict which tool calls this scope matches.
        globs: Vec<String>,
    },
}

/// When a TTSR rule fires relative to prose/tool output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptMode {
    /// Never interrupts.
    Never,
    /// Interrupts on prose output only.
    ProseOnly,
    /// Interrupts on tool output only.
    ToolOnly,
    /// Interrupts on any matching output.
    Always,
}

/// Origin of a TTSR rule.
#[derive(Debug, Clone)]
pub enum RuleSource {
    /// Shipped with the SDK.
    BuiltinDefaults,
    /// Loaded from the project's rule files.
    Project,
    /// Loaded from the user's global rule files.
    User,
}

/// Source of TTSR rules and injection bookkeeping.
pub trait RuleRegistry: Send + Sync + 'static {
    /// Return all currently active rules.
    fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>>;
    /// Record that `name` was injected on `turn` (dedup bookkeeping).
    fn mark_injected(&self, _name: &str, _turn: u64) {}
    /// Return all (name, turn) injection records.
    fn injected_records(&self) -> Vec<(String, u64)> {
        Vec::new()
    }
    /// Restore injection records (e.g. after compaction).
    fn restore(&self, _records: Vec<(String, u64)>) {}
}

/// Noop registry: returns no rules.
#[derive(Default)]
pub struct NoopRuleRegistry;

impl RuleRegistry for NoopRuleRegistry {
    fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>> {
        Box::pin(async { Vec::new() })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Port 14 — EmbeddingProvider: text → vector for semantic search.
// ═══════════════════════════════════════════════════════════════════════════

/// Produces dense vector embeddings for semantic memory search.
pub trait EmbeddingProvider: Send + Sync + 'static {
    /// Produce a dense embedding vector for `text`.
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SdkError>> + Send + 'a>>;
}

/// Noop provider: `embed` always errors with `PortNotConfigured`.
pub struct NoopEmbeddingProvider;

impl EmbeddingProvider for NoopEmbeddingProvider {
    fn embed<'a>(
        &'a self,
        _text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SdkError>> + Send + 'a>> {
        Box::pin(async {
            Err(SdkError::PortNotConfigured {
                port: "EmbeddingProvider",
            })
        })
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
    /// Model catalog — provider/model metadata source of truth.
    /// Default: [`catalog::NoopModelCatalog`] (empty results).
    pub catalog: Arc<dyn catalog::ModelCatalog>,
    /// Internal URL router — protocol-scheme dispatch.
    /// Default: [`NoopInternalUrlRouter`].
    pub url_router: Arc<dyn InternalUrlRouter>,
    /// Rule registry — TTSR rules.
    /// Default: [`NoopRuleRegistry`].
    pub rules: Arc<dyn RuleRegistry>,
    /// Embedding provider — text→vector for semantic search.
    /// Default: [`NoopEmbeddingProvider`].
    pub embeddings: Arc<dyn EmbeddingProvider>,
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
            .field("catalog", &"<dyn ModelCatalog>")
            .field("url_router", &"<dyn InternalUrlRouter>")
            .field("rules", &"<dyn RuleRegistry>")
            .field("embeddings", &"<dyn EmbeddingProvider>")
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
            catalog: catalog::NoopModelCatalog::new(),
            url_router: Arc::new(NoopInternalUrlRouter),
            rules: Arc::new(NoopRuleRegistry),
            embeddings: Arc::new(NoopEmbeddingProvider),
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
