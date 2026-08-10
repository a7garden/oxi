//! Model auto-routing for oxicode.
//!
//! Provides intelligent model selection based on conversation structure
//! and behavioral signals. This is an **opt-in** feature — the user must
//! explicitly select `router/auto` or another profile in model selection.

#![allow(missing_docs)]

pub mod classifier;
pub mod fallback;
pub mod profiles;
pub mod scoring;
pub mod signals;
pub mod types;

use crate::context::Context;
use crate::error::ProviderError;
use crate::messages::Message;
use crate::providers::ProviderRegistry;
use crate::providers::StreamOptions;
use crate::types::Model;
use crate::{Api, Provider, StreamResult, ThinkingLevel, register_model, register_provider};

/// Global router state snapshot — updated after each routing decision.
static ROUTER_SNAPSHOT: parking_lot::RwLock<Option<RouterSnapshot>> =
    parking_lot::RwLock::new(None);

/// Global pin tier override — set by /router pin command.
static ROUTER_PIN_TIER: parking_lot::RwLock<Option<RouterTier>> = parking_lot::RwLock::new(None);

/// Snapshot of the current router state for UI display.
#[derive(Debug, Clone, Default)]
pub struct RouterSnapshot {
    pub last_tier: Option<RouterTier>,
    pub last_score: f64,
    pub last_model: Option<String>,
    pub last_provider: Option<String>,
    pub accumulated_cost: f64,
    pub turn_count: usize,
    pub profile: Option<String>,
}
use parking_lot::RwLock;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use fallback::FallbackChain;
pub use profiles::{ProviderModel, RouterProfiles, parse_tier_model};
pub use scoring::{compute_score, lerp};
pub use signals::{
    BehavioralSignal, ContextBudgetSignal, MessageContentSignal, StructuralSignal, VisionSignal,
};
pub use types::{
    DecisionMethod, RoutedTierConfig, RouterConfig, RouterPhase, RouterProfile, RouterState,
    RouterTier, RoutingDecision, RoutingScore, ScoringWeights,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn message_chars(msg: &Message) -> usize {
    match msg {
        Message::User(u) => u.content.as_str().map(|s| s.len()).unwrap_or(0),
        Message::Assistant(a) => a
            .content
            .iter()
            .map(|b| b.as_text().map(|t| t.len()).unwrap_or(0))
            .sum(),
        Message::ToolResult(t) => t
            .content
            .iter()
            .map(|b| b.as_text().map(|t| t.len()).unwrap_or(0))
            .sum(),
    }
}

fn build_target_model(pm: &ProviderModel, reasoning: bool) -> Model {
    let mut m = Model::new(
        &pm.model_id,
        &pm.model_id,
        Api::AnthropicMessages,
        &pm.provider,
        "",
    );
    m.reasoning = reasoning;
    m
}

// ── Pipeline ─────────────────────────────────────────────────────────────────

/// Routing pipeline state for a session.
#[derive(Debug)]
pub struct RouterPipeline {
    pub weights: ScoringWeights,
    decision_history: Vec<RoutingDecision>,
    accumulated_cost: f64,
    budget_limit: Option<f64>,
    context_upgrade_threshold: Option<usize>,
    last_score: f64,
    pin_tier: Option<RouterTier>,
    phase_bias: f64,
    classifier_model: Option<String>,
}

impl RouterPipeline {
    pub fn new() -> Self {
        Self {
            weights: ScoringWeights::default(),
            decision_history: Vec::new(),
            accumulated_cost: 0.0,
            budget_limit: None,
            context_upgrade_threshold: None,
            last_score: 0.5,
            pin_tier: None,
            phase_bias: 0.5,
            classifier_model: None,
        }
    }

    pub fn from_config(config: &RouterConfig) -> Self {
        Self {
            weights: config.weights.clone(),
            decision_history: Vec::new(),
            accumulated_cost: 0.0,
            budget_limit: config.max_session_budget,
            context_upgrade_threshold: config.context_upgrade_threshold,
            last_score: 0.5,
            pin_tier: config.pin_tier,
            phase_bias: config.phase_bias.unwrap_or(0.5).clamp(0.0, 1.0),
            classifier_model: config.classifier_model.clone(),
        }
    }

    /// Route a context to (score, tier, phase, method).
    ///
    /// Decision cascade:
    ///   1. Override? (pin / rule / scenario / context) → return immediately
    ///   2. Signal fusion scoring (5 signals) → heuristic score → tier
    ///   3. The caller (stream) handles LLM classification for ambiguous scores.
    pub fn route(&mut self, context: &Context) -> (f64, RouterTier, RouterPhase, DecisionMethod) {
        // ── Layer 0: Override check ────────────────────────────────────────
        if let Some((tier, method)) = self.check_override(context) {
            let phase = BehavioralSignal::extract(&context.messages, &self.decision_history).phase;
            return (self.last_score, tier, phase, method);
        }

        // ── Layer 1: Signal fusion scoring ─────────────────────────────────
        let structural = StructuralSignal::extract(&context.messages);
        let behavioral = BehavioralSignal::extract(&context.messages, &self.decision_history);
        let budget = ContextBudgetSignal::extract(
            structural.estimated_tokens,
            self.accumulated_cost,
            self.budget_limit,
            self.context_upgrade_threshold,
        );
        let message = MessageContentSignal::extract(&context.messages);

        let raw_score = compute_score(
            &structural,
            &behavioral,
            &budget,
            None,
            Some(&message),
            &self.weights,
        );

        // Apply phase bias — blend toward previous score
        let blended = if self.decision_history.is_empty() {
            raw_score
        } else {
            let prev = self.last_score;
            lerp(raw_score, prev, self.phase_bias * 0.3) // dampened bias
        };

        self.last_score = blended;

        let score = RoutingScore(blended);
        let mut tier = score.to_tier(0.65, 0.35);
        let mut method = DecisionMethod::Heuristic;

        // Context upgrade
        if budget.should_upgrade_context() && tier != RouterTier::High {
            tier = RouterTier::High;
            method = DecisionMethod::ContextUpgrade;
        }
        // Budget downgrade
        if budget.is_over_budget() && tier == RouterTier::High {
            tier = RouterTier::Medium;
            method = DecisionMethod::BudgetDowngrade;
        }

        (blended, tier, behavioral.phase, method)
    }

    /// Layer 0: Check for definitive overrides.
    fn check_override(&self, _context: &Context) -> Option<(RouterTier, DecisionMethod)> {
        // Global pin (set by /router pin)
        if let Some(tier) = *ROUTER_PIN_TIER.read() {
            return Some((tier, DecisionMethod::PinOverride));
        }
        // Config pin
        if let Some(tier) = self.pin_tier {
            return Some((tier, DecisionMethod::PinOverride));
        }
        None
    }

    pub fn record_decision(&mut self, decision: RoutingDecision) {
        self.decision_history.push(decision);
        if self.decision_history.len() > 20 {
            self.decision_history.remove(0);
        }
    }

    pub fn record_turn_cost(&mut self, cost: f64) {
        self.accumulated_cost += cost;
    }

    pub fn accumulated_cost(&self) -> f64 {
        self.accumulated_cost
    }
    pub fn last_score(&self) -> f64 {
        self.last_score
    }
    pub fn history(&self) -> &[RoutingDecision] {
        &self.decision_history
    }

    /// Get the classifier model identifier, if configured.
    pub fn classifier_model(&self) -> Option<String> {
        self.classifier_model.clone()
    }
}

impl Default for RouterPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ── Provider ───────────────────────────────────────────────────────────────────

/// Router provider implementing the [`Provider`] trait.
///
/// Registers as provider `"router"` and accepts profile names as model IDs
/// (e.g. `"auto"`, `"budget"`). Delegation uses global provider resolution
/// (`get_provider_arc`) when `use_global_resolution` is true.
pub struct RouterProvider {
    pipeline: RwLock<RouterPipeline>,
    profiles: RwLock<RouterProfiles>,
    provider_registry: Arc<ProviderRegistry>,
    use_global_resolution: bool,
}

impl RouterProvider {
    /// Create with an instance-based ProviderRegistry.
    pub fn new(config: &RouterConfig, registry: Arc<ProviderRegistry>) -> Self {
        Self {
            pipeline: RwLock::new(RouterPipeline::from_config(config)),
            profiles: RwLock::new(RouterProfiles::from_config(config)),
            provider_registry: registry,
            use_global_resolution: false,
        }
    }

    /// Create with global provider resolution (for CLI use).
    pub fn new_global(config: &RouterConfig) -> Self {
        Self {
            pipeline: RwLock::new(RouterPipeline::from_config(config)),
            profiles: RwLock::new(RouterProfiles::from_config(config)),
            provider_registry: Arc::new(ProviderRegistry::new()),
            use_global_resolution: true,
        }
    }

    /// Hot-reload configuration at runtime.
    pub fn reload_config(&self, config: &RouterConfig) {
        let mut p = self.pipeline.write();
        p.weights = config.weights.clone();
        p.budget_limit = config.max_session_budget;
        p.context_upgrade_threshold = config.context_upgrade_threshold;
        drop(p);
        *self.profiles.write() = RouterProfiles::from_config(config);
    }

    /// Resolve a target provider (instance or global fallback).
    fn resolve_provider(&self, name: &str) -> Option<Arc<dyn Provider>> {
        if self.use_global_resolution {
            crate::get_provider_arc(name)
        } else {
            self.provider_registry.get(name)
        }
    }

    /// Resolve model from registry (instance or global fallback).
    #[allow(dead_code)]
    fn resolve_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        crate::lookup_model(provider, model_id)
    }

    /// Update the global snapshot after a routing decision.
    fn update_snapshot(&self) {
        let p = self.pipeline.read();
        let last = p.history().last();
        let mut snap = ROUTER_SNAPSHOT.write();
        *snap = Some(RouterSnapshot {
            last_tier: last.map(|d| d.tier),
            last_score: p.last_score(),
            last_model: last.map(|d| d.target_label.clone()),
            last_provider: last.map(|d| d.target_provider.clone()),
            accumulated_cost: p.accumulated_cost(),
            turn_count: p.history().len(),
            profile: last.map(|d| d.profile.clone()),
        });
    }

    /// Get the current router state snapshot (global).
    pub fn get_snapshot() -> Option<RouterSnapshot> {
        ROUTER_SNAPSHOT.read().clone()
    }

    /// Route with vision awareness — extracts VisionSignal and adjusts tier/model.
    ///
    /// Returns `(vision_signal, adjusted_tier_config)`.
    pub fn route_with_vision(
        &self,
        context: &Context,
        profile_name: &str,
        tier: RouterTier,
    ) -> (VisionSignal, RoutedTierConfig) {
        // Extract vision signal from recent messages
        let vision = VisionSignal::extract(&context.messages, 10);

        // Get base tier config
        let tier_config = self
            .profiles
            .read()
            .tier_config(profile_name, tier)
            .cloned()
            .unwrap_or_else(|| RoutedTierConfig {
                model: String::new(),
                thinking: None,
                fallbacks: vec![],
            });

        // If vision is required, ensure we have a vision-capable model
        if vision.requires_vision() {
            let adjusted = self.ensure_vision_model(tier_config, tier, profile_name);
            (vision, adjusted)
        } else {
            (vision, tier_config)
        }
    }

    /// Ensure the selected model supports vision.
    ///
    /// Fallback priority:
    /// 1. Current model already supports vision → keep it
    /// 2. Fallback list contains a vision model → swap
    /// 3. Higher tier has a vision model → upgrade
    /// 4. No vision model found → warn and keep original
    fn ensure_vision_model(
        &self,
        tier_config: RoutedTierConfig,
        tier: RouterTier,
        profile_name: &str,
    ) -> RoutedTierConfig {
        // 1. Current model already supports vision?
        if let Some(pm) = parse_tier_model(&tier_config)
            && let Some(model) = crate::lookup_model(&pm.provider, &pm.model_id)
            && model.supports_vision()
        {
            return tier_config;
        }

        let original_model = tier_config.model.clone();
        let thinking = tier_config.thinking;
        let fallbacks = tier_config.fallbacks.clone();

        // 2. Fallback list contains a vision model?
        for fb in &fallbacks {
            if let Some(pm) = ProviderModel::parse(fb)
                && let Some(model) = crate::lookup_model(&pm.provider, &pm.model_id)
                && model.supports_vision()
            {
                tracing::info!(
                    "Vision override: {} → {} (vision-capable fallback)",
                    original_model,
                    fb
                );
                return RoutedTierConfig {
                    model: fb.clone(),
                    thinking,
                    fallbacks,
                };
            }
        }

        // 3. Higher tier has a vision model?
        let profiles = self.profiles.read();
        if let Some(profile) = profiles.get_with_fallback(profile_name) {
            for higher_tier in [RouterTier::High, RouterTier::Medium] {
                if higher_tier.rank() > tier.rank() {
                    let tc = profile.tier_config(higher_tier);
                    if let Some(pm) = parse_tier_model(tc)
                        && let Some(model) = crate::lookup_model(&pm.provider, &pm.model_id)
                        && model.supports_vision()
                    {
                        tracing::info!(
                            "Vision upgrade: tier {:?} → {:?}, model {} → {}",
                            tier,
                            higher_tier,
                            original_model,
                            tc.model
                        );
                        return tc.clone();
                    }
                }
            }
        }

        // 4. No vision model found — warn and keep original
        tracing::warn!(
            "Vision required but no vision-capable model found for tier {:?}. \
             Model {} may fail with image content.",
            tier,
            original_model
        );
        tier_config
    }
}

