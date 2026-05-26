//! OxiBuilder and Oxi — SDK entry point

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use oxi_agent::{ProviderResolver, ToolRegistry};
use oxi_ai::{Model, ModelRegistry, Provider, ProviderRegistry};

use crate::agent_builder::AgentBuilder;
use crate::lifecycle::{AgentSupervisor, FileSnapshotStore, SupervisorPolicy};
use crate::multi_provider::{MultiProviderBuilder, RoutingConfig};
use crate::observability::{AuditLog, CostTracker, Tracer};
use crate::security::Authorizer;

/// Oxi AI engine instance — holds isolated provider and model registries.
///
/// Created via [`OxiBuilder`]. Provides access to providers, models,
/// provider creation, and agent building.
///
/// Implements [`ProviderResolver`] so it can be passed directly to
/// [`oxi_agent::Agent::new_with_resolver`] for fully isolated operation.
#[derive(Clone)]
pub struct Oxi {
    providers: Arc<ProviderRegistry>,
    models: Arc<ModelRegistry>,
    tools: Arc<ToolRegistry>,
    /// Whether to include built-in provider resolution (from create_builtin_provider).
    include_builtins: bool,
    /// Per-provider API keys (take precedence over environment variables).
    api_keys: Arc<HashMap<String, String>>,
    /// Per-provider base URL overrides.
    base_urls: Arc<HashMap<String, String>>,
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
    /// Resolution order:
    /// 1. Custom providers registered via `OxiBuilder::provider()`
    /// 2. Provider factories registered via `OxiBuilder::provider_factory()`
    /// 3. Built-in providers with credential injection (if `with_builtins()` was called)
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        // 1. Check custom providers registered via OxiBuilder::provider()
        if let Some(p) = self.providers.get_custom(name) {
            return Ok(p);
        }
        // 2. Fall back to built-in providers (with optional credential injection)
        if self.include_builtins {
            let api_key = self.api_keys.get(name).map(|s| s.as_str());
            let base_url = self.base_urls.get(name).map(|s| s.as_str());
            if let Some(p) = oxi_ai::create_builtin_provider_with_options(name, api_key, base_url) {
                return Ok(Arc::from(p));
            }
            // Fallback to default built-in creation (no credential override)
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
    api_keys: HashMap<String, String>,
    base_urls: HashMap<String, String>,
}

impl OxiBuilder {
    /// Create a new empty builder (no builtins, no providers, no models).
    pub fn new() -> Self {
        Self {
            providers: ProviderRegistry::new(),
            models: ModelRegistry::new(),
            tools: ToolRegistry::new(),
            include_builtins: false,
            api_keys: HashMap::new(),
            base_urls: HashMap::new(),
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

    /// Register a provider factory — a closure that lazily creates a provider.
    ///
    /// Unlike [`Self::provider()`], which takes an already-constructed instance,
    /// this stores a factory closure. The factory is invoked the **first time**
    /// `Oxi::create_provider(name)` is called, and the resulting provider is
    /// cached for subsequent calls.
    ///
    /// This is useful when provider construction requires credential resolution
    /// or network configuration that should happen at first use, not at build time.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .provider_factory("zai", || {
    ///         let api_key = resolve_key("zai");
    ///         let base_url = env::var("ZAI_BASE_URL").unwrap_or_default();
    ///         Ok(Arc::new(OpenAiProvider::with_base_url_and_key(&base_url, api_key)))
    ///     })
    ///     .build();
    /// ```
    pub fn provider_factory(
        self,
        name: &str,
        factory: impl Fn() -> anyhow::Result<Arc<dyn Provider>> + Send + Sync + 'static,
    ) -> Self {
        self.providers.register_factory(name, factory);
        self
    }

    /// Register an API key for a specific provider.
    ///
    /// When `create_provider(name)` is called, the key is injected into
    /// the provider's constructor automatically. Keys registered here
    /// take precedence over environment variables.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .api_key("anthropic", "sk-ant-...")
    ///     .api_key("openai", "sk-...")
    ///     .build();
    /// ```
    pub fn api_key(mut self, provider_name: &str, key: impl Into<String>) -> Self {
        self.api_keys.insert(provider_name.to_string(), key.into());
        self
    }

    /// Register a base URL override for a specific provider.
    ///
    /// Useful for OpenAI-compatible providers (ZAI, Groq, etc.)
    /// that use a different endpoint.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .base_url("openai", "https://my-proxy.example.com/v1")
    ///     .build();
    /// ```
    pub fn base_url(mut self, provider_name: &str, url: impl Into<String>) -> Self {
        self.base_urls.insert(provider_name.to_string(), url.into());
        self
    }

    /// Register a full credential set for a provider.
    ///
    /// Convenience method combining [`api_key()`](Self::api_key) and
    /// [`base_url()`](Self::base_url).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .credential("openai", "sk-...", Some("https://proxy.example.com/v1"))
    ///     .build();
    /// ```
    pub fn credential(
        self,
        provider_name: &str,
        api_key: impl Into<String>,
        base_url: Option<&str>,
    ) -> Self {
        let mut builder = self.api_key(provider_name, api_key);
        if let Some(url) = base_url {
            builder = builder.base_url(provider_name, url);
        }
        builder
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

    /// Create a supervisor builder for managing agent lifecycles.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let supervisor = oxi.supervisor()
    ///     .snapshot_dir("/data/snapshots")
    ///     .build()?;
    /// ```
    pub fn supervisor(self) -> SupervisorBuilder {
        SupervisorBuilder {
            oxi_builder: self,
            policy: SupervisorPolicy::default(),
            snapshot_dir: None,
            audit: None,
            authorizer: None,
            tracer: None,
            cost_tracker: None,
        }
    }

    /// Build the Oxi engine instance.
    pub fn build(self) -> Oxi {
        Oxi {
            providers: Arc::new(self.providers),
            models: Arc::new(self.models),
            tools: Arc::new(self.tools),
            include_builtins: self.include_builtins,
            api_keys: Arc::new(self.api_keys),
            base_urls: Arc::new(self.base_urls),
        }
    }
}

impl Default for OxiBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── SupervisorBuilder ──────────────────────────────────────────────────────

/// Builder for creating an `AgentSupervisor`.
///
/// Created via [`OxiBuilder::supervisor()`].
pub struct SupervisorBuilder {
    oxi_builder: OxiBuilder,
    policy: SupervisorPolicy,
    snapshot_dir: Option<std::path::PathBuf>,
    audit: Option<Arc<AuditLog>>,
    authorizer: Option<Arc<Authorizer>>,
    tracer: Option<Arc<Tracer>>,
    cost_tracker: Option<Arc<CostTracker>>,
}

impl SupervisorBuilder {
    /// Set the restart policy.
    pub fn policy(mut self, policy: SupervisorPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set the directory for persisting snapshots.
    pub fn snapshot_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.snapshot_dir = Some(dir.into());
        self
    }

    /// Attach an audit log.
    pub fn with_audit(mut self, audit: Arc<AuditLog>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Attach an authorizer.
    pub fn with_authorizer(mut self, authorizer: Arc<Authorizer>) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    /// Attach a tracer.
    pub fn with_tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attach a cost tracker.
    pub fn with_cost_tracker(mut self, tracker: Arc<CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }

    /// Build the supervisor.
    ///
    /// Creates an `Oxi` instance internally and constructs the supervisor
    /// with a file-based snapshot store.
    pub fn build(self) -> anyhow::Result<(Oxi, AgentSupervisor)> {
        let oxi = self.oxi_builder.build();
        let resolver: Arc<dyn oxi_agent::ProviderResolver> = Arc::new(oxi.clone());

        let snapshot_store: Arc<dyn crate::lifecycle::SnapshotStore> = match &self.snapshot_dir {
            Some(dir) => Arc::new(FileSnapshotStore::new(dir)?),
            None => Arc::new(FileSnapshotStore::new(
                std::env::temp_dir().join("oxi-snapshots"),
            )?),
        };

        let supervisor = AgentSupervisor::with_policy(resolver, snapshot_store, self.policy);
        Ok((oxi, supervisor))
    }
}
