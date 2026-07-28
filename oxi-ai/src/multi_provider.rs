//! MultiProvider — intelligent routing with fallback
//!
//! This module provides the core routing provider that ties together
//! ComplexityRouter, CircuitBreaker, and FallbackChain to implement
//! intelligent model selection with automatic failover.
//!
//! # Architecture
//!
//! MultiProvider orchestrates multiple components:
//! - **ComplexityRouter**: Classifies task complexity and selects appropriate models
//! - **CircuitBreaker**: Prevents cascading failures by tracking provider health
//! - **FallbackChain**: Provides ordered fallback when primary models fail
//!
//! # Priority Order (from design §8.3)
//!
//! When `auto_routing=true`:
//! 1. Router's best model (based on classified complexity)
//! 2. Incoming model (if registered and healthy)
//! 3. Fallback chain (if configured)
//!
//! When `auto_routing=false`:
//! 1. Incoming model (if registered and healthy)
//! 2. Fallback chain (if configured)
//!
//! # Error Handling
//!
//! - **Retryable errors** (429, 5xx, network, timeout): Record failure, try next model
//! - **Non-retryable errors** (400, 401, 403, etc.): Return immediately without recording failure
//!
//! # Example
//!
//! ```ignore
//! use oxi_ai::multi_provider::{MultiProvider, MultiProviderConfig};
//! use oxi_ai::fallback_chain::FallbackChain;
//!
//! let config = MultiProviderConfig::default();
//! let mut provider = MultiProvider::new(config);
//!
//! // Register providers
//! provider.register_provider("openai", Arc::new(openai_provider));
//! provider.register_provider("anthropic", Arc::new(anthropic_provider));
//!
//! // Set fallback chain
//! let fallback = FallbackChain::from_ids(&[
//!     "anthropic/claude-sonnet-4-20250514",
//!     "openai/gpt-4o",
//! ])?;
//! provider.with_fallback(fallback);
//!
//! // Use like any Provider
//! let stream = provider.stream(&model, &context, None).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use std::future::Future;
use std::pin::Pin;

use crate::{
    Model,
    circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker},
    complexity_router::{ComplexityRouter, DefaultRouter},
    context::Context,
    error::ProviderError,
    fallback_chain::FallbackChain,
    model_db::ModelEntry,
    providers::{FallbackReason, Provider, ProviderEvent, StreamOptions, StreamResult},
};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for MultiProvider behavior.
///
/// Controls auto-routing, cost preference, retry behavior, and circuit breaker settings.
#[derive(Debug, Clone)]
pub struct MultiProviderConfig {
    /// Enable automatic complexity-based routing.
    ///
    /// When `true`, the router classifies task complexity and selects
    /// appropriate models before falling back to the incoming model.
    ///
    /// Default: `true`
    pub auto_routing: bool,

    /// Prefer cost-efficient models when routing.
    ///
    /// When `true` and `auto_routing` is enabled, the router selects
    /// cheaper models that still meet the complexity requirements.
    ///
    /// Default: `true`
    pub prefer_cost_efficient: bool,

    /// Maximum retries per model before giving up.
    ///
    /// For each model in the candidate list, we retry this many times
    /// on retryable errors before moving to the next model.
    ///
    /// Default: `1`
    pub max_retries_per_model: usize,

    /// Per-model timeout for requests.
    ///
    /// If set, wraps the request in a timeout. If `None`, uses the
    /// provider's default timeout.
    ///
    /// Default: `None`
    pub per_model_timeout: Option<Duration>,

    /// Circuit breaker configuration for all providers.
    ///
    /// Each registered provider gets its own circuit breaker instance
    /// with this configuration.
    ///
    /// Default: `CircuitBreakerConfig::default()`
    pub circuit_breaker: CircuitBreakerConfig,
}

impl Default for MultiProviderConfig {
    fn default() -> Self {
        Self {
            auto_routing: true,
            prefer_cost_efficient: true,
            max_retries_per_model: 1,
            per_model_timeout: None,
            circuit_breaker: CircuitBreakerConfig::default(),
        }
    }
}

impl MultiProviderConfig {
    /// Enable or disable automatic routing.
    #[must_use]
    pub fn with_auto_routing(mut self, enabled: bool) -> Self {
        self.auto_routing = enabled;
        self
    }

    /// Enable or disable cost-efficient preference.
    #[must_use]
    pub fn with_prefer_cost_efficient(mut self, enabled: bool) -> Self {
        self.prefer_cost_efficient = enabled;
        self
    }

