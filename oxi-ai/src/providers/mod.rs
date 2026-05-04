//! Provider abstraction layer

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

/// Provider factory functions

/// Get a provider by name
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
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
        "cloudflare",
        "copilot",
        "openai-responses",
        "openai-completions",
        "codex",
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
        ("cloudflare", "Cloudflare Workers AI"),
        ("copilot", "GitHub Copilot"),
        ("openai-responses", "OpenAI Responses API"),
        ("openai-completions", "OpenAI Completions API (Legacy)"),
        ("codex", "GitHub Codex"),
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