impl Provider for RouterProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let profile_name = &model.id;

            // 1. Route through pipeline.
            let (score, tier, phase, method) = self.pipeline.write().route(context);

            // 2. Resolve tier config.
            let tier_config = match self
                .profiles
                .read()
                .tier_config(profile_name, tier)
                .cloned()
            {
                Some(tc) => tc,
                None => {
                    return Err(ProviderError::StreamError(format!(
                        "Router profile '{}' is not configured. \
                         Run /router setup or edit ~/.oxicode/settings.toml",
                        profile_name
                    )));
                }
            };

            // 3. LLM classifier for ambiguous scores (optional).
            let (score, tier, method) = {
                let classifier_model = self.pipeline.read().classifier_model();
                if (0.25..0.75).contains(&score) && classifier_model.is_some() {
                    let input = classifier::ClassifierInput {
                        message: context
                            .messages
                            .iter()
                            .rev()
                            .find_map(|m| match m {
                                Message::User(u) => {
                                    Some(u.content.as_str().unwrap_or("").to_string())
                                }
                                _ => None,
                            })
                            .unwrap_or_default(),
                        context_tokens: 0, // filled below
                        turn_count: self.pipeline.read().history().len(),
                        available_tools: vec![],
                    };
                    let llm = classifier::LlmClassifier::new(classifier_model);
                    match llm.classify(&input, score).await {
                        Ok(llm_score) => {
                            let llm_tier = RoutingScore(llm_score).to_tier(0.65, 0.35);
                            tracing::info!(
                                "LLM classifier: score {score:.2} → {llm_score:.2}, tier {tier:?} → {llm_tier:?}"
                            );
                            (llm_score, llm_tier, DecisionMethod::LlmClassifier)
                        }
                        Err(e) => {
                            tracing::warn!("LLM classifier failed: {e}, using heuristic score");
                            (score, tier, method)
                        }
                    }
                } else {
                    (score, tier, method)
                }
            };

            // 3b. Vision-aware model override. When recent messages carry image
            // content (e.g. a `browse` screenshot), ensure the resolved model is
            // vision-capable — swapping to a vision fallback or upgrading the tier
            // via `ensure_vision_model`. No-op when no images are present. This
            // activates the VisionSignal infrastructure (signals.rs) that was
            // previously built but never wired into the routing hot path.
            let vision = VisionSignal::extract(&context.messages, 10);
            let is_vision_triggered = vision.requires_vision();
            let vision_images = vision.recent_image_count;
            let tier_config = if is_vision_triggered {
                self.ensure_vision_model(tier_config, tier, profile_name)
            } else {
                tier_config
            };

            // 4. Resolve tier config.
            let pm = parse_tier_model(&tier_config).unwrap_or_else(|| ProviderModel {
                provider: model.provider.clone(),
                model_id: model.id.clone(),
            });

            // 4. Record decision.
            let decision = RoutingDecision {
                profile: profile_name.clone(),
                tier,
                phase,
                target_provider: pm.provider.clone(),
                target_model_id: pm.model_id.clone(),
                target_label: tier_config.model.clone(),
                reasoning: format!(
                    "tier={tier:?}, score={score:.2}, method={method:?}, provider={}, model={}",
                    pm.provider, pm.model_id
                ),
                thinking: tier_config.thinking.unwrap_or(ThinkingLevel::Off),
                timestamp: chrono::Utc::now().timestamp_millis(),
                score,
                is_fallback: false,
                is_context_triggered: method == DecisionMethod::ContextUpgrade,
                is_budget_forced: method == DecisionMethod::BudgetDowngrade,
                is_vision_triggered,
                vision_images,
                decision_method: method,
            };
            self.pipeline.write().record_decision(decision);
            self.update_snapshot();

            // 4. Resolve target provider.
            let target_provider = self
                .resolve_provider(&pm.provider)
                .ok_or_else(|| ProviderError::UnknownProvider(pm.provider.clone()))?;

            // 5. Build target model.
            let target_model = build_target_model(&pm, tier_config.thinking.is_some());

            // 6. Inject thinking.
            let mut opts = options.unwrap_or_default();
            if let Some(thinking) = tier_config.thinking {
                opts.thinking_level = Some(thinking);
            }

            // 7. Truncate context if needed (preserve first message — system prompt).
            let estimated_chars: usize = context.messages.iter().map(message_chars).sum();
            let estimated_tokens = estimated_chars / 4;
            let adjusted_context = if estimated_tokens > target_model.context_window {
                let mut ctx = context.clone();
                let max_chars = target_model.context_window * 3;
                let mut total: usize = ctx.messages.iter().map(message_chars).sum();
                while total > max_chars && ctx.messages.len() > 2 {
                    let removed = ctx.messages.remove(1);
                    total -= message_chars(&removed);
                }
                ctx
            } else {
                context.clone()
            };

            // 8. Try primary model.
            match target_provider
                .stream(&target_model, &adjusted_context, Some(opts.clone()))
                .await
            {
                Ok(stream) => Ok(stream),
                Err(primary_err) => {
                    if tier_config.fallbacks.is_empty() {
                        return Err(primary_err);
                    }
                    let chain = FallbackChain::new(tier_config.fallbacks.clone());
                    chain
                        .try_models_with_resolver(
                            |name| self.resolve_provider(name),
                            &adjusted_context,
                            Some(opts),
                        )
                        .await
                }
            }
        })
    }
}