    /// Set the maximum retries per model.
    #[must_use]
    pub fn with_max_retries(mut self, retries: usize) -> Self {
        self.max_retries_per_model = retries;
        self
    }

    /// Set the per-model timeout.
    #[must_use]
    pub fn with_per_model_timeout(mut self, timeout: Duration) -> Self {
        self.per_model_timeout = Some(timeout);
        self
    }

    /// Set the circuit breaker configuration.
    #[must_use]
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = config;
        self
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur in MultiProvider operations.
#[derive(Debug, thiserror::Error)]
pub enum MultiProviderError {
    /// All providers in the candidate list have failed.
    ///
    /// Contains the list of errors from each provider for debugging.
    #[error("All providers exhausted")]
    AllProvidersExhausted {
        /// Errors from each provider in order of attempt.
        errors: Vec<(String, ProviderError)>,
    },

    /// No provider is registered that can handle the requested model.
    #[error("No provider available for model: {0}")]
    NoProviderForModel(String),

    /// Circuit breaker is open for the provider.
    ///
    /// The provider should be retried after `retry_after` duration.
    #[error("Circuit breaker open: {provider} (retry after {retry_after:?})")]
    CircuitBreakerOpen {
        /// Name of the provider whose circuit is open.
        provider: String,
        /// Duration to wait before retrying.
        retry_after: Duration,
    },

    /// No fallback models configured and the primary provider failed.
    #[error("No fallback models configured and primary provider failed")]
    NoFallback,

    /// No providers are registered with this MultiProvider.
    #[error("No provider registered")]
    NoProviderRegistered,
}

impl MultiProviderError {
    /// Check if this is a circuit breaker error.
    pub fn is_circuit_breaker(&self) -> bool {
        matches!(self, Self::CircuitBreakerOpen { .. })
    }

    /// Get the retry duration if this is a circuit breaker error.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::CircuitBreakerOpen { retry_after, .. } => Some(*retry_after),
            _ => None,
        }
    }
}

// ============================================================================
// MultiProvider
// ============================================================================

/// Intelligent routing provider with fallback support.
///
/// MultiProvider implements the `Provider` trait and provides automatic
/// model selection based on task complexity, with circuit breaker protection
/// and ordered fallback for resilience.
///
/// # Type Parameters
///
/// - `R`: The complexity router type (default: `DefaultRouter`)
/// - `F`: The fallback chain type (default: `FallbackChain`)
pub struct MultiProvider {
    /// Router for complexity-based model selection.
    router: Arc<dyn ComplexityRouter>,

    /// Registered providers by name.
    providers: HashMap<String, Arc<dyn Provider>>,

    /// Fallback chain for ordered failover.
    fallback: FallbackChain,

    /// Circuit breakers for each provider.
    breakers: HashMap<String, Arc<ProviderCircuitBreaker>>,

    /// Configuration settings.
    config: MultiProviderConfig,
}

impl MultiProvider {
    /// Create a new MultiProvider with the given configuration.
    ///
    /// Uses `DefaultRouter` for complexity-based routing.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = MultiProviderConfig::default();
    /// let provider = MultiProvider::new(config);
    /// ```
    pub fn new(config: MultiProviderConfig) -> Self {
        Self {
            router: Arc::new(DefaultRouter::new()),
            providers: HashMap::new(),
            fallback: FallbackChain::default(),
            breakers: HashMap::new(),
            config,
        }
    }

    /// Create a new MultiProvider with a custom router.
    ///
    /// Allows using a custom implementation of `ComplexityRouter`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let router = MyCustomRouter::new();
    /// let provider = MultiProvider::with_router(router);
    /// ```
    pub fn with_router(router: impl ComplexityRouter + 'static) -> Self {
        Self {
            router: Arc::new(router),
            providers: HashMap::new(),
            fallback: FallbackChain::default(),
            breakers: HashMap::new(),
            config: MultiProviderConfig::default(),
        }
    }

    /// Create a new MultiProvider with custom config and router.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = MultiProviderConfig::default()
    ///     .with_auto_routing(false)
    ///     .with_max_retries(2);
    /// let router = DefaultRouter::new();
    /// let provider = MultiProvider::with_config_and_router(config, router);
    /// ```
    pub fn with_config_and_router(
        config: MultiProviderConfig,
        router: impl ComplexityRouter + 'static,
    ) -> Self {
        Self {
            router: Arc::new(router),
            providers: HashMap::new(),
            fallback: FallbackChain::default(),
            breakers: HashMap::new(),
            config,
        }
    }

    /// Replace the router with a new implementation.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let provider = multi_provider.set_router(new_router);
    /// ```
    pub fn set_router(mut self, router: impl ComplexityRouter + 'static) -> Self {
        self.router = Arc::new(router);
        self
    }

