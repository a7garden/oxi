//! Router type definitions — tiers, phases, decisions, and configuration.

use crate::ThinkingLevel;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Tiers & Phases ─────────────────────────────────────────────────────────────

/// Routing tier representing model capability level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RouterTier {
    /// Highest capability — research, architecture, complex reasoning.
    High,
    /// Medium capability — standard coding, multi-step tasks.
    Medium,
    /// Lowest capability — simple Q&A, formatting, trivial edits.
    Low,
}

impl RouterTier {
    /// Returns a stable ordering value.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
        }
    }
}

/// Phase of the conversation influencing routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RouterPhase {
    /// Default: active coding / tool-use phase.
    #[default]
    Implementation,
    /// Planning / design / exploration phase.
    Planning,
    /// Lightweight / lookup phase.
    Lightweight,
}

impl RouterPhase {
    /// Returns a numeric weight for scoring.
    pub fn weight(&self) -> f64 {
        match self {
            Self::Planning => 0.8,
            Self::Implementation => 0.5,
            Self::Lightweight => 0.2,
        }
    }
}

// ── Decision Method ──────────────────────────────────────────────────────────

/// How the routing decision was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionMethod {
    /// Heuristic scoring from structural / behavioral signals.
    Heuristic,
    /// LLM-based classifier (future).
    LlmClassifier,
    /// Explicit pin override from user config.
    PinOverride,
    /// Automatic upgrade due to context length.
    ContextUpgrade,
    /// Automatic downgrade due to budget constraints.
    BudgetDowngrade,
}

// ── Routing Score ─────────────────────────────────────────────────────────────

/// Score produced by signal aggregation, mapped to a [`RouterTier`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoutingScore(pub f64);

impl RoutingScore {
    /// Create a new score, clamped to `[0.0, 1.0]`.
    pub fn new(raw: f64) -> Self {
        Self(raw.clamp(0.0, 1.0))
    }

    /// Map the score to a routing tier using configurable thresholds.
    pub fn to_tier(&self, high_threshold: f64, low_threshold: f64) -> RouterTier {
        if self.0 >= high_threshold {
            RouterTier::High
        } else if self.0 <= low_threshold {
            RouterTier::Low
        } else {
            RouterTier::Medium
        }
    }

    /// Returns `true` if the score is close enough to a threshold boundary
    /// that an LLM classifier could refine the decision.
    pub fn needs_refinement(&self, margin: f64) -> bool {
        let near_high = (self.0 - 0.65).abs() < margin;
        let near_low = (self.0 - 0.35).abs() < margin;
        near_high || near_low
    }

    /// Raw score value.
    pub fn raw(&self) -> f64 {
        self.0
    }
}

// ── Routing Decision ──────────────────────────────────────────────────────────

/// A single routing decision recording what was chosen and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Profile name used for this decision.
    pub profile: String,
    /// Selected tier.
    pub tier: RouterTier,
    /// Detected conversation phase.
    pub phase: RouterPhase,
    /// Target provider name (e.g. `"anthropic"`).
    pub target_provider: String,
    /// Target model identifier (e.g. `"claude-sonnet-4"`).
    pub target_model_id: String,
    /// Human-readable label for the target model.
    pub target_label: String,
    /// Short explanation of why this tier was chosen.
    pub reasoning: String,
    /// Thinking level to apply.
    pub thinking: ThinkingLevel,
    /// Unix-epoch milliseconds when the decision was made.
    pub timestamp: i64,
    /// Raw routing score `[0, 1]`.
    pub score: f64,
    /// Whether this is a fallback from a previous failure.
    pub is_fallback: bool,
    /// Whether the decision was influenced by context length.
    pub is_context_triggered: bool,
    /// Whether the decision was forced by budget constraints.
    pub is_budget_forced: bool,
    /// Method used to make the decision.
    pub decision_method: DecisionMethod,
}

// ── Tier Config ──────────────────────────────────────────────────────────────

/// Configuration for a single tier within a profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutedTierConfig {
    /// Model in `"provider/model-id"` format (e.g. `"anthropic/claude-sonnet-4"`).
    pub model: String,
    /// Optional thinking level override for this tier.
    #[serde(default)]
    pub thinking: Option<ThinkingLevel>,
    /// Ordered list of fallback model strings (`"provider/model-id"`).
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// A named routing profile mapping tiers to model configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterProfile {
    /// High-tier model configuration.
    pub high: RoutedTierConfig,
    /// Medium-tier model configuration.
    pub medium: RoutedTierConfig,
    /// Low-tier model configuration.
    pub low: RoutedTierConfig,
}

impl RouterProfile {
    /// Get the tier config for a given tier.
    pub fn tier_config(&self, tier: RouterTier) -> &RoutedTierConfig {
        match tier {
            RouterTier::High => &self.high,
            RouterTier::Medium => &self.medium,
            RouterTier::Low => &self.low,
        }
    }
}

// ── Scoring Weights ──────────────────────────────────────────────────────────

/// Weights for combining routing signals into a composite score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringWeights {
    /// Weight for structural signal (message/tool count, context size).
    #[serde(default = "default_structural")]
    pub structural: f64,
    /// Weight for behavioral signal (phase detection, tool density).
    #[serde(default = "default_behavioral")]
    pub behavioral: f64,
    /// Weight for context/budget signal.
    #[serde(default = "default_context")]
    pub context_budget: f64,
}

fn default_structural() -> f64 {
    0.4
}
fn default_behavioral() -> f64 {
    0.35
}
fn default_context() -> f64 {
    0.25
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            structural: default_structural(),
            behavioral: default_behavioral(),
            context_budget: default_context(),
        }
    }
}

// ── Router Config ─────────────────────────────────────────────────────────────

/// Top-level router configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Default profile name to use.
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
    /// Optional classifier model for LLM-based routing (future).
    #[serde(default)]
    pub classifier_model: Option<String>,
    /// Context token threshold that triggers automatic upgrade.
    #[serde(default)]
    pub context_upgrade_threshold: Option<usize>,
    /// Maximum session budget in dollars (optional).
    #[serde(default)]
    pub max_session_budget: Option<f64>,
    /// Named routing profiles.
    #[serde(default)]
    pub profiles: HashMap<String, RouterProfile>,
    /// Scoring weights.
    #[serde(default)]
    pub weights: ScoringWeights,
}

fn default_profile_name() -> String {
    "auto".to_string()
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            default_profile: default_profile_name(),
            classifier_model: None,
            context_upgrade_threshold: None,
            max_session_budget: None,
            profiles: HashMap::new(),
            weights: ScoringWeights::default(),
        }
    }
}

impl RouterConfig {
    /// Construct a full router config from loaded parts.
    pub fn new(
        default_profile: String,
        classifier_model: Option<String>,
        context_upgrade_threshold: Option<usize>,
        max_session_budget: Option<f64>,
        profiles: HashMap<String, RouterProfile>,
        weights: ScoringWeights,
    ) -> Self {
        Self {
            default_profile,
            classifier_model,
            context_upgrade_threshold,
            max_session_budget,
            profiles,
            weights,
        }
    }
}

/// Session-scoped router state (for persistence across restarts).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouterState {
    /// Accumulated session cost in dollars.
    #[serde(default)]
    pub accumulated_cost: f64,
    /// Decision history for this session.
    #[serde(default)]
    pub decision_history: Vec<RoutingDecision>,
}
