//! Provider abstraction layer

use std::sync::OnceLock;

mod anthropic;
mod azure;
mod bedrock;
mod event;
mod google;
mod google_shared;
mod mistral;
pub mod model_fetch;
mod openai;
mod openai_responses;
pub mod openai_responses_shared;
mod options;
pub mod register_builtins;
#[allow(unused_imports)]
pub use register_builtins::AuthMethod;
#[allow(unused_imports)]
pub use register_builtins::create_builtin_provider_with_options;
mod trait_def;
mod vertex;

use futures::Stream;
use std::pin::Pin;

#[allow(unused_imports)]
pub use crate::Api;
pub use crate::CacheRetention;
pub use crate::Context;
pub use crate::Model;
#[allow(unused_imports)]
pub use crate::ThinkingLevel;
use crate::error::ProviderError;
#[allow(unused_imports)]
pub use anthropic::AnthropicProvider;
#[allow(unused_imports)]
pub use azure::AzureProvider;
#[allow(unused_imports)]
pub use bedrock::BedrockProvider;
pub use event::FallbackReason;
pub use event::ProviderEvent;
#[allow(unused_imports)]
pub use google::GoogleProvider;
#[allow(unused_imports)]
pub use mistral::MistralProvider;
#[allow(unused_imports)]
pub use openai::OpenAiProvider;
pub use openai::normalize_messages;
#[allow(unused_imports)]
pub use openai_responses::OpenAiResponsesProvider;
#[allow(unused_imports)]
pub use options::{ProviderOptions, StreamOptions, ThinkingBudgets};
pub use trait_def::{Provider, StreamResult};
#[allow(unused_imports)]
pub use vertex::VertexProvider;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

/// Default HTTP client timeout for LLM provider streams.
///
/// Long enough for multi-minute reasoning responses, short enough that a
/// stalled upstream (TLS handshake hang, dead proxy, etc.) surfaces to the
/// caller within a usable window. Connect failures fail fast (10s) — the
/// full timeout covers the entire request body including stream read.
const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 600;
const DEFAULT_PROVIDER_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Shared client singleton.
///
/// All eight built-in providers (`AnthropicProvider`, `OpenAiProvider`,
/// `GoogleProvider`, etc.) construct their `reqwest::Client` via this
/// accessor. A bare `reqwest::Client::new()` would have **no timeout**,
/// which means a stalled TCP/TLS handshake or a silent upstream hang
/// would freeze the CLI indefinitely. With `panic = "abort"` in the
/// release profile the user has to kill the process to recover.
///
/// `oxi-cli/src/util/http_client.rs::shared_http_client` follows the same
/// pattern (30s timeout) for non-LLM HTTP. Keep these two in sync.
pub fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(
                DEFAULT_PROVIDER_CONNECT_TIMEOUT_SECS,
            ))
            .timeout(std::time::Duration::from_secs(
                DEFAULT_PROVIDER_TIMEOUT_SECS,
            ))
            .build()
            .expect("provider shared_client: reqwest builder should not fail")
    })
}

// ── Instance-based provider registry ───────────────────────────────

/// Type alias for the provider factory closure stored in [`ProviderRegistry`].
pub type ProviderFactory = Box<dyn Fn() -> anyhow::Result<Arc<dyn Provider>> + Send + Sync>;

/// Runtime registry for providers (custom + built-in resolution).
///
/// This is an instance-based alternative to the global `CUSTOM_PROVIDERS` static.
/// It supports `register()`, `get()`, `remove()`, and `names()`, falling back
/// to built-in providers from the built-in provider factory when a name isn't found locally.
///
/// Providers can also be registered as **factories** via [`Self::register_factory`].
/// A factory is a closure that lazily creates the provider on first access. The
/// result is cached, so the factory runs at most once per name.
pub struct ProviderRegistry {
    custom: RwLock<HashMap<String, Arc<dyn Provider>>>,
    factories: RwLock<HashMap<String, ProviderFactory>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            custom: RwLock::new(HashMap::new()),
            factories: RwLock::new(HashMap::new()),
        }
    }

    /// Register a custom provider.
    pub fn register(&self, name: &str, provider: impl Provider + 'static) {
        self.custom
            .write()
            .insert(name.to_string(), Arc::new(provider));
    }

    /// Register a pre-boxed provider.
    pub fn register_arc(&self, name: &str, provider: Arc<dyn Provider>) {
        self.custom.write().insert(name.to_string(), provider);
    }

    /// Remove a previously registered custom provider.
    pub fn remove(&self, name: &str) {
        self.custom.write().remove(name);
    }

    /// Return the set of currently registered custom provider names.
    pub fn names(&self) -> Vec<String> {
        self.custom.read().keys().cloned().collect()
    }

    /// Get a provider by name.
    ///
    /// Checks local custom providers first, then falls back to built-in providers.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        // 1. Check local custom providers
        {
            let guard = self.custom.read();
            if let Some(provider) = guard.get(name) {
                return Some(Arc::clone(provider));
            }
        }

        // 2. Fall back to built-in providers
        get_provider(name).map(Arc::from)
    }

    /// Get a provider by name, checking only custom providers (no built-in fallback).
    ///
    /// If the provider was registered via [`Self::register_factory`] and hasn't
    /// been materialized yet, the factory is invoked and the result cached.
    pub fn get_custom(&self, name: &str) -> Option<Arc<dyn Provider>> {
        {
            let guard = self.custom.read();
            if let Some(provider) = guard.get(name) {
                return Some(Arc::clone(provider));
            }
        }
        // Try materializing from factory
        self.materialize_factory(name)
    }

    /// Register a factory closure that lazily creates a provider on first access.
    ///
    /// When [`Self::get_custom`] or [`Self::get`] is called and the provider is
    /// not yet in `custom`, the factory is invoked, the result is cached in
    /// `custom`, and the factory entry is removed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// registry.register_factory("my_provider", || {
    ///     let key = resolve_api_key("my_provider");
    ///     Ok(Arc::new(MyProvider::new(key)))
    /// });
    /// ```
    pub fn register_factory(
        &self,
        name: &str,
        factory: impl Fn() -> anyhow::Result<Arc<dyn Provider>> + Send + Sync + 'static,
    ) {
        self.factories
            .write()
            .insert(name.to_string(), Box::new(factory));
    }

    /// Invoke a registered factory (if any) for the given name.
    ///
    /// On success the resulting provider is cached in `custom` and the factory
    /// entry is removed. Returns `None` if no factory is registered.
    fn materialize_factory(&self, name: &str) -> Option<Arc<dyn Provider>> {
        let factory = {
            let mut factories = self.factories.write();
            factories.remove(name)?
        };
        match factory() {
            Ok(provider) => {
                self.custom
                    .write()
                    .insert(name.to_string(), Arc::clone(&provider));
                Some(provider)
            }
            Err(e) => {
                tracing::warn!(provider = name, error = %e, "Provider factory failed");
                None
            }
        }
    }
}