    /// Set the fallback chain.
    ///
    /// The fallback chain is used when the primary model fails or is unavailable.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let fallback = FallbackChain::from_ids(&["anthropic/claude-sonnet-4"])?;
    /// let provider = multi_provider.with_fallback(fallback);
    /// ```
    pub fn with_fallback(mut self, fallback: FallbackChain) -> Self {
        self.fallback = fallback;
        self
    }

    /// Set the fallback chain by reference.
    pub fn set_fallback(&mut self, fallback: FallbackChain) {
        self.fallback = fallback;
    }

    /// Register a provider with this MultiProvider.
    ///
    /// The provider can be referenced by name when calling `stream()`.
    /// Each provider gets its own circuit breaker instance.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider identifier (e.g., "openai", "anthropic")
    /// * `provider` - The provider implementation
    ///
    /// # Example
    ///
    /// ```ignore
    /// let openai_provider = Arc::new(OpenAiProvider::new());
    /// multi_provider.register_provider("openai", openai_provider);
    /// ```
    pub fn register_provider(&mut self, name: &str, provider: Arc<dyn Provider>) {
        // Create circuit breaker for this provider
        let breaker = Arc::new(ProviderCircuitBreaker::new(
            name.to_string(),
            self.config.circuit_breaker.clone(),
        ));

        self.providers.insert(name.to_string(), provider);
        self.breakers.insert(name.to_string(), breaker);
    }

    /// Unregister a provider.
    ///
    /// Removes the provider and its associated circuit breaker.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider identifier to remove
    ///
    /// # Returns
    ///
    /// `true` if the provider was found and removed.
    pub fn unregister_provider(&mut self, name: &str) -> bool {
        let provider_removed = self.providers.remove(name).is_some();
        let breaker_removed = self.breakers.remove(name).is_some();
        provider_removed || breaker_removed
    }

    /// Get a provider by name.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider identifier
    ///
    /// # Returns
    ///
    /// `Option` containing the provider if found.
    pub fn get_provider(&self, name: &str) -> Option<&Arc<dyn Provider>> {
        self.providers.get(name)
    }

    /// Get the circuit breaker for a provider.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Provider identifier
    ///
    /// # Returns
    ///
    /// `Arc<ProviderCircuitBreaker>` if the provider is registered.
    pub fn get_breaker(&self, provider_name: &str) -> Option<Arc<ProviderCircuitBreaker>> {
        self.breakers.get(provider_name).cloned()
    }

    /// Get all registered provider names.
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }

    /// Get diagnostic information for all circuit breakers.
    ///
    /// # Returns
    ///
    /// Vector of diagnostics for each registered provider.
    pub fn circuit_breaker_diagnostics(
        &self,
    ) -> Vec<crate::circuit_breaker::CircuitBreakerDiagnostics> {
        self.breakers.values().map(|b| b.diagnostics()).collect()
    }

    /// Get the router used for complexity-based routing.
    pub fn router(&self) -> &Arc<dyn ComplexityRouter> {
        &self.router
    }

    /// Get a reference to the fallback chain.
    pub fn fallback(&self) -> &FallbackChain {
        &self.fallback
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &MultiProviderConfig {
        &self.config
    }

    /// Get diagnostic summary of the multi-provider state.
    pub fn diagnostics(&self) -> MultiProviderDiagnostics {
        MultiProviderDiagnostics {
            provider_count: self.providers.len(),
            router_type: "DefaultRouter".to_string(),
            fallback_len: self.fallback.len(),
            auto_routing: self.config.auto_routing,
            prefer_cost_efficient: self.config.prefer_cost_efficient,
            circuit_breakers: self.circuit_breaker_diagnostics(),
        }
    }
}

// ============================================================================
// Diagnostic Types
// ============================================================================

/// Diagnostic information about MultiProvider state.
#[derive(Debug, Clone)]
pub struct MultiProviderDiagnostics {
    /// Number of registered providers.
    pub provider_count: usize,
    /// Type of router being used.
    pub router_type: String,
    /// Number of models in the fallback chain.
    pub fallback_len: usize,
    /// Whether auto-routing is enabled.
    pub auto_routing: bool,
    /// Whether cost-efficient models are preferred.
    pub prefer_cost_efficient: bool,
    /// Circuit breaker diagnostics for each provider.
    pub circuit_breakers: Vec<crate::circuit_breaker::CircuitBreakerDiagnostics>,
}

// ============================================================================
// Fallback Event Stream Wrapper
// ============================================================================

