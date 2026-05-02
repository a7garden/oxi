//! Provider abstraction layer

mod trait_def;
mod event;
mod options;
mod openai;
mod anthropic;

pub use trait_def::{Provider, ProviderError};
pub use event::{ProviderEvent, AssistantMessage};
pub use options::{StreamOptions, CacheRetention};
pub use openai::OpenAiProvider;
pub use anthropic::AnthropicProvider;

// Provider registry
use std::collections::HashMap;
use once_cell::sync::Lazy;
use super::Model;

/// Global provider registry
static PROVIDERS: Lazy<HashMap<String, ProviderRegistryEntry>> = Lazy::new(|| {
    let mut map = HashMap::new();
    
    // Register built-in providers
    map.insert("openai".to_string(), ProviderRegistryEntry {
        name: "OpenAI".to_string(),
        create: Box::new(|| Box::new(OpenAiProvider::new()) as Box<dyn Provider>),
    });
    
    map.insert("anthropic".to_string(), ProviderRegistryEntry {
        name: "Anthropic".to_string(),
        create: Box::new(|| Box::new(AnthropicProvider::new()) as Box<dyn Provider>),
    });
    
    map
});

struct ProviderRegistryEntry {
    name: String,
    create: Box<dyn Fn() -> Box<dyn Provider> + Send + Sync>,
}

/// Get a provider by name
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    PROVIDERS.get(name).map(|entry| (entry.create)())
}

/// Get all available provider names
pub fn provider_names() -> Vec<String> {
    PROVIDERS.keys().cloned().collect()
}

/// Get all available providers
pub fn providers() -> Vec<(&'static str, &'static str)> {
    PROVIDERS
        .iter()
        .map(|(k, v)| (k.as_str(), v.name.as_str()))
        .collect()
}

/// Create a stream for a model using the appropriate provider
pub async fn stream(
    model: &Model,
    context: &super::Context,
    options: Option<StreamOptions>,
) -> Result<impl futures::Stream<Item = ProviderEvent> + Send + 'static, ProviderError> {
    let provider = match model.provider.as_str() {
        "openai" => get_provider("openai"),
        "anthropic" => get_provider("anthropic"),
        _ => get_provider(&model.provider),
    };
    
    let provider = provider.ok_or_else(|| {
        ProviderError::UnknownProvider(model.provider.clone())
    })?;
    
    provider.stream(model, context, options).await
}