// ── Registration ───────────────────────────────────────────────────────────────

/// Register the router provider and its profile models.
///
/// This is **opt-in** — the router is only active when the user selects
/// `router/auto` or another profile in model selection.
pub fn register_router(config: &RouterConfig) {
    let provider = RouterProvider::new_global(config);
    register_provider("router", provider);
    tracing::info!("Model router registered (opt-in: select router/auto)");
    for name in config.profiles.keys() {
        let model = Model::new(
            name,
            format!("Router ({name})"),
            Api::AnthropicMessages,
            "router",
            "router://local",
        );
        register_model(model);
        tracing::debug!("Registered router model: router/{}", name);
    }
}

/// Set the global router pin tier. Used by `/router pin <tier>`.
pub fn set_router_pin(tier: Option<RouterTier>) {
    *ROUTER_PIN_TIER.write() = tier;
}

/// Get the current global router pin tier.
pub fn get_router_pin() -> Option<RouterTier> {
    *ROUTER_PIN_TIER.read()
}

#[cfg(test)]
mod vision_routing_tests {
    //! These tests cover the vision model-swap that `stream()` now activates.
    //! `VisionSignal` extraction itself is covered in `signals::vision_tests`;
    //! here we prove `route_with_vision` (the pub wrapper around the exact
    //! `ensure_vision_model` logic the hot path calls) performs a real swap.
    use super::types::RouterTier;
    use super::{RoutedTierConfig, RouterConfig, RouterProfile, RouterProvider};
    use crate::context::Context;
    use crate::messages::{ContentBlock, ImageContent, Message, MessageContent, UserMessage};
    use crate::providers::ProviderRegistry;
    use crate::register_model;
    use crate::types::{Api, InputModality, Model};
    use std::sync::Arc;