use futures::stream::Stream as StreamTrait;

/// A wrapper stream that injects a `FallbackStart` event at the beginning,
/// then forwards all subsequent events from the underlying stream.
struct FallbackStream {
    /// The injected fallback event (always emitted first).
    fallback_event: ProviderEvent,
    /// Whether the fallback event has been emitted yet.
    emitted: bool,
    /// The inner stream we're wrapping.
    inner: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>,
}

impl FallbackStream {
    /// Create a new wrapper stream that will emit `FallbackStart` first.
    fn new(
        from_model: String,
        to_model: String,
        reason: FallbackReason,
        inner: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>,
    ) -> Self {
        Self {
            fallback_event: ProviderEvent::FallbackStart {
                from_model,
                to_model,
                reason,
            },
            emitted: false,
            inner,
        }
    }
}

impl StreamTrait for FallbackStream {
    type Item = ProviderEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // Emit the fallback event on the first poll
        if !self.emitted {
            self.emitted = true;
            return std::task::Poll::Ready(Some(self.fallback_event.clone()));
        }

        // Then delegate to the inner stream
        Stream::poll_next(self.inner.as_mut(), cx)
    }
}

/// A wrapper stream that emits `FallbackExhausted` and then terminates.
/// Used when all fallback candidates have been exhausted.
struct FallbackExhaustedStream {
    /// The exhausted event to emit.
    exhausted_event: ProviderEvent,
    /// Whether we've emitted the exhausted event.
    emitted: bool,
    /// The inner error stream (may emit additional error events before terminating).
    inner: Option<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>>,
}

impl FallbackExhaustedStream {
    /// Create a new wrapper stream that will emit `FallbackExhausted` first.
    fn new(models_tried: Vec<String>, final_error: String) -> Self {
        Self {
            exhausted_event: ProviderEvent::FallbackExhausted {
                models_tried,
                final_error,
            },
            emitted: false,
            inner: None,
        }
    }

    /// Set the inner error stream to forward events from.
    #[allow(dead_code)]
    fn with_inner(mut self, inner: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>) -> Self {
        self.inner = Some(inner);
        self
    }
}

impl StreamTrait for FallbackExhaustedStream {
    type Item = ProviderEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // Emit the exhausted event on the first poll
        if !self.emitted {
            self.emitted = true;
            return std::task::Poll::Ready(Some(self.exhausted_event.clone()));
        }

        // Then forward from inner stream if present, otherwise terminate
        if let Some(ref mut inner) = self.inner {
            Stream::poll_next(inner.as_mut(), cx)
        } else {
            std::task::Poll::Ready(None)
        }
    }
}

/// Determine the fallback reason from a provider error.
fn error_to_fallback_reason(error: &ProviderError) -> FallbackReason {
    match error {
        ProviderError::HttpError(d) if d.status == 429 => FallbackReason::RateLimit,
        ProviderError::HttpError(d) if d.status >= 500 => FallbackReason::ServerError,
        ProviderError::HttpError(d) if d.status == 401 || d.status == 403 => {
            FallbackReason::AuthError
        }
        ProviderError::RequestFailed(_) => FallbackReason::NetworkError,
        ProviderError::Timeout => FallbackReason::NetworkError,
        ProviderError::ContextOverflow => FallbackReason::ContextOverflow,
        _ => FallbackReason::Unknown,
    }
}

// ============================================================================
// Provider Trait Implementation
// ============================================================================

impl Provider for MultiProvider {
    /// Stream assistant message events with intelligent routing.
    ///
    /// This method implements the priority order logic:
    ///
    /// 1. If `auto_routing=true`: classify complexity and select router's best model
    /// 2. Try the incoming model (if registered and circuit breaker allows)
    /// 3. Try fallback chain models in order
    ///
    /// For each candidate model:
    /// - Check circuit breaker (skip if open)
    /// - Call provider.stream()
    /// - On retryable error: record failure, retry or move to next
    /// - On non-retryable error: return immediately
    /// - On success: record success to breaker, return stream
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            // Build candidate list based on priority order
            let candidates = self.build_candidate_list(model, context).await?;

            // Try each candidate in order
            let mut errors: Vec<(String, ProviderError)> = Vec::new();
            let mut current_candidate_idx: usize = 0;

