//! Port 12 — `ModelCatalog`: source of truth for provider/model metadata.
//!
//! Replaces the legacy `OnceLock`/`LazyLock` globals in `oxi-ai/src/catalog/`
//! with a proper port trait that SDK consumers can implement.
//!
//! See `docs/designs/2026-06-17-catalog-port-design.md` (v3) for rationale.
//!
//! # Layering
//!
//! ```text
//! models.dev JSON → FileModelCatalog (oxi-sdk) → ModelCatalog port
//!                                                         ↑
//!                                                         │ lookup
//!                                                         ▼
//!                                              Oxi / SDK consumer
//! ```
//!
//! # Threading
//!
//! Read methods are async, return owned values. Reference implementations
//! typically hold the snapshot behind `Arc<RwLock<_>>` and replace it
//! atomically on refresh.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::error::SdkResult;

use super::AuthMethod;

// ═══════════════════════════════════════════════════════════════════════════
// Protocol enum — SDK's own type, source of truth for catalog protocol IDs
// ═══════════════════════════════════════════════════════════════════════════

/// Protocol used to talk to a provider/model.
///
/// SDK-owned enum (does not depend on `oxi_ai::Api`). The bridge layer
/// (PR 3, `oxi-sdk/src/bridge.rs`) converts to `oxi_ai::Api` via
/// [`CatalogProtocol::as_oxi_api`].
///
/// New protocol = add a variant + a `protocol_for` mapping line + a bridge
/// dispatch arm. Unknown npm values fall back to [`CatalogProtocol::OpenAiCompatible`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogProtocol {
    /// Anthropic Messages API (`@ai-sdk/anthropic`).
    AnthropicMessages,
    /// OpenAI Chat Completions API (`@ai-sdk/openai`,
    /// `@ai-sdk/openai-compatible`, most aggregators/gateways).
    OpenAiCompletions,
    /// OpenAI Responses API.
    OpenAiResponses,
    /// Azure OpenAI Responses (`@ai-sdk/azure`).
    AzureOpenAiResponses,
    /// Google Generative AI (`@ai-sdk/google`).
    GoogleGenerativeAi,
    /// Google Vertex (`@ai-sdk/google-vertex`,
    /// `@ai-sdk/google-vertex/anthropic`).
    GoogleVertex,
    /// Mistral Conversations (`@ai-sdk/mistral`).
    MistralConversations,
    /// AWS Bedrock Converse Stream (`@ai-sdk/amazon-bedrock`).
    BedrockConverseStream,
    /// OpenAI-compatible fallback for unknown npm values.
    OpenAiCompatible,
}

impl CatalogProtocol {
    /// Default authentication header scheme for this protocol.
    ///
    /// Per-model override (`model.npm`) is applied BEFORE the entry's
    /// `protocol` field is set, so this method's result is the auth for the
    /// actual model — no separate `auth_method` field needed on entries.
    pub fn default_auth(&self) -> AuthMethod {
        match self {
            CatalogProtocol::AnthropicMessages => AuthMethod::XApiKey,
            CatalogProtocol::AzureOpenAiResponses => AuthMethod::ApiKey,
            CatalogProtocol::GoogleGenerativeAi
            | CatalogProtocol::GoogleVertex
            | CatalogProtocol::BedrockConverseStream => AuthMethod::None,
            CatalogProtocol::OpenAiCompletions
            | CatalogProtocol::OpenAiResponses
            | CatalogProtocol::OpenAiCompatible
            | CatalogProtocol::MistralConversations => AuthMethod::Bearer,
        }
    }

    /// Convert to oxi-ai's internal `Api` enum at the impl boundary.
    ///
    /// Always available — oxi-sdk already depends on oxi-ai (re-exports
    /// `Api`/`Model`/`Provider` since v0.x). No feature flag (v3 정정).
    ///
    /// Called only by the SDK bridge layer
    /// (`oxi_sdk::bridge::create_provider_from_entry`, PR 3). Port
    /// implementations never call this method.
    pub fn as_oxi_api(&self) -> oxi_ai::Api {
        use CatalogProtocol::*;
        match self {
            AnthropicMessages => oxi_ai::Api::AnthropicMessages,
            OpenAiCompletions => oxi_ai::Api::OpenAiCompletions,
            OpenAiResponses => oxi_ai::Api::OpenAiResponses,
            AzureOpenAiResponses => oxi_ai::Api::AzureOpenAiResponses,
            GoogleGenerativeAi => oxi_ai::Api::GoogleGenerativeAi,
            GoogleVertex => oxi_ai::Api::GoogleVertex,
            MistralConversations => oxi_ai::Api::MistralConversations,
            BedrockConverseStream => oxi_ai::Api::BedrockConverseStream,
            // OpenAI 호환은 oxi-ai 에서 OpenAiCompletions 로 처리
            OpenAiCompatible => oxi_ai::Api::OpenAiCompletions,
        }
    }

