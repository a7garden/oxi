//! Scoring functions for routing decisions.

use super::signals::{BehavioralSignal, ContextBudgetSignal, StructuralSignal};
use super::types::ScoringWeights;

/// Sigmoid function with configurable center and steepness.
pub fn sigmoid(x: f64, center: f64, k: f64) -> f64 {
    let z = k * (x - center);
    if z > 500.0 {
        1.0
    } else if z < -500.0 {
        0.0
    } else {
        1.0 / (1.0 + (-z).exp())
    }
}

/// Linear interpolation between `a` and `b`.
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Compute a composite routing score from all signals.
pub fn compute_score(
    structural: &StructuralSignal,
    behavioral: &BehavioralSignal,
    budget: &ContextBudgetSignal,
    weights: &ScoringWeights,
) -> f64 {
    let s_raw = structural.normalized();
    let b_raw = behavioral.normalized();
    let c_raw = budget.normalized();

    // Sigmoid sharpening.
    let s_sharp = sigmoid(s_raw, 0.5, 4.0);
    let b_sharp = sigmoid(b_raw, 0.5, 4.0);
    let c_sharp = sigmoid(c_raw, 0.5, 4.0);

    let raw = weights.structural * s_sharp
        + weights.behavioral * b_sharp
        + weights.context_budget * c_sharp;
    let total = weights.structural + weights.behavioral + weights.context_budget;

    if total > 0.0 {
        (raw / total).clamp(0.0, 1.0)
    } else {
        0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigmoid_center() {
        let val = sigmoid(0.5, 0.5, 4.0);
        assert!((val - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sigmoid_high() {
        let val = sigmoid(1.0, 0.5, 4.0);
        assert!(val > 0.8, "sigmoid(1.0) = {}", val);
    }

    #[test]
    fn sigmoid_low() {
        let val = sigmoid(0.0, 0.5, 4.0);
        assert!(val < 0.2, "sigmoid(0.0) = {}", val);
    }

    #[test]
    fn sigmoid_overflow_guard() {
        assert!((sigmoid(1e6, 0.5, 1.0) - 1.0).abs() < 1e-6);
        assert!(sigmoid(-1e6, 0.5, 1.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_basic() {
        assert!((lerp(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((lerp(0.0, 10.0, 0.3) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_clamped() {
        assert!((lerp(0.0, 1.0, 1.5) - 1.0).abs() < 1e-6);
        assert!((lerp(0.0, 1.0, -0.5) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn compute_score_default_weights() {
        let structural = StructuralSignal::default();
        let behavioral = BehavioralSignal::default();
        let budget = ContextBudgetSignal::default();
        let weights = ScoringWeights::default();
        let score = compute_score(&structural, &behavioral, &budget, &weights);
        assert!(
            (0.0..=1.0).contains(&score),
            "score out of range: {}",
            score
        );
    }

    #[test]
    fn compute_score_clamped() {
        let structural = StructuralSignal {
            message_count: 100,
            tool_call_count: 50,
            tool_result_count: 50,
            estimated_tokens: 500_000,
            user_message_count: 20,
        };
        let behavioral = BehavioralSignal::default();
        let budget = ContextBudgetSignal {
            estimated_tokens: 500_000,
            accumulated_cost: 10.0,
            budget_limit: Some(5.0),
            context_upgrade_threshold: Some(50_000),
        };
        let weights = ScoringWeights::default();
        let score = compute_score(&structural, &behavioral, &budget, &weights);
        assert!(
            (0.0..=1.0).contains(&score),
            "score out of range: {}",
            score
        );
    }
}
