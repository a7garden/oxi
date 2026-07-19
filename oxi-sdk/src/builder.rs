//! OxiBuilder and Oxi — SDK entry point

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use oxi_agent::{ProviderResolver, ToolRegistry};
use oxi_ai::{Model, ModelRegistry, Provider, ProviderRegistry};

use crate::agent_builder::AgentBuilder;
use crate::lifecycle::{AgentSupervisor, FileSnapshotStore, SupervisorPolicy};
use crate::multi_provider::{MultiProviderBuilder, RoutingConfig};
use crate::ports::PortRegistry;

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
    /// Whether built-in providers are enabled (`OxiBuilder::with_builtins`).
    include_builtins: bool,
    /// Per-provider API key overrides (`OxiBuilder::api_key`).
    api_keys: Arc<HashMap<String, String>>,
    /// Per-provider base URL overrides (`OxiBuilder::base_url`).
    base_urls: Arc<HashMap<String, String>>,
    /// Port registry (None = use noop default).
    ports: PortRegistry,
    /// MCP manager (Phase 1+). `None` if MCP is disabled or has not been
    /// spawned yet.
    mcp_manager: Option<Arc<oxi_agent::mcp::McpManager>>,
    /// Live routing state. `Arc` so external holders (the supervisor,
    /// agent builders, host apps) share the same instance and see
    /// each other's mutations. Resolution-time exclusion of models
    /// declared in `excluded_models` consults this field.
    routing: Arc<crate::routing::RoutingControl>,
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

    /// Get the port registry (state, config, auth, event bus, ...).
    pub fn ports(&self) -> &PortRegistry {
        &self.ports
    }

    /// Catalog port accessor. Use this for all catalog queries.
    ///
    /// Returns a reference to the `Arc<dyn ModelCatalog>`. The default
    /// (when `OxiBuilder::with_catalog()` is not called) is a
    /// [`NoopModelCatalog`](crate::ports::catalog::NoopModelCatalog) —
    /// all lookups return empty/None.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn doc(oxi: oxi_sdk::Oxi) -> Result<(), oxi_sdk::SdkError> {
    /// let providers = oxi.catalog().list_providers().await?;
    /// let model = oxi.catalog().get_model("anthropic", "claude-sonnet-4-20250514").await?;
    /// # Ok(()) }
    /// ```
    pub fn catalog(&self) -> &Arc<dyn crate::ports::catalog::ModelCatalog> {
        &self.ports.catalog
    }

    /// Get the MCP manager, if MCP is enabled.
    ///
    /// This is the entry point for SDK consumers who want to use MCP from
    /// outside the agent loop — e.g. the TUI dashboard, RPC handlers, or
    /// custom agent integrations.
    ///
    /// Returns `None` if MCP was disabled via [`OxiBuilder::with_mcp`] with
    /// `false`.
    pub fn mcp(&self) -> Option<Arc<oxi_agent::mcp::McpManager>> {
        self.mcp_manager.clone()
    }

    /// Resolve a model ID to a Model.
    ///
    /// Accepts `"provider/model"` or bare `"model"` (defaults to "anthropic").
    ///
    /// Resolution order:
    /// 1. The catalog port (if wired) — reads the in-memory snapshot.
    /// 2. The static model registry (`with_builtins`).
    ///
    /// The `routing.excluded_models` list is consulted **before** the
    /// catalog/static lookups — `set_enabled(false)` / `exclude_model`
    /// / `unexclude_model` on the shared `RoutingControl` instance
    /// take effect on the next resolution.
    pub fn resolve_model(&self, model_id: &str) -> Result<Model> {
        // Live routing exclusion: ONLY active when is_enabled().
        // `set_enabled(false)` is an explicit opt-out — it means
        // "skip routing rules, resolve normally," NOT "refuse to
        // resolve." The default Oxi (RoutingControl::default) has
        // auto_routing=true, so this gate is a no-op unless the
        // host explicitly disabled routing.
        if self.routing.is_enabled() && self.routing.excluded_models().iter().any(|m| m == model_id)
        {
            return Err(anyhow::anyhow!(
                "Model '{model_id}' is in RoutingControl::excluded_models"
            ));
        }

        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        let (provider, model) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("anthropic", parts[0])
        };

        // 1. Catalog port (sync read of the snapshot).
        if let Some(ref entry) = self.ports.catalog.get_model_sync(provider, model) {
            return Ok(crate::bridge::catalog_entry_to_model(provider, entry));
        }

        // 2. Static model registry fallback.
        self.models
            .lookup(provider, model)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_id))
    }

    /// 1. Custom providers registered via `OxiBuilder::provider()`
    /// 2. Provider factories registered via `OxiBuilder::provider_factory()`
    /// 3. Built-in providers with credential injection (if `with_builtins()` was called):
    ///    a. Explicit per-provider key from `OxiBuilder::api_key(name, key)`
    ///    b. The wired `AuthProvider` port (sync fast-path). This is the
    ///    primary credential source for products like the CLI, which
    ///    never call `OxiBuilder::api_key()` and instead register
    ///    `FileAuthProvider` via `.with_auth(...)`. Consulted on every
    ///    `create_provider` call, so auth-store updates (e.g. a key entered
    ///    via the TUI overlay) are picked up without rebuilding the engine.
    ///    c. Provider env var (the `create_builtin_provider_with_options`
    ///    fallback inside `oxi-ai`).
    ///
    /// This is the **single credential authority** for the agent loop: the
    /// `AgentConfig.api_key` field and the `api_key` params on
    /// `Agent::switch_model` / `Agent::refresh_api_key` are vestigial after
    /// this wiring and are removed in a follow-up. See issues #39 and #40.
    pub fn create_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        // 1. Check custom providers registered via OxiBuilder::provider()
        if let Some(p) = self.providers.get_custom(name) {
            return Ok(p);
        }
        // 2. Built-in providers with credential injection.
        if self.include_builtins {
            let base_url = self.base_urls.get(name).map(|s| s.as_str());
            // Credential resolution: explicit OxiBuilder::api_key() override first,
            // then the AuthProvider port's sync fast-path, then env-var fallback
            // (handled inside create_builtin_provider_with_options).
            let explicit_key = self.api_keys.get(name).map(|s| s.as_str());
            let auth_port_key = self
                .ports
                .auth
                .get_api_key_sync(name)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty());
            let api_key = explicit_key.or(auth_port_key.as_deref());
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

    /// Borrow the shared [`RoutingControl`] instance. Use this to
    /// call `set_enabled`, `exclude_model`, `set_fallback_models`, etc.
    /// Mutations are observed by the next model/provider resolution.
    pub fn routing(&self) -> &Arc<crate::routing::RoutingControl> {
        &self.routing
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
    /// Port registry (None = use noop default).
    ports: Option<PortRegistry>,
    /// Programmatic MCP config (overrides the on-disk config if set).
    mcp_config: Option<oxi_agent::mcp::McpConfig>,
    /// Whether MCP is enabled. Defaults to true (when `with_builtins()` is
    /// also called) or as set by `with_mcp(false)`.
    mcp_enabled: bool,
    /// Custom disk path for the MCP metadata cache. When unset, oxi uses
    /// its default (`~/.config/oxi/mcp-cache.json`).
    mcp_cache_path: Option<std::path::PathBuf>,
    /// Custom disk path for the MCP consent store. When unset, oxi uses
    /// its default (`~/.config/oxi/mcp-consent.json`).
    mcp_consent_path: Option<std::path::PathBuf>,
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
            ports: None,
            mcp_config: None,
            mcp_enabled: true,
            mcp_cache_path: None,
            mcp_consent_path: None,
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
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxi_sdk::{OxiBuilder, OpenAiProvider};
    ///
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .provider_factory("custom", || {
    ///         Ok(Arc::new(OpenAiProvider::with_base_url_and_key(
    ///             "https://api.example.com",
    ///             Some("key".into()),
    ///         )))
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
    /// ```rust
    /// use oxi_sdk::OxiBuilder;
    ///
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .api_key("anthropic", "sk-ant-test-key")
    ///     .api_key("openai", "sk-test-key")
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
    /// ```rust
    /// use oxi_sdk::OxiBuilder;
    ///
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
    /// ```rust
    /// use oxi_sdk::OxiBuilder;
    ///
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .credential("openai", "sk-test", Some("https://proxy.example.com/v1"))
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

    // ─── Port registration ────────────────────────────────────────────────
    //
    // Products (oxi-cli, oxios-kernel, custom apps) register concrete
    // implementations of the port traits defined in `crate::ports`.
    // All ports are optional: unset ports use a noop default.

    /// Register a complete [`PortRegistry`] at once.
    ///
    /// Use this when you have a fully-built registry (e.g. loaded from a
    /// directory of file-based adapters). For piecemeal registration, use
    /// the `with_port_*` methods below.
    pub fn with_ports(mut self, ports: PortRegistry) -> Self {
        self.ports = Some(ports);
        self
    }

    /// Register the model catalog port.
    ///
    /// The catalog is the source of truth for provider/model metadata.
    /// If not called, the SDK uses [`NoopModelCatalog`](crate::ports::catalog::NoopModelCatalog)
    /// (empty results — all lookups return `None`/`vec![]`).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oxi_sdk::{OxiBuilder, NoopModelCatalog};
    ///
    /// // `NoopModelCatalog` is the empty default used when no catalog is
    /// // registered — pass any `Arc<dyn ModelCatalog>` here instead.
    /// let catalog = NoopModelCatalog::new();
    /// let oxi = OxiBuilder::new()
    ///     .with_catalog(catalog)
    ///     .build();
    /// ```
    pub fn with_catalog(mut self, catalog: Arc<dyn crate::ports::catalog::ModelCatalog>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.catalog = catalog;
        self.ports = Some(ports);
        self
    }

    /// Register the state store.
    pub fn with_state(mut self, store: Arc<dyn crate::ports::StateStore>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.state = store;
        self.ports = Some(ports);
        self
    }

    /// Register the config store.
    pub fn with_config(mut self, store: Arc<dyn crate::ports::ConfigStore>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.config = store;
        self.ports = Some(ports);
        self
    }

    /// Register the auth provider.
    pub fn with_auth(mut self, auth: Arc<dyn crate::ports::AuthProvider>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.auth = auth;
        self.ports = Some(ports);
        self
    }

    /// Register the event bus.
    pub fn with_event_bus(mut self, bus: Arc<dyn crate::ports::EventBus>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.event_bus = bus;
        self.ports = Some(ports);
        self
    }

    /// Register the skill loader.
    pub fn with_skills(mut self, loader: Arc<dyn crate::ports::SkillLoader>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.skills = loader;
        self.ports = Some(ports);
        self
    }

    /// Register the persona provider.
    pub fn with_personas(mut self, provider: Arc<dyn crate::ports::PersonaProvider>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.personas = provider;
        self.ports = Some(ports);
        self
    }

    /// Register the access gate.
    pub fn with_access(mut self, gate: Arc<dyn crate::ports::AccessGate>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.access = gate;
        self.ports = Some(ports);
        self
    }

    /// Register the capability resolver.
    pub fn with_capabilities(
        mut self,
        resolver: Arc<dyn crate::ports::CapabilityResolver>,
    ) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.capabilities = resolver;
        self.ports = Some(ports);
        self
    }

    /// Register the memory store.
    pub fn with_memory(mut self, store: Arc<dyn crate::ports::MemoryStore>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.memory = store;
        self.ports = Some(ports);
        self
    }

    /// Register the cron scheduler.
    pub fn with_cron(mut self, scheduler: Arc<dyn crate::ports::CronScheduler>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.cron = scheduler;
        self.ports = Some(ports);
        self
    }

    /// Register the resource monitor.
    pub fn with_resources(mut self, monitor: Arc<dyn crate::ports::ResourceMonitor>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.resources = monitor;
        self.ports = Some(ports);
        self
    }

    /// Register the internal URL router.
    pub fn with_url_router(mut self, router: Arc<dyn crate::ports::InternalUrlRouter>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.url_router = router;
        self.ports = Some(ports);
        self
    }

    /// Register the rule registry (TTSR).
    pub fn with_rules(mut self, rules: Arc<dyn crate::ports::RuleRegistry>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.rules = rules;
        self.ports = Some(ports);
        self
    }

    /// Register the embedding provider.
    pub fn with_embeddings(mut self, embeddings: Arc<dyn crate::ports::EmbeddingProvider>) -> Self {
        let mut ports = self.ports.unwrap_or_default();
        ports.embeddings = embeddings;
        self.ports = Some(ports);
        self
    }

    /// Enable multi-provider routing with automatic complexity-based model selection.
    ///
    /// This registers a [`MultiProvider`](oxi_ai::multi_provider::MultiProvider) that routes requests based on task complexity,
    /// with configurable fallback chains and circuit breaker protection.
    ///
    /// # Arguments
    ///
    /// * `config` - Routing configuration (use [`RoutingConfig::new()`] for defaults)
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_sdk::{OxiBuilder, RoutingConfig};
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
    /// use oxi_sdk::OxiBuilder;
    ///
    /// let (oxi, supervisor) = OxiBuilder::new()
    ///     .with_builtins()
    ///     .supervisor()
    ///     .snapshot_dir("/data/snapshots")
    ///     .build()?;
    /// ```
    pub fn supervisor(self) -> SupervisorBuilder {
        SupervisorBuilder {
            oxi_builder: self,
            policy: SupervisorPolicy::default(),
            snapshot_dir: None,
            agent_decorator: None,
        }
    }
    /// Build the Oxi engine. This consumes the builder.
    pub fn build(self) -> Oxi {
        // Spawn the MCP manager unless explicitly disabled.
        let mcp_manager = if self.mcp_enabled {
            if self.mcp_cache_path.is_some() || self.mcp_consent_path.is_some() {
                let cfg = match self.mcp_config {
                    Some(cfg) => cfg,
                    None => oxi_agent::mcp::config::load_mcp_config(),
                };
                Some(oxi_agent::mcp::McpManager::spawn_with_paths(
                    cfg,
                    self.mcp_cache_path,
                    self.mcp_consent_path,
                ))
            } else {
                Some(match self.mcp_config {
                    Some(cfg) => oxi_agent::mcp::McpManager::spawn_with_config(cfg),
                    None => oxi_agent::mcp::McpManager::spawn(),
                })
            }
        } else {
            None
        };

        Oxi {
            providers: Arc::new(self.providers),
            models: Arc::new(self.models),
            tools: Arc::new(self.tools),
            include_builtins: self.include_builtins,
            api_keys: Arc::new(self.api_keys),
            base_urls: Arc::new(self.base_urls),
            ports: self.ports.unwrap_or_default(),
            mcp_manager,
            routing: Arc::new(crate::routing::RoutingControl::new(
                crate::routing::RoutingConfig::default(),
            )),
        }
    }

    // ── MCP configuration (Phase SDK) ───────────────────────────────

    /// Inject a programmatic MCP configuration. This overrides the
    /// on-disk `~/.config/oxi/mcp.json` and `.mcp.json` discovery.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oxi_sdk::{OxiBuilder, McpConfig, ServerEntry, LifecycleMode};
    ///
    /// let mut mcp = McpConfig::default();
    /// mcp.mcp_servers.insert(
    ///     "my-server".into(),
    ///     ServerEntry {
    ///         command: Some("npx".into()),
    ///         args: Some(vec!["-y".into(), "@my-org/mcp-server".into()]),
    ///         lifecycle: Some(LifecycleMode::Lazy),
    ///         ..Default::default()
    ///     },
    /// );
    ///
    /// let oxi = OxiBuilder::new()
    ///     .with_builtins()
    ///     .with_mcp_config(mcp)
    ///     .build();
    /// ```
    pub fn with_mcp_config(mut self, config: oxi_agent::mcp::McpConfig) -> Self {
        self.mcp_config = Some(config);
        self.mcp_enabled = true;
        self
    }

    /// Set custom disk paths for the MCP metadata cache and consent store.
    ///
    /// Only takes effect when MCP is enabled (see [`with_mcp`](Self::with_mcp)).
    /// When unset, oxi uses its default paths (`~/.config/oxi/`). Intended
    /// for SDK consumers that self-host MCP state under their own config
    /// directory (e.g. oxios under `~/.oxios/`).
    ///
    /// Combine with [`with_mcp_config`](Self::with_mcp_config) to also inject
    /// a programmatic config. If only paths are supplied (no config), oxi
    /// auto-discovers its config from the standard file locations and writes
    /// cache/consent to the supplied paths.
    pub fn with_mcp_paths(
        mut self,
        cache_path: std::path::PathBuf,
        consent_path: std::path::PathBuf,
    ) -> Self {
        self.mcp_cache_path = Some(cache_path);
        self.mcp_consent_path = Some(consent_path);
        self
    }

    /// Enable or disable MCP. When disabled, no `McpManager` is spawned
    /// and the `mcp` proxy tool / direct tools are not registered.
    ///
    /// Defaults to `true`.
    pub fn with_mcp(mut self, enabled: bool) -> Self {
        self.mcp_enabled = enabled;
        self
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
    /// Cross-cutting decorator applied to every supervisor-spawned
    /// agent. `None` (default) keeps the legacy fast path.
    agent_decorator: Option<Arc<dyn crate::observability::AgentDecorator>>,
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

    /// Attach an [`AgentDecorator`] that wraps every
    /// supervisor-spawned agent.
    ///
    /// When set, [`SupervisorBuilder::build`] clones the built `Oxi`
    /// into the supervisor and configures it to route spawns through
    /// `Oxi::agent(config)` + `decorator.decorate(builder)` instead
    /// of the bare `Agent::new(provider, config, tools)` fast path.
    /// Use [`ObservabilityDecorator`] to bundle audit / authorizer /
    /// tracer / cost-tracker — those hooks then actually run on
    /// every spawned agent (no longer silent no-ops).
    ///
    /// Replaces the four deprecated no-op setters `with_audit`,
    /// `with_authorizer`, `with_tracer`, `with_cost_tracker`, which
    /// emitted `tracing::warn!` and dropped their arguments.
    pub fn with_agent_decorator(
        mut self,
        decorator: Arc<dyn crate::observability::AgentDecorator>,
    ) -> Self {
        self.agent_decorator = Some(decorator);
        self
    }

    /// Build the supervisor.
    ///
    /// Creates an `Oxi` instance internally and constructs the
    /// supervisor with a file-based snapshot store. When
    /// [`with_agent_decorator`](Self::with_agent_decorator) was
    /// called, the built `Oxi` is cloned into the supervisor so
    /// every spawn routes through the decorator.
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
        let supervisor = if let Some(decorator) = self.agent_decorator {
            supervisor.with_agent_decorator(Arc::new(oxi.clone()), decorator)
        } else {
            supervisor
        };
        Ok((oxi, supervisor))
    }
}
