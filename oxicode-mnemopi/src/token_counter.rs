//! Token counting + cost estimation — ported from omp `core/token-counter.ts`.
//!
//! Heuristic token estimator (`text.len() / 4`) and a per-model USD pricing
//! table for back-of-envelope cost estimates on recall workloads.
//!
//! MIT — attribution: adapted from [omp](https://github.com/earendil-works/pi)
//! `packages/mnemopi/src/core/token-counter.ts`.

use serde::{Deserialize, Serialize};

/// Fallback rate (USD per 1M tokens) for unknown models.
const DEFAULT_RATE_PER_1M: f64 = 3.0;

/// Per-model USD pricing (per 1M tokens).
fn pricing_for(model: &str) -> f64 {
    match model {
        "claude-sonnet-4" => 3.0,
        "claude-haiku" => 0.8,
        "gpt-4o" => 2.5,
        "gpt-4o-mini" => 0.15,
        _ => DEFAULT_RATE_PER_1M,
    }
}

/// Cost estimate for `tokens` against `model`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub tokens: usize,
    pub model: String,
    pub cost_usd: f64,
    pub rate_per_1m: f64,
}

/// Estimate token count from raw text using the `len/4` heuristic.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.len() / 4
}

/// Estimate USD cost for `tokens` against `model`, rounded to 6 decimals.
pub fn estimate_cost(tokens: usize, model: &str) -> CostEstimate {
    let rate = pricing_for(model);
    let cost = (tokens as f64 / 1_000_000.0) * rate;
    CostEstimate {
        tokens,
        model: model.to_string(),
        cost_usd: (cost * 1_000_000.0).round() / 1_000_000.0,
        rate_per_1m: rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        // empty
        assert_eq!(estimate_tokens(""), 0);
        // short (2/4 == 0, 11/4 == 2)
        assert_eq!(estimate_tokens("hi"), 0);
        assert_eq!(estimate_tokens("hello world"), 2);
        // long
        let text = "a".repeat(10_000);
        assert_eq!(estimate_tokens(&text), 2_500);
    }

    #[test]
    fn test_estimate_cost() {
        // known model
        let known = estimate_cost(1_000_000, "gpt-4o");
        assert_eq!(known.tokens, 1_000_000);
        assert_eq!(known.model, "gpt-4o");
        assert_eq!(known.rate_per_1m, 2.5);
        assert!((known.cost_usd - 2.5).abs() < 1e-9);
        // unknown model falls back to DEFAULT_RATE_PER_1M
        let unknown = estimate_cost(1_000_000, "unknown-model-xyz");
        assert_eq!(unknown.rate_per_1m, DEFAULT_RATE_PER_1M);
        assert!((unknown.cost_usd - DEFAULT_RATE_PER_1M).abs() < 1e-9);
    }

    #[test]
    fn test_pricing() {
        assert_eq!(pricing_for("claude-sonnet-4"), 3.0);
        assert_eq!(pricing_for("claude-haiku"), 0.8);
        assert_eq!(pricing_for("gpt-4o"), 2.5);
        assert_eq!(pricing_for("gpt-4o-mini"), 0.15);
        assert_eq!(pricing_for(""), DEFAULT_RATE_PER_1M);
    }
}
