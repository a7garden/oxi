//! MultiProvider builder — ergonomic API for creating MultiProvider instances.
//!
//! This module provides a fluent builder for constructing [`MultiProvider`]
//! with custom routing, fallback chains, and circuit breaker configuration.
//!
//! # Example
//!
//! ```ignore
//! // Requires provider instances — see oxi_ai::create_builtin_provider
//! use oxi_sdk::multi_provider::{MultiProviderBuilder, RoutingConfig};
//!
//! let provider = MultiProviderBuilder::new()
//!     .prefer_cost_efficient()
//!     .with_fallbacks(&["anthropic/claude-3-5-haiku-20241022"])
//!     .build()
//!     .unwrap();
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use oxi_ai::{
    circuit_breaker::CircuitBreakerConfig, fallback_chain::FallbackChain,
    multi_provider::MultiProviderConfig, ComplexityRouter, MultiProvider, Provider,
};

// ============================================================================
// RoutingConfig
// ============================================================================

/// Routing configuration for enabling complexity-based routing.
///
/// Used with [`OxiBuilder::enable_routing()`] to configure automatic
/// model selection based on task complexity.
pub struct RoutingConfig {
    /// Enable automatic complexity-based routing.
    pub auto_routing: bool,
    /// Prefer cost-efficient models when capable.
    pub prefer_cost_efficient: bool,
    /// Custom complexity router (optional).
    pub router: Option<Box<dyn ComplexityRouter>>,
}

impl Clone for RoutingConfig {
    fn clone(&self) -> Self {
        Self {
            auto_routing: self.auto_routing,
            prefer_cost_efficient: self.prefer_cost_efficient,
            // Router can't be cloned, so we leave it as None
            router: None,
        }
    }
}

impl fmt::Debug for RoutingConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoutingConfig")
            .field("auto_routing", &self.auto_routing)
            .field("prefer_cost_efficient", &self.prefer_cost_efficient)
            .field("router", &"<dyn ComplexityRouter>")
            .finish()
    }
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            auto_routing: true,
            prefer_cost_efficient: true,
            router: None,
        }
    }
}

impl RoutingConfig {
    /// Create a new routing config with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable automatic complexity-based routing.
    pub fn auto_routing(mut self, enabled: bool) -> Self {
        self.auto_routing = enabled;
        self
    }

    /// Prefer cost-efficient models (cheaper when capable).
    pub fn prefer_cost_efficient(mut self, enabled: bool) -> Self {
        self.prefer_cost_efficient = enabled;
        self
    }

    /// Use a custom complexity router.
    pub fn with_router(mut self, router: impl ComplexityRouter + 'static) -> Self {
        self.router = Some(Box::new(router));
        self
    }
}

// ============================================================================
// MultiProviderBuilder
// ============================================================================

/// Fluent builder for creating [`MultiProvider`] instances.
///
/// # Example
///
/// ```ignore
/// // Requires provider instances — see oxi_ai::create_builtin_provider
/// use oxi_sdk::multi_provider::MultiProviderBuilder;
///
/// let provider = MultiProviderBuilder::new()
///     .prefer_cost_efficient()
///     .enable_auto_routing()
///     .build()
///     .unwrap();
/// ```
///
/// # Builder Methods
///
/// | Method | Description |
/// |--------|-------------|
/// | [`provider()`](MultiProviderBuilder::provider) | Add a named provider |
/// | [`with_fallbacks()`](MultiProviderBuilder::with_fallbacks) | Set fallback models by ID |
/// | [`with_fallback_chain()`](MultiProviderBuilder::with_fallback_chain) | Set custom fallback chain |
/// | [`with_router()`](MultiProviderBuilder::with_router) | Use custom complexity router |
/// | [`with_circuit_breaker()`](MultiProviderBuilder::with_circuit_breaker) | Configure circuit breaker |
/// | [`prefer_cost_efficient()`](MultiProviderBuilder::prefer_cost_efficient) | Prefer cheaper models |
/// | [`enable_auto_routing()`](MultiProviderBuilder::enable_auto_routing) | Enable complexity-based routing |
pub struct MultiProviderBuilder {
    router: Option<Box<dyn ComplexityRouter>>,
    providers: HashMap<String, Arc<dyn Provider>>,
    fallback_chain: FallbackChain,
    config: MultiProviderConfig,
}