            while current_candidate_idx < candidates.len() {
                let candidate = &candidates[current_candidate_idx];
                let provider_name = &candidate.provider;
                let candidate_model = candidate.model.clone();

                // Get provider
                let Some(provider) = self.providers.get(provider_name) else {
                    current_candidate_idx += 1;
                    continue;
                };

                // Check circuit breaker
                if let Some(breaker) = self.breakers.get(provider_name) {
                    match breaker.allow_request() {
                        Ok(()) => {
                            // Circuit allows request, proceed
                        }
                        Err(e) => {
                            // Circuit is open - skip this provider
                            tracing::debug!(
                                provider = %provider_name,
                                remaining = ?e.remaining,
                                "Circuit breaker open, skipping provider"
                            );
                            current_candidate_idx += 1;
                            continue;
                        }
                    }
                }

                // Try to stream with retries
                let mut retry_count = 0;
                let max_retries = self.config.max_retries_per_model;

                loop {
                    match provider
                        .stream(&candidate_model, context, options.clone())
                        .await
                    {
                        Ok(inner_stream) => {
                            // Success! Record to circuit breaker
                            if let Some(breaker) = self.breakers.get(provider_name) {
                                breaker.record_success();
                            }
                            tracing::debug!(
                                provider = %provider_name,
                                model = %candidate_model.id,
                                "MultiProvider: stream successful"
                            );

                            // Wrap stream with fallback event if we attempted a previous candidate
                            if current_candidate_idx > 0 {
                                let from_model = format!(
                                    "{}/{}",
                                    candidates[current_candidate_idx - 1].provider,
                                    candidates[current_candidate_idx - 1].model.id
                                );
                                let to_model = format!("{}/{}", provider_name, candidate_model.id);
                                let reason = errors
                                    .last()
                                    .map(|(_, e)| error_to_fallback_reason(e))
                                    .unwrap_or(FallbackReason::Unknown);

                                let wrapped =
                                    FallbackStream::new(from_model, to_model, reason, inner_stream);
                                return Ok(Box::pin(wrapped) as Pin<Box<_>>);
                            }

                            return Ok(inner_stream);
                        }
                        Err(e) => {
                            // Check if error is retryable
                            if e.is_retryable() && retry_count < max_retries {
                                // Retryable error - record failure and retry
                                retry_count += 1;
                                if let Some(breaker) = self.breakers.get(provider_name) {
                                    breaker.record_failure();
                                }
                                tracing::debug!(
                                    provider = %provider_name,
                                    model = %candidate_model.id,
                                    error = %e,
                                    retry = retry_count,
                                    "Retryable error, retrying"
                                );
                                continue;
                            }

                            // Non-retryable error or max retries exceeded
                            if !e.is_retryable() {
                                // Non-retryable errors (400, 401, 403, etc.) don't record failure
                                // Return immediately - these won't be fixed by retrying
                                tracing::warn!(
                                    provider = %provider_name,
                                    model = %candidate_model.id,
                                    error = %e,
                                    "Non-retryable error, returning immediately"
                                );
                                return Err(e);
                            }

                            // Max retries exceeded - try next candidate
                            tracing::debug!(
                                provider = %provider_name,
                                model = %candidate_model.id,
                                error = %e,
                                retries = retry_count,
                                "Max retries exceeded, trying next candidate"
                            );
                            errors.push((format!("{}/{}", provider_name, candidate_model.id), e));
                            break;
                        }
                    }
                }

                current_candidate_idx += 1;
            }

            // All candidates exhausted
            if errors.is_empty() {
                if self.providers.is_empty() {
                    Err(ProviderError::UnknownProvider(
                        "multi-provider: no providers registered".to_string(),
                    ))
                } else {
                    Err(ProviderError::UnknownProvider(
                        "multi-provider: no model could be routed".to_string(),
                    ))
                }
            } else {
                // Emit FallbackExhausted event
                let models_tried: Vec<String> = errors.iter().map(|(m, _)| m.clone()).collect();
                let final_error = errors
                    .last()
                    .map(|(_, e)| e.to_string())
                    .unwrap_or_else(|| "Unknown error".to_string());

                tracing::warn!(
                    models_tried = ?models_tried,
                    error = %final_error,
                    "All fallback models exhausted"
                );

                let stream = FallbackExhaustedStream::new(models_tried, final_error);
                Ok(Box::pin(stream) as Pin<Box<_>>)
            }
        })
    }

    /// Returns "multi-provider" as the provider name.
    fn name(&self) -> &str {
        "multi-provider"
    }
}

// ============================================================================
// Candidate List Building
// ============================================================================

/// A candidate model for streaming attempts.
struct Candidate {
    /// Provider name for this candidate.
    provider: String,
    /// Model to use with this provider.
    model: Model,
}

