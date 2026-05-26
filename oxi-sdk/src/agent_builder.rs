//! AgentBuilder — Fluent API for creating agents

use std::path::PathBuf;
use std::sync::Arc;

use oxi_agent::{
    tools::browse::{BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine},
    Agent, AgentConfig, AgentTool, AgentToolResult, ProviderResolver, ToolContext, ToolRegistry,
};

use crate::builder::Oxi;
use crate::middleware::{Middleware, MiddlewarePipeline};
use crate::observability::{AuditLog, CostTracker, Tracer};
use crate::security::{Authorizer, CapabilitySet};

/// Wrapper that makes Arc<Oxi> usable as ProviderResolver.
/// This is needed because Agent stores Arc<dyn ProviderResolver + 'static>.
pub(crate) struct OxiResolver {
    oxi: Arc<OxiCore>,
}

/// Type-erased Oxi inner for the resolver.
/// We can't use `Oxi` directly because it's in the same crate.
/// Instead we use a trait object approach.
pub(crate) struct OxiCore {
    #[allow(clippy::type_complexity)]
    resolve_provider_fn: Box<dyn Fn(&str) -> Option<Arc<dyn oxi_ai::Provider>> + Send + Sync>,
    #[allow(clippy::type_complexity)]
    resolve_model_fn: Box<dyn Fn(&str) -> Option<oxi_ai::Model> + Send + Sync>,
}

impl ProviderResolver for OxiResolver {
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn oxi_ai::Provider>> {
        (self.oxi.resolve_provider_fn)(name)
    }

    fn resolve_model(&self, model_id: &str) -> Option<oxi_ai::Model> {
        (self.oxi.resolve_model_fn)(model_id)
    }
}

