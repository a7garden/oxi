//! Provider abstraction layer

mod trait_def;
mod event;
mod options;
mod openai;
mod anthropic;

use std::pin::Pin;
use futures::Stream;

pub use trait_def::Provider;
pub use event::ProviderEvent;
use crate::error::ProviderError;
pub use options::{StreamOptions, ThinkingBudgets};
pub use openai::OpenAiProvider;
pub use anthropic::AnthropicProvider;
pub use crate::CacheRetention;
pub use crate::ThinkingLevel;
pub use crate::Context;
pub use crate::Model;
pub use crate::messages::AssistantMessage;

/// Provider factory functions

/// Get a provider by name
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    match name {
        "openai" | "azure-openai" | "deepseek" | "groq" | "cerebras" | "xai" | "mistral" | "openrouter" | "fireworks" | "huggingface" => {
            Some(Box::new(openai::OpenAiProvider::new()))
        }
        "anthropic" => {
            Some(Box::new(anthropic::AnthropicProvider::new()))
        }
        _ => None,
    }
}

/// Get all available provider names
pub fn provider_names() -> Vec<&'static str> {
    vec![
        "openai",
        "anthropic",
        "google",
        "deepseek",
        "mistral",
        "groq",
        "cerebras",
        "xai",
        "openrouter",
        "azure-openai",
    ]
}

/// Get all available providers with names
pub fn providers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("openai", "OpenAI"),
        ("anthropic", "Anthropic"),
        ("google", "Google"),
        ("deepseek", "DeepSeek"),
        ("mistral", "Mistral"),
        ("groq", "Groq"),
        ("cerebras", "Cerebras"),
        ("xai", "xAI"),
        ("openrouter", "OpenRouter"),
        ("azure-openai", "Azure OpenAI"),
    ]
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