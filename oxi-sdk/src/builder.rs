//! OxiBuilder and Oxi — SDK entry point

use anyhow::Result;
use std::sync::Arc;

use oxi_agent::{ProviderResolver, ToolRegistry};
use oxi_ai::{Model, ModelRegistry, Provider, ProviderRegistry};

use crate::agent_builder::AgentBuilder;
use crate::multi_provider::{MultiProviderBuilder, RoutingConfig};

/// Oxi AI engine instance — holds isolated provider and model registries.
///
/// Created via [`OxiBuilder`]. Provides access to providers, models,
/// provider creation, and agent building.
///
/// Implements [`ProviderResolver`] so it can be passed directly to
/// [`oxi_agent::Agent::new_with_resolver`] for fully isolated operation.
pub struct Oxi {
    providers: Arc<ProviderRegistry>,
    models: Arc<ModelRegistry>,
    tools: Arc<ToolRegistry>,
    /// Whether to include built-in provider resolution (from create_builtin_provider).
    include_builtins: bool,
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
    /// to built-in providers (if `with_builtins()` was called).
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        // 1. Check custom providers registered via OxiBuilder::provider()
        if let Some(p) = self.providers.get_custom(name) {
            return Ok(p);
        }
        // 2. Fall back to built-in providers (stateless creation)
        if self.include_builtins {
            if let Some(p) = oxi_ai::create_builtin_provider(name) {
                return Ok(Arc::from(p));
            }
        }
        Err(anyhow::anyhow!("Provider '{}' not found", name))
    }

    /// Get the provider registry (Arc clone).
    pub fn providers_arc(&self) -> Arc<ProviderRegistry> {
        Arc::clone(&self.providers)
    }

    /// Get the model registry (Arc clone).
    pub fn models_arc(&self) -> Arc<ModelRegistry> {
        Arc::clone(&self.models)
    }

    /// Check whether built-in providers are enabled.
    pub fn has_builtins(&self) -> bool {
        self.include_builtins
    }
}

/// Implement ProviderResolver so Oxi can be used as Agent's resolver.
impl ProviderResolver for Oxi {
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.create_provider(name).ok()
    }

    fn resolve_model(&self, model_id: &str) -> Option<Model> {
        self.resolve_model(model_id).ok()
    }
}

/// Builder for creating an Oxi instance.
pub struct OxiBuilder {
    providers: ProviderRegistry,
    models: ModelRegistry,
    tools: ToolRegistry,
    include_builtins: bool,
}

impl OxiBuilder {
    /// Create a new empty builder (no builtins, no providers, no models).
    pub fn new() -> Self {
        Self {
            providers: ProviderRegistry::new(),
            models: ModelRegistry::new(),
            tools: ToolRegistry::new(),
            include_builtins: false,
        }
    }

    /// Register all built-in models and enable built-in provider creation.
    ///
    /// This loads 50+ model definitions from the oxi-ai static database
    /// and enables `create_builtin_provider()` fallback in [`Oxi::create_provider`].
    pub fn with_builtins(mut self) -> Self {
        self.models = ModelRegistry::from_static();
        self.include_builtins = true;
        self
    }

    /// Register a custom provider.
    pub fn provider(self, name: &str, p: impl Provider + 'static) -> Self {
        self.providers.register(name, p);
        self
    }

    /// Register a custom tool in the shared tool registry.
    pub fn tool(self, tool: impl oxi_agent::AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// Register a custom model.
    pub fn model(self, model: Model) -> Self {
        self.models.register(model);
        self
    }

    /// Enable multi-provider routing with automatic complexity-based model selection.
    ///
    /// This registers a [`MultiProvider`] that routes requests based on task complexity,
    /// with configurable fallback chains and circuit breaker protection.
    ///
    /// # Arguments
    ///
    /// * `config` - Routing configuration (use [`RoutingConfig::new()`] for defaults)
    ///
    /// # Example
    ///
    /// ```ignore
    /// use oxi_sdk::{RoutingConfig, create_builtin_provider};
    ///
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .enable_routing(RoutingConfig::new().prefer_cost_efficient(true))
    ///     .build();
    /// ```
    pub fn enable_routing(self, config: RoutingConfig) -> Self {
        // Collect providers before consuming self
        let provider_names: Vec<String> = self.providers.names();
        let mut providers_to_add: Vec<(String, Arc<dyn Provider>)> = Vec::new();
        for name in &provider_names {
            if let Some(provider) = self.providers.get_custom(name) {
                providers_to_add.push((name.clone(), provider));
            }
        }

        // Build multi-provider with registered providers and routing config
        let mut mp_builder = MultiProviderBuilder::new();

        // Apply routing config
        if config.auto_routing {
            mp_builder = mp_builder.enable_auto_routing();
        }
        if config.prefer_cost_efficient {
            mp_builder = mp_builder.prefer_cost_efficient();
        }
        if let Some(router) = config.router {
            mp_builder = mp_builder.with_router_boxed(router);
        }

        // Add collected providers
        for (name, provider) in providers_to_add {
            mp_builder = mp_builder.provider(&name, provider);
        }

        // Build and register the multi-provider
        let built = mp_builder.build();
        if let Ok(mp) = built {
            self.providers.register_arc("multi", mp);
        }
        self
    }

    /// Build the Oxi engine instance.
    pub fn build(self) -> Oxi {
        Oxi {
            providers: Arc::new(self.providers),
            models: Arc::new(self.models),
            tools: Arc::new(self.tools),
            include_builtins: self.include_builtins,
        }
    }
}

impl Default for OxiBuilder {
    fn default() -> Self {
        Self::new()
    }
}
