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
pub use options::StreamOptions;
pub use crate::CacheRetention;
pub use crate::Context;
pub use crate::Model;
pub use crate::error::ProviderError;

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
    vec!["openai", "anthropic"]
}

/// Get all available providers with names
pub fn providers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("openai", "OpenAI"),
        ("anthropic", "Anthropic"),
    ]
}

/// Create a stream for a model using the appropriate provider
pub async fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send + 'static>>, ProviderError> {
    let provider = get_provider(&model.provider)
        .ok_or_else(|| ProviderError::UnknownProvider(model.provider.clone()))?;
    
    provider.stream(model, context, options).await
}