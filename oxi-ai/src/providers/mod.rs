//! Provider abstraction layer

use std::sync::OnceLock;

mod anthropic;
mod azure;
mod bedrock;
mod event;
mod google;
mod google_shared;
mod mistral;
mod openai;
mod openai_responses;
pub mod openai_responses_shared;
mod options;
pub mod register_builtins;
mod trait_def;
mod vertex;
pub mod model_fetch;

use futures::Stream;
use std::pin::Pin;

use crate::error::ProviderError;
pub use crate::Api;
pub use crate::CacheRetention;
pub use crate::Context;
pub use crate::Model;
#[allow(unused_imports)]
pub use crate::ThinkingLevel;
#[allow(unused_imports)]
pub use anthropic::AnthropicProvider;
#[allow(unused_imports)]
pub use azure::AzureProvider;
pub use event::ProviderEvent;
#[allow(unused_imports)]
pub use openai::OpenAiProvider;
#[allow(unused_imports)]
pub use openai_responses::OpenAiResponsesProvider;
#[allow(unused_imports)]
pub use options::{StreamOptions, ThinkingBudgets};
pub use trait_def::Provider;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Shared client singleton.
pub fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

// ── Instance-based provider registry ───────────────────────────────

/// Runtime registry for providers (custom + built-in resolution).
///
/// This is an instance-based alternative to the global `CUSTOM_PROVIDERS` static.
/// It supports `register()`, `get()`, `remove()`, and `names()`, falling back
/// to built-in providers from [`register_builtins`] when a name isn't found locally.
#[derive(Default)]
pub struct ProviderRegistry {
    custom: RwLock<HashMap<String, Arc<dyn Provider>>>,
}

impl ProviderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            custom: RwLock::new(HashMap::new()),
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
    /// Checks local custom providers first, then falls back to built-in providers
    /// from [`register_builtins`].
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        // 1. Check local custom providers
        {
            let guard = self.custom.read();
            if let Some(provider) = guard.get(name) {
                return Some(Arc::clone(provider));
            }
        }

        // 2. Fall back to built-in providers
        get_provider(name).map(|boxed| Arc::from(boxed))
    }
}

// ── Global custom provider registry (legacy) ───────────────────────

/// Global custom provider registry (for backward compatibility with CLI).
///
/// Custom providers registered via [`register_provider`] are stored here
/// and take priority over built-in providers in [`get_provider`].
static CUSTOM_PROVIDERS: Lazy<RwLock<HashMap<String, Arc<dyn Provider>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

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
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    // 1. Check custom providers first (higher priority than builtins)
    {
        let custom = CUSTOM_PROVIDERS.read();
        if let Some(provider) = custom.get(name) {
            // Clone the Arc and wrap in a newtype that implements Provider via Arc delegation
            return Some(Box::new(ArcedProvider(provider.clone())));
        }
    }

    // 2. Built-in providers: look up metadata from registry
    let builtin = register_builtins::get_builtin_provider(name)?;

    match builtin.api {
        Api::AnthropicMessages => Some(Box::new(anthropic::AnthropicProvider::new())),
        Api::GoogleGenerativeAi => Some(Box::new(google::GoogleProvider::new())),
        Api::GoogleVertex => Some(Box::new(vertex::VertexProvider::new())),
        Api::MistralConversations => Some(Box::new(mistral::MistralProvider::new())),
        Api::AzureOpenAiResponses => Some(Box::new(azure::AzureProvider::new())),
        Api::BedrockConverseStream => Some(Box::new(bedrock::BedrockProvider::new())),
        Api::OpenAiCompletions => {
            // All OpenAI-compatible providers use OpenAiProvider with a custom base_url
            if builtin.base_url.is_empty() {
                Some(Box::new(openai::OpenAiProvider::new()))
            } else {
                Some(Box::new(openai::OpenAiProvider::with_base_url(
                    builtin.base_url,
                )))
            }
        }
        Api::OpenAiResponses => Some(Box::new(openai_responses::OpenAiResponsesProvider::new())),
    }
}

/// Wrapper that lets us return a cloned `Arc<dyn Provider>` as `Box<dyn Provider>`.
struct ArcedProvider(Arc<dyn Provider>);

#[async_trait::async_trait]
impl Provider for ArcedProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        self.0.stream(model, context, options).await
    }

    fn name(&self) -> &str {
        self.0.name()
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
