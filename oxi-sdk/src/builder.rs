//! OxiBuilder and Oxi — SDK entry point

use anyhow::Result;
use std::sync::Arc;

use oxi_ai::{Provider, Model};
use oxi_agent::{Agent, AgentLoop, AgentLoopConfig, ToolRegistry};

use crate::agent_builder::AgentBuilder;

/// Oxi AI engine instance.
///
/// Created via [`OxiBuilder`]. Provides access to providers, models,
/// and agent creation.
pub struct Oxi {
    providers: Arc<oxi_ai::providers::ProviderRegistry>,
    models: Arc<oxi_ai::model_registry::ModelRegistry>,
}

impl Oxi {
    /// Create an agent builder with the given config.
    pub fn agent(&self, config: AgentConfig) -> AgentBuilder<'_> {
        AgentBuilder::new(self, config)
    }

    /// Get the provider registry.
    pub fn providers(&self) -> &oxi_ai::providers::ProviderRegistry {
        &self.providers
    }
    
    /// Get the model registry.
    pub fn models(&self) -> &oxi_ai::model_registry::ModelRegistry {
        &self.models
    }
    
    /// Resolve a model ID to a Model.
    pub fn resolve_model(&self, model_id: &str) -> Result<Model> {
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        let (provider, model) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("anthropic", parts[0])
        };
        oxi_ai::model_registry::lookup_model(provider, model)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_id))
    }
    
    /// Create a provider instance.
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        self.providers
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", name))
    }
}

/// Builder for creating an Oxi instance.
pub struct OxiBuilder {
    providers: oxi_ai::providers::ProviderRegistry,
    models: oxi_ai::model_registry::ModelRegistry,
}

impl OxiBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            providers: oxi_ai::providers::ProviderRegistry::new(),
            models: oxi_ai::model_registry::ModelRegistry::new(),
        }
    }
    
    /// Register all built-in providers and models.
    pub fn with_builtins(mut self) -> Self {
        // Register all built-in models from model_registry
        for model in oxi_ai::model_registry::get_models("") {
            let model = model.clone();
            self.models.register(model);
        }
        
        // Note: providers are created on-demand via create_provider()
        // by looking up the model's API type
        self
    }
    
    /// Register a custom provider.
    pub fn provider(mut self, name: &str, provider: impl Provider + 'static) -> Self {
        self.providers.register(name, provider);
        self
    }
    
    /// Register a custom model.
    pub fn model(mut self, model: Model) -> Self {
        self.models.register(model);
        self
    }
    
    /// Build the Oxi instance.
    pub fn build(self) -> Oxi {
        Oxi {
            providers: Arc::new(self.providers),
            models: Arc::new(self.models),
        }
    }
}

impl Default for OxiBuilder {
    fn default() -> Self { Self::new() }
}