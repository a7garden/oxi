//! OxiBuilder and Oxi — SDK entry point

use anyhow::Result;
use std::sync::Arc;

use oxi_ai::{Provider, ProviderRegistry, Model, ModelRegistry};
use oxi_agent::ToolRegistry;

use crate::agent_builder::AgentBuilder;

/// Oxi AI engine instance — holds isolated provider and model registries.
///
/// Created via [`OxiBuilder`]. Provides access to providers, models,
/// provider creation, and agent building.
pub struct Oxi {
    providers: Arc<ProviderRegistry>,
    models: Arc<ModelRegistry>,
    tools: Arc<ToolRegistry>,
}

impl Oxi {
    /// Create an agent builder with the given config.
    pub fn agent(&self, config: oxi_agent::AgentConfig) -> AgentBuilder<'_> {
        AgentBuilder::new(self, config)
    }

    /// Get the provider registry.
    pub fn providers(&self) -> &ProviderRegistry {
        &self.providers
    }

    /// Get the model registry.
    pub fn models(&self) -> &ModelRegistry {
        &self.models
    }

    /// Get the shared tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    /// Resolve a model ID to a Model.
    ///
    /// Accepts `"provider/model"` or bare `"model"` (defaults to "anthropic").
    pub fn resolve_model(&self, model_id: &str) -> Result<Model> {
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        let (provider, model) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("anthropic", parts[0])
        };
        self.models
            .lookup(provider, model)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_id))
    }

    /// Create a provider instance for a given provider name.
    ///
    /// Checks the local `ProviderRegistry` first, then falls back
    /// to built-in providers.
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        self.providers
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", name))
    }
}

/// Builder for creating an Oxi instance.
pub struct OxiBuilder {
    providers: ProviderRegistry,
    models: ModelRegistry,
    tools: ToolRegistry,
}

impl OxiBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            providers: ProviderRegistry::new(),
            models: ModelRegistry::new(),
            tools: ToolRegistry::new(),
        }
    }

    /// Register all built-in models from the oxi-ai static database.
    pub fn with_builtins(mut self) -> Self {
        self.models = ModelRegistry::from_static();
        self
    }

    /// Register a custom provider.
    pub fn provider(self, name: &str, p: impl Provider + 'static) -> Self {
        self.providers.register(name, p);
        self
    }

    /// Register a custom tool.
    pub fn tool(self, tool: impl oxi_agent::AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// Register a custom model.
    pub fn model(self, model: Model) -> Self {
        self.models.register(model);
        self
    }

    /// Build the Oxi instance.
    pub fn build(self) -> Oxi {
        Oxi {
            providers: Arc::new(self.providers),
            models: Arc::new(self.models),
            tools: Arc::new(self.tools),
        }
    }
}

impl Default for OxiBuilder {
    fn default() -> Self {
        Self::new()
    }
}
