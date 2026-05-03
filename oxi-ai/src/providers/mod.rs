//! Provider abstraction layer

mod trait_def;
mod event;
mod options;
mod openai;
mod anthropic;
mod google;
mod vertex;
mod deepseek;
mod bedrock;
mod azure;

use std::pin::Pin;
use futures::Stream;

pub use trait_def::Provider;
pub use event::ProviderEvent;
use crate::error::ProviderError;
#[allow(unused_imports)]
pub use options::{StreamOptions, ThinkingBudgets};
#[allow(unused_imports)]
pub use openai::OpenAiProvider;
#[allow(unused_imports)]
pub use anthropic::AnthropicProvider;
#[allow(unused_imports)]
pub use azure::AzureProvider;
pub use crate::CacheRetention;
#[allow(unused_imports)]
pub use crate::ThinkingLevel;
pub use crate::Context;
pub use crate::Model;

/// Provider factory functions

/// Get a provider by name
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    match name {
        "openai" | "groq" | "cerebras" | "xai" | "openrouter" | "fireworks" | "huggingface" => {
            Some(Box::new(openai::OpenAiProvider::new()))
        }
        "azure" | "azure-openai" => {
            Some(Box::new(azure::AzureProvider::new()))
        }
        "anthropic" => {
            Some(Box::new(anthropic::AnthropicProvider::new()))
        }
        "google" => {
            Some(Box::new(google::GoogleProvider::new()))
        }
        "vertex" | "google-vertex" => {
            Some(Box::new(vertex::VertexProvider::new()))
        }
        "deepseek" => {
            Some(Box::new(deepseek::DeepSeekProvider::new()))
        }
        "mistral" => {
            // Mistral is OpenAI-compatible with minor differences
            Some(Box::new(deepseek::DeepSeekProvider::with_api_key(
                std::env::var("MISTRAL_API_KEY").unwrap_or_default()
            )))
        }
        "bedrock" | "amazon-bedrock" | "aws-bedrock" => {
            Some(Box::new(bedrock::BedrockProvider::new()))
        }
        _ => None,
    }
}

/// Get all available provider names
#[allow(dead_code)]
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
        "azure",
        "vertex",
        "bedrock",
    ]
}

/// Get all available providers with names
#[allow(dead_code)]
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
        ("azure", "Azure OpenAI"),
        ("vertex", "Google Vertex AI"),
        ("bedrock", "Amazon Bedrock"),
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