// ── Global custom provider registry (legacy) ───────────────────────

/// Global custom provider registry (for backward compatibility with CLI).
///
/// Custom providers registered via [`register_provider`] are stored here
/// and take priority over built-in providers in [`get_provider`].
static CUSTOM_PROVIDERS: LazyLock<RwLock<HashMap<String, Arc<dyn Provider>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a custom provider at runtime (global registry).
///
/// This is called from `oxi-cli` during startup for each `[[custom_provider]]` entry
/// found in settings.
pub fn register_provider(name: &str, provider: impl Provider + 'static) {
    CUSTOM_PROVIDERS
        .write()
        .insert(name.to_string(), Arc::new(provider));
}

/// Unregister a previously registered custom provider (global registry).
pub fn unregister_provider(name: &str) {
    CUSTOM_PROVIDERS.write().remove(name);
}

/// Return the set of currently registered custom provider names (global registry).
pub fn custom_provider_names() -> Vec<String> {
    CUSTOM_PROVIDERS.read().keys().cloned().collect()
}

/// Get a provider by name
///
/// Checks custom providers first (global registry), then falls back to the
/// data-driven built-in provider factory.
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    // 1. Check custom providers first (higher priority than builtins)
    {
        let custom = CUSTOM_PROVIDERS.read();
        if let Some(provider) = custom.get(name) {
            return Some(Box::new(ArcedProvider(provider.clone())));
        }
    }

    // 2. Fall back to built-in provider factory (data-driven from BuiltinProvider metadata)
    register_builtins::create_builtin_provider(name)
}

/// Get a provider by name, returning Arc (for router delegation).
pub fn get_provider_arc(name: &str) -> Option<Arc<dyn Provider>> {
    {
        let custom = CUSTOM_PROVIDERS.read();
        if let Some(provider) = custom.get(name) {
            return Some(Arc::clone(provider));
        }
    }
    register_builtins::create_builtin_provider(name).map(Arc::from)
}

/// Wrapper that lets us return a cloned `Arc<dyn Provider>` as `Box<dyn Provider>`.
struct ArcedProvider(Arc<dyn Provider>);

impl Provider for ArcedProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move { self.0.stream(model, context, options).await })
    }

    fn name(&self) -> &str {
        self.0.name()
    }
}

/// Wrapper that attaches the canonical catalog provider id to a transport.
///
/// Fixes the provider identity collapse where a transport like `OpenAiProvider`
/// hardcoded `name() -> "openai"` regardless of which catalog provider it
/// served (so `create_builtin_provider("deepseek").name()` returned `"openai"`).
/// The wrapped provider reports the correct catalog id via `name()` while
/// delegating streaming to the inner transport unchanged.
///
/// This is a stepping stone toward the omp three-way split (transport carries
/// no identity; identity lives in a `ProviderDefinition` registry) — see
/// `docs/superpowers/specs/2026-07-27-omp-realignment-design.md` (P0.3).
pub(crate) struct NamedProvider {
    id: &'static str,
    inner: Box<dyn Provider>,
}

impl NamedProvider {
    /// Wrap `inner` so its reported `name()` is `id` (the catalog provider id),
    /// not the inner transport's hardcoded protocol-family name.
    pub(crate) fn wrap(id: &'static str, inner: Box<dyn Provider>) -> Box<dyn Provider> {
        Box::new(Self { id, inner })
    }
}

impl Provider for NamedProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        self.inner.stream(model, context, options)
    }

    fn name(&self) -> &str {
        self.id
    }
}

/// Create a stream for a model using the appropriate provider
pub async fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
    let provider = get_provider(&model.provider)
        .ok_or_else(|| ProviderError::UnknownProvider(model.provider.clone()))?;

    provider.stream(model, context, options).await
}