    /// Register a text-only model and a vision-capable model under a test
    /// provider so `ensure_vision_model` can observe a real swap via the
    /// global model registry (nextest isolates each test in its own process,
    /// so the global mutation is safe).
    fn register_test_models() {
        let novision = Model::new(
            "novision",
            "NoVision",
            Api::OpenAiCompletions,
            "testvision",
            "http://localhost",
        ); // input defaults to [Text] — no vision
        let mut sees = Model::new(
            "sees",
            "Sees",
            Api::OpenAiCompletions,
            "testvision",
            "http://localhost",
        );
        sees.input.push(InputModality::Image);
        register_model(novision);
        register_model(sees);
    }

    /// Profile whose low tier is text-only with a vision-capable fallback.
    fn swap_config() -> RouterConfig {
        let mut config = RouterConfig::default();
        config.profiles.insert(
            "auto".to_string(),
            RouterProfile {
                high: RoutedTierConfig {
                    model: "testvision/novision".to_string(),
                    thinking: None,
                    fallbacks: vec![],
                },
                medium: RoutedTierConfig {
                    model: "testvision/novision".to_string(),
                    thinking: None,
                    fallbacks: vec![],
                },
                low: RoutedTierConfig {
                    model: "testvision/novision".to_string(),
                    thinking: None,
                    fallbacks: vec!["testvision/sees".to_string()],
                },
            },
        );
        config
    }