impl MultiProvider {
    /// Build the candidate list based on configuration and priority order.
    ///
    /// Priority order (from design §8.3):
    /// - auto_routing=true → router's best model → incoming model → fallback chain
    /// - auto_routing=false → incoming model → fallback chain
    async fn build_candidate_list(
        &self,
        incoming_model: &Model,
        context: &Context,
    ) -> Result<Vec<Candidate>, ProviderError> {
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut seen_ids: HashMap<String, ()> = HashMap::new();

        // Helper to add candidate if not already added
        let add_candidate = |candidates: &mut Vec<Candidate>,
                             seen_ids: &mut HashMap<String, ()>,
                             provider: String,
                             model: Model| {
            let id = format!("{}/{}", provider, model.id);
            if seen_ids.insert(id, ()).is_none() {
                candidates.push(Candidate { provider, model });
            }
        };

        // 1. Auto-routing: get router's best model
        if self.config.auto_routing {
            let complexity = self.router.classify(context);
            let router_models = self
                .router
                .route(complexity, self.config.prefer_cost_efficient);

            tracing::debug!(
                complexity = ?complexity,
                model_count = router_models.len(),
                "MultiProvider: router selected models for complexity"
            );

            for entry in router_models {
                // Try to get the model from registry
                if let Some(registered_model) =
                    crate::model_registry::get_model(entry.provider, entry.id)
                    && self.providers.contains_key(entry.provider)
                {
                    add_candidate(
                        &mut candidates,
                        &mut seen_ids,
                        entry.provider.to_string(),
                        registered_model.clone(),
                    );
                }

                // Also construct from entry if not found in registry
                if self.providers.contains_key(entry.provider) {
                    let model = self.model_from_entry(entry);
                    let id = format!("{}/{}", entry.provider, entry.id);
                    if seen_ids.insert(id, ()).is_none() {
                        candidates.push(Candidate {
                            provider: entry.provider.to_string(),
                            model,
                        });
                    }
                }
            }
        }

        // 2. Incoming model
        if self.providers.contains_key(&incoming_model.provider) {
            add_candidate(
                &mut candidates,
                &mut seen_ids,
                incoming_model.provider.clone(),
                incoming_model.clone(),
            );
        } else {
            // Try to find a provider for this model
            // Look through all providers to find one that handles this model type
            for provider_name in self.providers.keys() {
                // Check if the incoming model matches this provider's expected models
                let model_id = &incoming_model.id;

                // Try to get the model from registry
                if let Some(model) = self.find_model_for_provider(provider_name, model_id) {
                    add_candidate(&mut candidates, &mut seen_ids, provider_name.clone(), model);
                    break;
                }
            }
        }

        // 3. Fallback chain
        for fallback_entry in self.fallback.iter() {
            // Try registry first
            if let Some(registered_model) =
                crate::model_registry::get_model(fallback_entry.provider, fallback_entry.id)
            {
                if self.providers.contains_key(fallback_entry.provider) {
                    add_candidate(
                        &mut candidates,
                        &mut seen_ids,
                        fallback_entry.provider.to_string(),
                        registered_model.clone(),
                    );
                }
            } else if self.providers.contains_key(fallback_entry.provider) {
                // Construct from entry
                let model = self.model_from_entry(fallback_entry);
                let id = format!("{}/{}", fallback_entry.provider, fallback_entry.id);
                if seen_ids.insert(id, ()).is_none() {
                    candidates.push(Candidate {
                        provider: fallback_entry.provider.to_string(),
                        model,
                    });
                }
            }
        }

        // If no candidates found and providers exist, try using the first provider
        if candidates.is_empty() && !self.providers.is_empty() {
            // Use first available provider with a default model
            let (provider_name, _provider) = self
                .providers
                .iter()
                .next()
                .expect("providers map is non-empty");
            let model = self.default_model_for_provider(provider_name);
            add_candidate(&mut candidates, &mut seen_ids, provider_name.clone(), model);
        }

        tracing::debug!(
            candidate_count = candidates.len(),
            "MultiProvider: built candidate list"
        );

        if candidates.is_empty() && self.providers.is_empty() {
            return Err(ProviderError::UnknownProvider(
                "multi-provider: no providers registered".to_string(),
            ));
        }

        Ok(candidates)
    }

    /// Construct a Model from a ModelEntry.
    fn model_from_entry(&self, entry: &ModelEntry) -> Model {
        Model {
            id: entry.id.to_string(),
            name: entry.name.to_string(),
            api: entry.api,
            provider: entry.provider.to_string(),
            base_url: String::new(), // Will be set by provider
            reasoning: entry.reasoning,
            input: entry.input.to_vec(),
            cost: crate::types::Cost {
                input: entry.cost_input,
                output: entry.cost_output,
                cache_read: entry.cost_cache_read,
                cache_write: entry.cost_cache_write,
            },
            context_window: entry.context_window as usize,
            max_tokens: entry.max_tokens as usize,
            headers: HashMap::new(),
            compat: None,
        }
    }

