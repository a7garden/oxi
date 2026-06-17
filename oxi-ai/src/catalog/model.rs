//! Model metadata structures — used by the materialize pipeline.

use serde::{Deserialize, Serialize};

use super::provider::AuthMethod;

/// A single built-in model entry.
///
/// Produced by [`crate::catalog::materialize::materialize`] from the
/// models.dev catalog. The TOML-based path that previously populated
/// these entries has been removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinModelEntry {
    /// Model identifier (e.g., "claude-sonnet-4-20250514")
    pub id: String,
    /// Human-readable model name (e.g., "Claude Sonnet 4")
    pub name: String,
    /// API protocol to use
    pub api: String,
    /// Provider name (e.g., "anthropic", "openai")
    pub provider: String,
    /// Whether this model supports reasoning/thinking
    #[serde(default)]
    pub reasoning: bool,
    /// Supported input modalities
    #[serde(default)]
    pub input: Vec<String>,
    /// Cost per million input tokens (USD)
    #[serde(default)]
    pub cost_input: f64,
    /// Cost per million output tokens (USD)
    #[serde(default)]
    pub cost_output: f64,
    /// Cost per million cached read tokens (USD)
    #[serde(default)]
    pub cost_cache_read: f64,
    /// Cost per million cached write tokens (USD)
    #[serde(default)]
    pub cost_cache_write: f64,
    /// Maximum context window in tokens
    #[serde(default)]
    pub context_window: u32,
    /// Maximum output tokens
    #[serde(default)]
    pub max_tokens: u32,
    /// Authentication method (overrides provider default).
    #[serde(default)]
    pub auth_method: AuthMethod,
    /// Per-model base URL override (None = inherit from provider).
    #[serde(default)]
    pub base_url: Option<String>,
}

impl BuiltinModelEntry {
    /// Check if this model supports image/vision input.
    pub fn supports_vision(&self) -> bool {
        self.input.iter().any(|m| m == "image" || m == "Image")
    }

    /// Check if this model supports reasoning/thinking.
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning
    }

    /// Calculate the cost for a given token usage.
    pub fn calculate_cost(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> f64 {
        let in_cost = (input_tokens as f64 / 1_000_000.0) * self.cost_input;
        let out_cost = (output_tokens as f64 / 1_000_000.0) * self.cost_output;
        let cr_cost = (cache_read as f64 / 1_000_000.0) * self.cost_cache_read;
        let cw_cost = (cache_write as f64 / 1_000_000.0) * self.cost_cache_write;
        in_cost + out_cost + cr_cost + cw_cost
    }
}