    /// Stable string identifier (kebab-case, matches `Api::to_str`).
    ///
    /// Used for serialization, debug output, and as the bridge key when
    /// the SDK consumer needs to look up a string-form protocol.
    pub fn as_str(&self) -> &'static str {
        use CatalogProtocol::*;
        match self {
            AnthropicMessages => "anthropic-messages",
            OpenAiCompletions => "openai-completions",
            OpenAiResponses => "openai-responses",
            AzureOpenAiResponses => "azure-openai-responses",
            GoogleGenerativeAi => "google-generative-ai",
            GoogleVertex => "google-vertex",
            MistralConversations => "mistral-conversations",
            BedrockConverseStream => "bedrock-converse-stream",
            OpenAiCompatible => "openai-compatible",
        }
    }
}

impl std::fmt::Display for CatalogProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Data types — what the port returns
// ═══════════════════════════════════════════════════════════════════════════

/// Snapshot of a single model entry.
///
/// `Clone` is cheap (mostly strings + small numerics). Owned values rather
/// than `&'static` so consumers can hold snapshots past the catalog's
/// internal lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModelEntry {
    pub provider: String,
    pub model_id: String,
    pub name: String,

    /// Protocol for this specific model. May differ from the parent
    /// provider's protocol (per-model override; dynamic-catalog §2.2).
    pub protocol: CatalogProtocol,

    /// Where this entry came from.
    pub source: CatalogSource,

    /// Model-level base URL override. `None` = inherit from provider.
    /// See dynamic-catalog §2.2 (v3 — 55 models).
    pub base_url: Option<String>,

    pub reasoning: bool,
    pub supports_vision: bool,

    /// USD per million tokens. `0.0` = free or undisclosed by upstream.
    pub cost_input: f64,
    pub cost_output: f64,
    pub cost_cache_read: f64,
    pub cost_cache_write: f64,

    pub context_window: u32,
    pub max_tokens: u32,

    /// Input modalities (e.g. `["text", "image"]`). Empty = unknown.
    pub input_modalities: Vec<String>,
    pub release_date: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProviderEntry {
    pub id: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub protocol: CatalogProtocol,
    pub env_key: Option<String>,
    pub extra_env_keys: Vec<String>,
    pub base_url: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    /// `category` and `description` may be empty (models.dev does not
    /// provide them). The UI uses alphabet sort as fallback.
    pub category: String,
    pub description: String,
    pub default_enabled: bool,
}

/// Outcome of a single refresh attempt.
///
/// Why `Result` and not `Result<_, _>`: `RefreshOutcome::Failed` is returned
/// as `Ok(Failed)` on purpose. Lazy on-call refresh should not break the
/// caller's success path with an `Err` — the snapshot still serves stale
/// data. Callers that want to react to failure use `match`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RefreshOutcome {
    /// Snapshot unchanged (HTTP 304, mtime fresh, etc.).
    Unchanged,
    /// Snapshot replaced with newer data.
    Updated {
        provider_count: usize,
        model_count: usize,
    },
    /// No network attempted; served from stale cache or SNAP.
    /// (e.g. `OXI_MODELS_DEV_DISABLE_FETCH=1`, mtime window fresh.)
    Offline { reason: &'static str },
    /// Refresh attempted and failed. Previous snapshot still in effect.
    /// Not an `Err` — see type-level doc above.
    Failed { reason: String },
}

/// Per-entry origin. UI uses this for "local" badges; debugging uses it
/// to trace where an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogSource {
    /// Compile-time embedded SNAP (models.dev gzip).
    Embedded,
    /// Runtime cache from a previous successful live fetch.
    Cache,
    /// Fresh fetch from upstream (e.g. models.dev).
    Live,
    /// Local `/v1/models` discovery (ollama, lmstudio, vllm, sglang).
    Local,
    /// User override file (highest precedence).
    Override,
}

/// Lifecycle event delivered to all subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CatalogEvent {
    /// Snapshot changed. New state available via read methods.
    Updated {
        provider_count: usize,
        model_count: usize,
    },
    /// Refresh failed; previous snapshot still in effect.
    RefreshFailed {
        reason: String,
        provider_count: usize,
        model_count: usize,
    },
    /// User override file applied or modified.
    OverrideApplied {
        path: PathBuf,
        provider_overrides: usize,
        model_overrides: usize,
    },
    /// Local discovery added new models.
    LocalDiscovered {
        base_url: String,
        model_count: usize,
    },
}

// ═══════════════════════════════════════════════════════════════════════════
// Port trait
// ═══════════════════════════════════════════════════════════════════════════