impl MultiProviderBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            router: None,
            providers: HashMap::new(),
            fallback_chain: FallbackChain::default(),
            config: MultiProviderConfig::default(),
        }
    }

    /// Add a named provider to the multi-provider.
    ///
    /// Providers are registered with the multi-provider and can be
    /// referenced by name in model routing.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider identifier (e.g., "openai", "anthropic")
    /// * `provider` - The provider instance to register
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Requires Arc<dyn Provider> instances
    /// let provider = builder
    ///     .provider("openai", openai_provider)
    ///     .provider("anthropic", anthropic_provider)
    ///     .build()?;
    /// ```
    pub fn provider(mut self, name: &str, provider: Arc<dyn Provider>) -> Self {
        self.providers.insert(name.to_string(), provider);
        self
    }

    /// Set fallback models by their IDs (e.g., `"anthropic/claude-3-5-haiku-20241022"`).
    ///
    /// These are tried in order when the primary model fails.
    ///
    /// # Arguments
    ///
    /// * `ids` - Slice of `"provider/model-id"` strings
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_sdk::multi_provider::MultiProviderBuilder;
    ///
    /// let builder = MultiProviderBuilder::new()
    ///     .with_fallbacks(&[
    ///         "anthropic/claude-3-5-haiku-20241022",
    ///         "openai/gpt-4o-mini",
    ///     ]);
    /// ```
    pub fn with_fallbacks(self, ids: &[&str]) -> Self {
        let fallback = FallbackChain::from_ids(ids).unwrap_or_else(|_| FallbackChain::default());
        self.with_fallback_chain(fallback)
    }

    /// Set a custom fallback chain.
    ///
    /// Use this when you need full control over the fallback order.
    pub fn with_fallback_chain(mut self, fallback: FallbackChain) -> Self {
        self.fallback_chain = fallback;
        self
    }

    /// Set a custom complexity router.
    ///
    /// When not set, uses `DefaultRouter`.
    pub fn with_router(mut self, router: impl ComplexityRouter + 'static) -> Self {
        self.router = Some(Box::new(router));
        self
    }

    /// Set a custom complexity router from a boxed value.
    ///
    /// Internal use for consuming boxed routers from [`RoutingConfig`].
    pub fn with_router_boxed(mut self, router: Box<dyn ComplexityRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// Prefer cheaper models when capable.
    ///
    /// Sets `config.prefer_cost_efficient = true`.
    pub fn prefer_cost_efficient(mut self) -> Self {
        self.config.prefer_cost_efficient = true;
        self
    }

    /// Enable automatic routing based on task complexity.
    ///
    /// When enabled, the router classifies incoming tasks and selects
    /// appropriate models before falling back to the incoming model.
    pub fn enable_auto_routing(mut self) -> Self {
        self.config.auto_routing = true;
        self
    }

    /// Configure circuit breaker settings.
    ///
    /// Circuit breakers track provider health and prevent cascading failures.
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_sdk::CircuitBreakerConfig;
    ///
    /// let config = CircuitBreakerConfig {
    ///     failure_threshold: 5,
    ///     ..Default::default()
    /// };
    /// ```
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.config.circuit_breaker = config;
        self
    }

    /// Build the [`MultiProvider`].
    ///
    /// Returns an error if no providers were registered.
    pub fn build(self) -> anyhow::Result<Arc<dyn Provider>> {
        // Create MultiProvider with config
        let mut mp = MultiProvider::new(self.config);

        // Register router if custom - use a wrapper struct
        if let Some(router) = self.router {
            struct BoxedRouter(Box<dyn ComplexityRouter>);
            impl ComplexityRouter for BoxedRouter {
                fn classify(&self, context: &oxi_ai::Context) -> oxi_ai::Complexity {
                    self.0.classify(context)
                }
                fn route(
                    &self,
                    complexity: oxi_ai::Complexity,
                    prefer_cost_efficient: bool,
                ) -> Vec<&'static oxi_ai::model_db::ModelEntry> {
                    self.0.route(complexity, prefer_cost_efficient)
                }
            }
            mp = mp.set_router(BoxedRouter(router));
        }

        // Register all providers
        for (name, provider) in self.providers {
            mp.register_provider(&name, provider);
        }

        // Set fallback chain
        if !self.fallback_chain.is_empty() {
            mp = mp.with_fallback(self.fallback_chain);
        }

        if mp.provider_names().is_empty() {
            anyhow::bail!("MultiProvider requires at least one provider");
        }

        Ok(Arc::new(mp))
    }
}

impl Default for MultiProviderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_config_default() {
        let config = RoutingConfig::default();
        assert!(config.auto_routing);
        assert!(config.prefer_cost_efficient);
        assert!(config.router.is_none());
    }

    #[test]
    fn test_routing_config_builder() {
        let config = RoutingConfig::new()
            .auto_routing(true)
            .prefer_cost_efficient(false);

        assert!(config.auto_routing);
        assert!(!config.prefer_cost_efficient);
        // Router not set in this test
        assert!(config.router.is_none());
    }

    #[test]
    fn test_builder_new() {
        let builder = MultiProviderBuilder::new();
        // Just verify it can be created
        assert!(builder.config.auto_routing);
    }

    #[test]
    fn test_builder_prefer_cost_efficient() {
        let builder = MultiProviderBuilder::new().prefer_cost_efficient();
        assert!(builder.config.prefer_cost_efficient);
    }

    #[test]
    fn test_builder_enable_auto_routing() {
        let builder = MultiProviderBuilder::new().enable_auto_routing();
        assert!(builder.config.auto_routing);
    }

    #[test]
    fn test_builder_with_fallback_chain() {
        let fallback = FallbackChain::from_ids(&["openai/gpt-4o"]).unwrap();
        let builder = MultiProviderBuilder::new().with_fallback_chain(fallback);
        assert!(!builder.fallback_chain.is_empty());
    }
}