/// Builder for creating an agent with custom configuration.
#[allow(dead_code)]
pub struct AgentBuilder<'a> {
    oxi: &'a Oxi,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace_dir: Option<PathBuf>,
    system_prompt: Option<String>,
    // ── Security ──
    capabilities: Option<CapabilitySet>,
    authorizer: Option<Arc<Authorizer>>,
    // ── Observability ──
    tracer: Option<Arc<Tracer>>,
    audit_log: Option<Arc<AuditLog>>,
    cost_tracker: Option<Arc<CostTracker>>,
    // ── Middleware ──
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl<'a> AgentBuilder<'a> {
    pub fn new(oxi: &'a Oxi, config: AgentConfig) -> Self {
        Self {
            oxi,
            config,
            tools: ToolRegistry::new(),
            workspace_dir: None,
            system_prompt: None,
            capabilities: None,
            authorizer: None,
            tracer: None,
            audit_log: None,
            cost_tracker: None,
            middlewares: Vec::new(),
        }
    }

    /// Set the working directory for file tools.
    pub fn workspace(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = Some(dir.into());
        self
    }

    /// Set a custom system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Register the standard coding tools (read, write, edit, bash, grep, find, ls, ...).
    pub fn coding_tools(self) -> Self {
        let cwd = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tools = crate::tool_factory::coding_tools(&cwd);
        for name in tools.names() {
            if let Some(tool) = tools.get(&name) {
                self.tools.register_arc(tool);
            }
        }
        self
    }

    /// Register read-only tools (read, ls).
    pub fn readonly_tools(self) -> Self {
        let cwd = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tools = crate::tool_factory::readonly_tools(&cwd);
        for name in tools.names() {
            if let Some(tool) = tools.get(&name) {
                self.tools.register_arc(tool);
            }
        }
        self
    }

    /// Register a tool.
    pub fn tool(self, tool: impl AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// Register a custom tool from a closure (synchronous handler).
    ///
    /// Creates a [`ClosureTool`] internally.
    ///
    /// # Example
    /// ```ignore
    /// .custom_tool(
    ///     "memory_recall",
    ///     "Search long-term memory",
    ///     json!({"type": "object", "properties": {"query": {"type": "string"}}}),
    ///     |params, _ctx| {
    ///         let query = params["query"].as_str().unwrap();
    ///         Ok(AgentToolResult::success(format!("Recalled: {}", query)))
    ///     },
    /// )
    /// ```
    pub fn custom_tool(
        self,
        name: impl Into<String>,
        description: impl Into<String>,
        schema: serde_json::Value,
        handler: impl Fn(serde_json::Value, &ToolContext) -> Result<AgentToolResult, oxi_agent::ToolError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.tool(crate::closure_tool::ClosureTool::new_sync(
            name,
            description,
            schema,
            handler,
        ))
    }

    /// Register multiple tools.
    pub fn tools(self, tools: impl IntoIterator<Item = impl AgentTool + 'static>) -> Self {
        for tool in tools {
            self.tools.register(tool);
        }
        self
    }

    /// Register browser tools (browse, browse_extract) with the given engine.
    ///
    /// This is the primary entry point for SDK consumers that want built-in
    /// web browsing. Pass any [`BrowserEngine`] implementation — when the
    /// `native-browser` feature is enabled on `oxi-agent`, use
    /// [`OxiBrowserEngine`](oxi_agent::tools::browse::OxiBrowserEngine) for
    /// the built-in headless browser.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let engine = Arc::new(OxiBrowserEngine::new()?);
    /// let agent = oxi.agent(config)
    ///     .workspace("/project")
    ///     .coding_tools()
    ///     .browsing(engine)
    ///     .build()?;
    /// ```
    pub fn browsing(self, engine: Arc<dyn BrowserEngine>) -> Self {
        self.tools.register(BrowseTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseExtractTool::new(engine));
        self
    }

    /// Register browser tools with custom configuration.
    ///
    /// Like [`browsing()`](Self::browsing) but allows tuning timeouts,
    /// cache, tab limits, etc. via [`BrowseConfig`].
    pub fn browsing_with_config(
        self,
        engine: Arc<dyn BrowserEngine>,
        config: BrowseConfig,
    ) -> Self {
        self.tools
            .register(BrowseTool::with_config(Arc::clone(&engine), config.clone()));
        self.tools
            .register(BrowseExtractTool::with_config(engine, config));
        self
    }

    /// Register the native browser tools using `oxibrowser-core`.
    ///
    /// Convenience method that creates an `OxiBrowserEngine` and registers
    /// all browser tools. Only available when the `native-browser` feature
    /// is enabled.
    #[cfg(feature = "native-browser")]
    #[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
    pub async fn native_browser(self) -> anyhow::Result<Self> {
        let engine = oxi_agent::tools::browse::OxiBrowserEngine::new().await?;
        Ok(self.browsing(Arc::new(engine)))
    }

    /// Register all browser tools including persistent session support.
    ///
    /// Like [`browsing()`](Self::browsing) but also registers `browse_script`
    /// and `browse_session` for multi-step interactive sessions with a
    /// persistent tab. Only available when the `native-browser` feature
    /// is enabled.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let engine = Arc::new(OxiBrowserEngine::new()?);
    /// let agent = oxi.agent(config)
    ///     .browsing_with_session(engine)
    ///     .build()?;
    /// ```
    #[cfg(feature = "native-browser")]
    #[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
    pub fn browsing_with_session(self, engine: Arc<dyn BrowserEngine>) -> Self {
        use oxi_agent::tools::browse::{BrowseScriptTool, BrowseSessionTool};

        self.tools.register(BrowseTool::new(Arc::clone(&engine)));
        self.tools
            .register(BrowseExtractTool::new(Arc::clone(&engine)));
        self.tools
            .register(BrowseScriptTool::new(Arc::clone(&engine)));
        self.tools.register(BrowseSessionTool::new(engine));
        self
    }

    /// Register kernel tools from a [`KernelToolProvider`].
    ///
    /// This is the bridge for oxios kernel tools (exec, memory, browser, etc.).
    /// The kernel implements `KernelToolProvider` and registers its tools
    /// into the agent's tool registry.
    ///
    /// [`KernelToolProvider`]: crate::KernelToolProvider
    pub fn kernel_tools(
        self,
        provider: &dyn crate::KernelToolProvider,
        context: &crate::KernelToolContext,
    ) -> Self {
        provider.register_tools(&self.tools, context);
        self
    }

    // ── Security ──────────────────────────────────────────

    /// Set the capability set for this agent.
    pub fn capabilities(mut self, caps: CapabilitySet) -> Self {
        self.capabilities = Some(caps);
        self
    }

    /// Use standard coding capabilities.
    pub fn coding_capabilities(self) -> Self {
        let ws = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        self.capabilities(CapabilitySet::coding(ws.to_str().unwrap_or(".")))
    }

    /// Use read-only capabilities.
    pub fn readonly_capabilities(self) -> Self {
        let ws = self
            .workspace_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        self.capabilities(CapabilitySet::read_only(ws.to_str().unwrap_or(".")))
    }

    /// Attach an authorizer for capability enforcement.
    pub fn authorizer(mut self, authorizer: Arc<Authorizer>) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    // ── Observability ──────────────────────────────────────

    /// Attach a tracer for distributed tracing.
    pub fn tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attach an audit log for security and tool audit trail.
    pub fn audit_log(mut self, audit: Arc<AuditLog>) -> Self {
        self.audit_log = Some(audit);
        self
    }

    /// Attach a cost tracker for token and cost monitoring.
    pub fn cost_tracker(mut self, tracker: Arc<CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }

    // ── Middleware ─────────────────────────────────────────

    /// Add a middleware to the pipeline.
    pub fn middleware(mut self, mw: impl Middleware + 'static) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }

    /// Add a rate limit middleware (convenience shortcut).
    pub fn with_rate_limit(self, max_per_minute: usize) -> Self {
        self.middleware(crate::middleware::RateLimitMiddleware::new(max_per_minute))
    }

    /// Add a token budget middleware (convenience shortcut).
    pub fn with_token_budget(self, max_tokens: usize) -> Self {
        self.middleware(crate::middleware::TokenBudgetMiddleware::new(max_tokens))
    }

    /// Add a logging middleware (convenience shortcut).
    pub fn with_logging(self) -> Self {
        self.middleware(crate::middleware::LoggingMiddleware::new(
            tracing::Level::INFO,
        ))
    }

    /// Build the agent.
    ///
    /// Uses the Oxi engine's `ProviderResolver` for isolated provider/model
    /// lookups, so `switch_model()` and compaction stay within the engine's
    /// registry — no global state pollution.
    pub fn build(self) -> anyhow::Result<Agent> {
        // 1. Resolve model from Oxi's instance registry
        let model = self.oxi.resolve_model(&self.config.model_id)?;

        // 2. Create provider via Oxi's engine (custom → built-in fallback)
        let provider: Arc<dyn oxi_ai::Provider> = self.oxi.create_provider(&model.provider)?;

        // 3. Merge workspace_dir into config
        let mut config = self.config.clone();
        config.workspace_dir = self.workspace_dir.or(config.workspace_dir);
        if let Some(ref prompt) = self.system_prompt {
            config.system_prompt = Some(prompt.clone());
        }

        // 4. Create resolver that captures Oxi's resolution functions
        let oxi_providers = self.oxi.providers_arc();
        let oxi_models = self.oxi.models_arc();
        let include_builtins = self.oxi.has_builtins();

        let resolver: Arc<dyn ProviderResolver> = Arc::new(OxiResolver {
            oxi: Arc::new(OxiCore {
                resolve_provider_fn: Box::new(move |name: &str| {
                    // Custom providers first
                    if let Some(p) = oxi_providers.get_custom(name) {
                        return Some(p);
                    }
                    // Built-in fallback
                    if include_builtins {
                        if let Some(p) = oxi_ai::create_builtin_provider(name) {
                            return Some(Arc::from(p));
                        }
                    }
                    None
                }),
                resolve_model_fn: Box::new(move |model_id: &str| {
                    let parts: Vec<&str> = model_id.splitn(2, '/').collect();
                    let (provider, model) = if parts.len() == 2 {
                        (parts[0], parts[1])
                    } else {
                        ("anthropic", parts[0])
                    };
                    oxi_models.lookup(provider, model)
                }),
            }),
        });

        // 5. Create agent with the isolated resolver
        let agent = Agent::new_with_resolver(provider, config, Arc::new(self.tools), resolver);

        // 6. Authorizer: grant capabilities
        if let Some(authorizer) = &self.authorizer {
            let agent_id = if agent.get_config().name.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                agent.get_config().name.clone()
            };
            if let Some(caps) = self.capabilities {
                let subject = crate::security::CapabilitySubject::Agent(agent_id);
                authorizer.grant(subject, caps);
            }
        }

        // 7. Middleware pipeline → AgentHooks
        if !self.middlewares.is_empty() {
            let pipeline = Arc::new(
                self.middlewares
                    .into_iter()
                    .fold(MiddlewarePipeline::new(), |p, mw| p.add_arc(mw)),
            );
            let agent_id = if agent.get_config().name.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                agent.get_config().name.clone()
            };
            let terminate_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let hooks = crate::middleware::build_hooks(pipeline, agent_id, terminate_flag);
            agent.set_hooks(hooks);
        }

        Ok(agent)
    }
}
