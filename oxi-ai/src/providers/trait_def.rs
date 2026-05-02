//! Provider trait definition

use async_trait::async_trait;
use futures::Stream;
use std::sync::Arc;
use super::{Model, Context, StreamOptions, ProviderEvent, ProviderError};
use crate::AssistantMessage;

/// LLM provider trait
///
/// Implement this trait to add support for new LLM providers.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Stream assistant message events
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<impl Stream<Item = ProviderEvent> + Send, ProviderError>;
    
    /// Get the provider name
    fn name(&self) -> &str;
}

/// Get a boxed provider for a model
pub fn provider_for_model(model: &Model) -> Result<Box<dyn Provider>, ProviderError> {
    match model.provider.as_str() {
        "openai" | "azure-openai" | "deepseek" | "groq" | "cerebras" | "xai" | "mistral" | "openrouter" | "fireworks" | "huggingface" => {
            Ok(Box::new(super::openai::OpenAiProvider::new()))
        }
        "anthropic" => {
            Ok(Box::new(super::anthropic::AnthropicProvider::new()))
        }
        "google" | "google-vertex" => {
            // TODO: Implement Google provider
            Err(ProviderError::NotImplemented(format!("Provider '{}' not yet implemented", model.provider)))
        }
        _ => Err(ProviderError::UnknownProvider(model.provider.clone())),
    }
}
