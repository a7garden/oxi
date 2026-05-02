//! Model registry for oxi-ai
//!
//! Provides a centralized registry of available LLM models.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use crate::{Api, Model, Cost, InputModality};

/// Global model registry
static MODELS: Lazy<HashMap<String, Model>> = Lazy::new(|| {
    let mut map = HashMap::new();
    
    // OpenAI models
    add_openai_models(&mut map);
    
    // Anthropic models
    add_anthropic_models(&mut map);
    
    // Google models
    add_google_models(&mut map);
    
    map
});

fn add_openai_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("openai/gpt-4o", "GPT-4o", "https://api.openai.com/v1", true),
        ("openai/gpt-4o-mini", "GPT-4o Mini", "https://api.openai.com/v1", true),
        ("openai/gpt-4-turbo", "GPT-4 Turbo", "https://api.openai.com/v1", true),
        ("openai/gpt-3.5-turbo", "GPT-3.5 Turbo", "https://api.openai.com/v1", false),
        ("openai/o1", "OpenAI o1", "https://api.openai.com/v1", true),
        ("openai/o1-mini", "OpenAI o1 Mini", "https://api.openai.com/v1", true),
        ("openai/o3", "OpenAI o3", "https://api.openai.com/v1", true),
        ("openai/o3-mini", "OpenAI o3 Mini", "https://api.openai.com/v1", true),
    ];
    
    for (id, name, url, reasoning) in models {
        map.insert(id.to_string(), Model {
            id: id.split('/').last().unwrap().to_string(),
            name: name.to_string(),
            api: Api::OpenAiCompletions,
            provider: "openai".to_string(),
            base_url: url.to_string(),
            reasoning,
            input: if reasoning { vec![InputModality::Text] } else { vec![InputModality::Text, InputModality::Image] },
            cost: Cost {
                input: if reasoning { 15.0 } else { 2.5 },
                output: if reasoning { 60.0 } else { 10.0 },
                cache_read: 1.25,
                cache_write: 18.75,
            },
            context_window: 128_000,
            max_tokens: 32_000,
            headers: Default::default(),
            compat: None,
        });
    }
}

fn add_anthropic_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("anthropic/claude-sonnet-4-20250514", "Claude Sonnet 4", true),
        ("anthropic/claude-opus-4-20250514", "Claude Opus 4", true),
        ("anthropic/claude-3-5-haiku-20241022", "Claude 3.5 Haiku", false),
        ("anthropic/claude-3-opus", "Claude 3 Opus", false),
        ("anthropic/claude-3-sonnet", "Claude 3 Sonnet", false),
        ("anthropic/claude-3-haiku", "Claude 3 Haiku", false),
    ];
    
    for (id, name, reasoning) in models {
        map.insert(id.to_string(), Model {
            id: id.split('/').last().unwrap().to_string(),
            name: name.to_string(),
            api: Api::AnthropicMessages,
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            reasoning,
            input: vec![InputModality::Text, InputModality::Image],
            cost: Cost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            },
            context_window: 200_000,
            max_tokens: 8192,
            headers: Default::default(),
            compat: None,
        });
    }
}

fn add_google_models(map: &mut HashMap<String, Model>) {
    let models = [
        ("google/gemini-2.0-flash", "Gemini 2.0 Flash"),
        ("google/gemini-2.5-flash", "Gemini 2.5 Flash"),
        ("google/gemini-2.5-pro", "Gemini 2.5 Pro"),
        ("google/gemini-1.5-flash", "Gemini 1.5 Flash"),
        ("google/gemini-1.5-pro", "Gemini 1.5 Pro"),
    ];
    
    for (id, name) in models {
        map.insert(id.to_string(), Model {
            id: id.split('/').last().unwrap().to_string(),
            name: name.to_string(),
            api: Api::GoogleGenerativeAi,
            provider: "google".to_string(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            reasoning: false,
            input: vec![InputModality::Text, InputModality::Image],
            cost: Cost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 1_000_000,
            max_tokens: 8192,
            headers: Default::default(),
            compat: None,
        });
    }
}

/// Model registry
pub struct ModelRegistry;

impl ModelRegistry {
    /// Get a model by provider/model ID
    pub fn get(provider: &str, model_id: &str) -> Option<&'static Model> {
        let key = format!("{}/{}", provider, model_id);
        MODELS.get(&key)
    }
    
    /// Get all models from a provider
    pub fn get_by_provider(provider: &str) -> Vec<&'static Model> {
        MODELS.values()
            .filter(|m| m.provider == provider)
            .collect()
    }
    
    /// Get all available models
    pub fn all() -> Vec<&'static Model> {
        MODELS.values().collect()
    }
    
    /// Search models by pattern
    pub fn search(pattern: &str) -> Vec<&'static Model> {
        let pattern_lower = pattern.to_lowercase();
        MODELS.values()
            .filter(|m| {
                m.id.to_lowercase().contains(&pattern_lower) ||
                m.name.to_lowercase().contains(&pattern_lower)
            })
            .collect()
    }
}

/// Convenience function to get a model
pub fn get_model(provider: &str, model_id: &str) -> Option<&'static Model> {
    ModelRegistry::get(provider, model_id)
}

/// Get all available providers
pub fn get_providers() -> Vec<&'static str> {
    let mut providers: Vec<&'static str> = MODELS.values()
        .map(|m| m.provider.as_str())
        .collect();
    providers.sort();
    providers.dedup();
    providers
}

/// Get all models from a provider
pub fn get_models(provider: &str) -> Vec<&'static Model> {
    ModelRegistry::get_by_provider(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_get_model() {
        let model = get_model("openai", "gpt-4o-mini");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider, "openai");
        assert!(model.reasoning);
    }
    
    #[test]
    fn test_get_providers() {
        let providers = get_providers();
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"google"));
    }
}