/// Source of truth for provider/model metadata.
///
/// # Threading & lifecycle
///
/// All read methods are async and return owned values. Implementations
/// typically hold a snapshot behind an `Arc<RwLock<_>>`; the trait does
/// not require any particular storage. The snapshot is replaced atomically
/// on refresh.
///
/// # Subscription
///
/// [`subscribe`](Self::subscribe) returns a `broadcast::Receiver` (capacity
/// 16). Slow consumers may miss intermediate updates — the latest state is
/// always available via the read methods, so this is not a correctness
/// issue.
pub trait ModelCatalog: Send + Sync + 'static {
    /// List all known provider IDs (sorted, no duplicates).
    fn list_providers(&self) -> Pin<Box<dyn Future<Output = SdkResult<Vec<String>>> + Send + '_>>;

    /// Look up a single provider by ID.
    fn get_provider(
        &self,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogProviderEntry>>> + Send + '_>>;

    /// List all models for a provider (order unspecified).
    fn list_models(
        &self,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>>;

    /// Look up a single model by `(provider, model_id)`.
    fn get_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogModelEntry>>> + Send + '_>>;

    /// Search models by case-insensitive substring of `provider`, `model_id`,
    /// or `name`. Empty pattern returns all.
    fn search(
        &self,
        pattern: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>>;

    /// Total number of models across all providers.
    fn model_count(&self) -> Pin<Box<dyn Future<Output = SdkResult<usize>> + Send + '_>>;

    /// Force a refresh. The implementation decides what "refresh" means
    /// (HTTP fetch, file re-read, etc.).
    fn refresh(&self) -> Pin<Box<dyn Future<Output = SdkResult<RefreshOutcome>> + Send + '_>>;

    /// Subscribe to catalog lifecycle events. Multiple consumers supported.
    fn subscribe(&self) -> broadcast::Receiver<CatalogEvent>;

    // ── Synchronous read-only API ───────────────────────────────────────
    //
    // The async methods above are the canonical contract, but some callers
    // (TUI event handlers, model pickers) run in a synchronous context and
    // cannot `.await`. The catalog's data already lives in memory behind a
    // `RwLock`, so synchronous reads are cheap and safe — they acquire the
    // read lock, clone the needed entries, and return immediately (no I/O,
    // no allocation of a `Future`).
    //
    // Implementations MUST NOT perform network or file I/O in these
    // methods. They reflect the *currently loaded* snapshot only.

    /// Sync equivalent of [`list_providers`](Self::list_providers).
    fn list_providers_sync(&self) -> Vec<String> {
        Vec::new()
    }

    /// Sync equivalent of [`get_provider`](Self::get_provider).
    fn get_provider_sync(&self, _provider_id: &str) -> Option<CatalogProviderEntry> {
        None
    }

    /// Sync equivalent of [`list_models`](Self::list_models).
    fn list_models_sync(&self, _provider_id: &str) -> Vec<CatalogModelEntry> {
        Vec::new()
    }

    /// Sync equivalent of [`get_model`](Self::get_model).
    fn get_model_sync(&self, _provider_id: &str, _model_id: &str) -> Option<CatalogModelEntry> {
        None
    }

    /// Sync equivalent of [`search`](Self::search).
    fn search_sync(&self, _pattern: &str) -> Vec<CatalogModelEntry> {
        Vec::new()
    }

    /// Sync equivalent of [`model_count`](Self::model_count).
    fn model_count_sync(&self) -> usize {
        0
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Noop default
// ═══════════════════════════════════════════════════════════════════════════

/// Empty catalog — for products that don't need any model metadata
/// (e.g. a single-provider app with hardcoded IDs).
///
/// No `Default` impl — `broadcast::Sender` has no `Default`. Use
/// [`NoopModelCatalog::new`] instead. `Debug` is a manual impl to keep
/// the output clean (the sender is opaque).
pub struct NoopModelCatalog {
    tx: broadcast::Sender<CatalogEvent>,
}

impl NoopModelCatalog {
    pub fn new() -> std::sync::Arc<Self> {
        let (tx, _) = broadcast::channel(16);
        std::sync::Arc::new(Self { tx })
    }
}

impl std::fmt::Debug for NoopModelCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoopModelCatalog").finish_non_exhaustive()
    }
}

impl ModelCatalog for NoopModelCatalog {
    fn list_providers(&self) -> Pin<Box<dyn Future<Output = SdkResult<Vec<String>>> + Send + '_>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn get_provider(
        &self,
        _: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogProviderEntry>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }

    fn list_models(
        &self,
        _: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn get_model(
        &self,
        _: &str,
        _: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Option<CatalogModelEntry>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }

    fn search(
        &self,
        _: &str,
    ) -> Pin<Box<dyn Future<Output = SdkResult<Vec<CatalogModelEntry>>> + Send + '_>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn model_count(&self) -> Pin<Box<dyn Future<Output = SdkResult<usize>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = SdkResult<RefreshOutcome>> + Send + '_>> {
        Box::pin(async { Ok(RefreshOutcome::Unchanged) })
    }

    fn subscribe(&self) -> broadcast::Receiver<CatalogEvent> {
        self.tx.subscribe()
    }
}