    /// Find a model for a provider given a model ID.
    fn find_model_for_provider(&self, provider_name: &str, model_id: &str) -> Option<Model> {
        // Check registry
        if let Some(model) = crate::model_registry::get_model(provider_name, model_id) {
            return Some(model.clone());
        }

        // Check model_db
        if let Some(entry) = crate::model_db::get_model_entry(provider_name, model_id) {
            return Some(self.model_from_entry(entry));
        }

        // Construct from model_id
        Some(self.construct_model_from_id(provider_name, model_id))
    }

    /// Construct a Model from just provider and model ID strings.
    ///
    /// Uses model_db to get actual metadata (context_window, cost, reasoning support, etc.).
    /// Falls back to reasonable defaults if the model is not found in model_db.
    fn construct_model_from_id(&self, provider: &str, model_id: &str) -> Model {
        // First, try to look up the model in model_db
        if let Some(entry) = crate::model_db::get_model_entry(provider, model_id) {
            return self.model_from_entry(entry);
        }

        // Not in model_db: determine API type from the provider registry
        // (which is now materialized from models.dev). Falls back to
        // OpenAI Responses for unknown providers.
        let api = crate::providers::register_builtins::get_provider_api(provider)
            .unwrap_or(crate::types::Api::OpenAiResponses);

        Model {
            id: model_id.to_string(),
            name: model_id.to_string(),
            api,
            provider: provider.to_string(),
            base_url: String::new(),
            reasoning: false,
            input: vec![crate::types::InputModality::Text],
            cost: crate::types::Cost::default(),
            context_window: 128_000,
            max_tokens: 32_000,
            headers: HashMap::new(),
            compat: None,
        }
    }