    fn image_context() -> Context {
        let mut ctx = Context::new();
        ctx.messages
            .push(Message::User(UserMessage::new(MessageContent::Blocks(
                vec![ContentBlock::Image(ImageContent::new("fake", "image/png"))],
            ))));
        ctx
    }

    /// Image-bearing context routed to a text-only tier model must swap to its
    /// vision-capable fallback — the core contract the hot-path wiring delivers.
    #[test]
    fn route_with_vision_swaps_to_vision_fallback() {
        register_test_models();
        let provider = RouterProvider::new(&swap_config(), Arc::new(ProviderRegistry::new()));

        let (vision, tier_config) =
            provider.route_with_vision(&image_context(), "auto", RouterTier::Low);

        assert!(
            vision.requires_vision(),
            "image context must require vision"
        );
        assert_eq!(
            tier_config.model, "testvision/sees",
            "non-vision low-tier model must swap to the vision fallback"
        );
        assert_eq!(vision.recent_image_count, 1);
    }

    /// No images → no vision requirement → model unchanged.
    #[test]
    fn route_with_vision_keeps_model_without_images() {
        register_test_models();
        let provider = RouterProvider::new(&swap_config(), Arc::new(ProviderRegistry::new()));

        let mut ctx = Context::new();
        ctx.messages
            .push(Message::User(UserMessage::new("just text")));

        let (vision, tier_config) = provider.route_with_vision(&ctx, "auto", RouterTier::Low);

        assert!(!vision.requires_vision());
        assert_eq!(tier_config.model, "testvision/novision");
    }
}
