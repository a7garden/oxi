//! Provider abstraction layer

use std::sync::OnceLock;

mod anthropic;
mod azure;
mod bedrock;
mod cloudflare;
mod copilot;
mod codex;
mod deepseek;
mod event;
mod google;
mod google_shared;
mod mistral;
mod openai;
mod openai_completions;
mod openai_responses;
pub mod openai_responses_shared;
mod options;
pub mod register_builtins;
mod trait_def;
mod vertex;

use futures::Stream;
use std::pin::Pin;

use crate::error::ProviderError;
pub use crate::CacheRetention;
pub use crate::Context;
pub use crate::Model;
#[allow(unused_imports)]
pub use crate::ThinkingLevel;
#[allow(unused_imports)]
pub use anthropic::AnthropicProvider;
#[allow(unused_imports)]
pub use azure::AzureProvider;
#[allow(unused_imports)]
pub use cloudflare::CloudflareProvider;
#[allow(unused_imports)]
pub use codex::CodexProvider;
#[allow(unused_imports)]
pub use copilot::CopilotProvider;
pub use event::ProviderEvent;
#[allow(unused_imports)]
pub use openai::OpenAiProvider;
#[allow(unused_imports)]
pub use openai_completions::OpenAICompletionsProvider;
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

// ── Custom provider registry ────────────────────────────────────────

/// Runtime registry for custom (user-defined) providers.
///
/// Custom providers are registered from `~/.oxi/settings.toml` `[[custom_provider]]`
/// sections during startup.  They take priority over the built-in match arm in
/// [`get_provider`].
static CUSTOM_PROVIDERS: Lazy<RwLock<HashMap<String, Arc<dyn Provider>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Register a custom provider at runtime.
///
/// This is called from `oxi-cli` during startup for each `[[custom_provider]]` entry
/// found in settings.
pub fn register_provider(name: &str, provider: impl Provider + 'static) {
    CUSTOM_PROVIDERS
        .write()
        .insert(name.to_string(), Arc::new(provider));
}

/// Unregister a previously registered custom provider.
pub fn unregister_provider(name: &str) {
    CUSTOM_PROVIDERS.write().remove(name);
}

/// Return the set of currently registered custom provider names.
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

    // 2. Built-in providers
    match name {
        "openai" | "groq" | "cerebras" | "xai" | "openrouter" | "fireworks" | "huggingface" => {
            Some(Box::new(openai::OpenAiProvider::new()))
        }
        "azure" | "azure-openai" => Some(Box::new(azure::AzureProvider::new())),
        "anthropic" => Some(Box::new(anthropic::AnthropicProvider::new())),
        "google" => Some(Box::new(google::GoogleProvider::new())),
        "vertex" | "google-vertex" => Some(Box::new(vertex::VertexProvider::new())),
        "deepseek" => Some(Box::new(deepseek::DeepSeekProvider::new())),
        "mistral" => Some(Box::new(mistral::MistralProvider::new())),
        "bedrock" | "amazon-bedrock" | "aws-bedrock" => {
            Some(Box::new(bedrock::BedrockProvider::new()))
        }
        "cloudflare" | "workers-ai" => Some(Box::new(cloudflare::CloudflareProvider::new())),
        "copilot" | "github-copilot" => Some(Box::new(copilot::CopilotProvider::new())),
        "openai-responses" => Some(Box::new(openai_responses::OpenAiResponsesProvider::new())),
        "openai-completions" | "completions" => Some(Box::new(openai_completions::OpenAICompletionsProvider::new())),
        "codex" | "github-codex" | "copilot-codex" => Some(Box::new(codex::CodexProvider::new())),
        _ => None,
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