    /// Get the default model for a provider.
    ///
    /// Uses model_db to look up the most capable model for each provider,
    /// with fallbacks for providers not in model_db.
    fn default_model_for_provider(&self, provider_name: &str) -> Model {
        // Define the preferred default model IDs for each major provider
        let default_model_id = match provider_name {
            "openai" => "gpt-4o-mini",
            "anthropic" => "claude-sonnet-4-20250514",
            "google" => "gemini-2.0-flash",
            _ => return self.construct_model_from_id(provider_name, "default"),
        };

        // Try to get the model from model_db
        if let Some(entry) = crate::model_db::get_model_entry(provider_name, default_model_id) {
            return self.model_from_entry(entry);
        }

        // Fallback: try to get the first/last model from model_db for this provider
        let provider_models = crate::model_db::get_provider_models(provider_name);
        if !provider_models.is_empty() {
            // Use the last model (typically the most capable/latest)
            if let Some(entry) = provider_models.last() {
                return self.model_from_entry(entry);
            }
        }

        // Ultimate fallback: construct with sensible defaults
        self.construct_model_from_id(provider_name, "default")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use crate::context::Context;

    fn create_test_context() -> Context {
        let mut ctx = Context::new();
        ctx.add_message(Message::User(crate::UserMessage::new(
            "Help me write a function to reverse a string".to_string(),
        )));
        ctx
    }

    #[test]
    fn test_config_defaults() {
        let config = MultiProviderConfig::default();
        assert!(config.auto_routing);
        assert!(config.prefer_cost_efficient);
        assert_eq!(config.max_retries_per_model, 1);
        assert!(config.per_model_timeout.is_none());
        // Circuit breaker config defaults are tested in circuit_breaker module
    }

    #[test]
    fn test_config_builder() {
        let config = MultiProviderConfig::default()
            .with_auto_routing(false)
            .with_prefer_cost_efficient(false)
            .with_max_retries(3)
            .with_per_model_timeout(Duration::from_secs(30));

        assert!(!config.auto_routing);
        assert!(!config.prefer_cost_efficient);
        assert_eq!(config.max_retries_per_model, 3);
        assert_eq!(config.per_model_timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_multi_provider_creation() {
        let config = MultiProviderConfig::default();
        let provider = MultiProvider::new(config);

        assert_eq!(provider.name(), "multi-provider");
        assert!(provider.provider_names().is_empty());
    }

    #[test]
    fn test_register_provider() {
        let mut provider = MultiProvider::new(MultiProviderConfig::default());

        // Register a mock provider
        struct MockProvider;
        impl Provider for MockProvider {
            fn stream<'a>(
                &'a self,
                _model: &'a Model,
                _context: &'a Context,
                _options: Option<StreamOptions>,
            ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
                Box::pin(async move { unreachable!("Mock provider - not called in this test") })
            }

            fn name(&self) -> &str {
                "mock"
            }
        }

        let mock = Arc::new(MockProvider);
        provider.register_provider("test", mock);

        assert_eq!(provider.provider_names(), vec!["test"]);
        assert!(provider.get_provider("test").is_some());
        assert!(provider.get_breaker("test").is_some());
    }

    #[test]
    fn test_unregister_provider() {
        let mut provider = MultiProvider::new(MultiProviderConfig::default());

        struct MockProvider;
        impl Provider for MockProvider {
            fn stream<'a>(
                &'a self,
                _model: &'a Model,
                _context: &'a Context,
                _options: Option<StreamOptions>,
            ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
                Box::pin(async move { unreachable!("Mock provider") })
            }

            fn name(&self) -> &str {
                "mock"
            }
        }

        let mock = Arc::new(MockProvider);
        provider.register_provider("test", mock.clone());

        assert!(provider.unregister_provider("test"));
        assert!(provider.provider_names().is_empty());
        assert!(provider.get_provider("test").is_none());
    }

    #[test]
    fn test_with_router() {
        let router = DefaultRouter::new();
        let provider = MultiProvider::with_router(router);

        assert_eq!(provider.name(), "multi-provider");
    }

    #[test]
    fn test_with_fallback() {
        let fallback = FallbackChain::from_ids(&["openai/gpt-4o"]).unwrap();
        let provider = MultiProvider::new(MultiProviderConfig::default()).with_fallback(fallback);

        assert_eq!(provider.fallback().len(), 1);
    }

    #[test]
    fn test_circuit_breaker_diagnostics() {
        let mut provider = MultiProvider::new(MultiProviderConfig::default());

        struct MockProvider;
        impl Provider for MockProvider {
            fn stream<'a>(
                &'a self,
                _model: &'a Model,
                _context: &'a Context,
                _options: Option<StreamOptions>,
            ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
                Box::pin(async move { unreachable!("Mock provider") })
            }

            fn name(&self) -> &str {
                "mock"
            }
        }

        let mock = Arc::new(MockProvider);
        provider.register_provider("test", mock);

        let diagnostics = provider.circuit_breaker_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].provider, "test");
    }

    #[test]
    fn test_multi_provider_error_display() {
        let err = MultiProviderError::NoProviderForModel("gpt-4o".to_string());
        assert!(err.to_string().contains("gpt-4o"));

        let err = MultiProviderError::AllProvidersExhausted { errors: vec![] };
        assert!(err.to_string().contains("All providers exhausted"));

        let err = MultiProviderError::CircuitBreakerOpen {
            provider: "openai".to_string(),
            retry_after: Duration::from_secs(10),
        };
        assert!(err.to_string().contains("openai"));
        assert!(err.to_string().contains("10"));
    }

    #[test]
    fn test_multi_provider_error_helpers() {
        let err = MultiProviderError::CircuitBreakerOpen {
            provider: "openai".to_string(),
            retry_after: Duration::from_secs(10),
        };
        assert!(err.is_circuit_breaker());
        assert_eq!(err.retry_after(), Some(Duration::from_secs(10)));

        let err = MultiProviderError::AllProvidersExhausted { errors: vec![] };
        assert!(!err.is_circuit_breaker());
        assert_eq!(err.retry_after(), None);
    }

    #[test]
    fn test_diagnostics() {
        let mut provider = MultiProvider::new(MultiProviderConfig::default());

        struct MockProvider;
        impl Provider for MockProvider {
            fn stream<'a>(
                &'a self,
                _model: &'a Model,
                _context: &'a Context,
                _options: Option<StreamOptions>,
            ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
                Box::pin(async move { unreachable!("Mock provider") })
            }

            fn name(&self) -> &str {
                "mock"
            }
        }

        let mock = Arc::new(MockProvider);
        provider.register_provider("test", mock);

        let diag = provider.diagnostics();
        assert_eq!(diag.provider_count, 1);
        assert!(diag.auto_routing);
        assert!(diag.prefer_cost_efficient);
        assert_eq!(diag.circuit_breakers.len(), 1);
    }

    #[test]
    fn test_router_classification() {
        use crate::Complexity;
        let router = DefaultRouter::new();
        let provider = MultiProvider::with_router(router);

        let ctx = create_test_context();
        let complexity = provider.router().classify(&ctx);

        // "Help me write a function to reverse a string" should be Simple complexity
        assert!(complexity >= Complexity::Simple);
    }
}
