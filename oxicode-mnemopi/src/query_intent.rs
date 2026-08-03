//! Query intent classification — ported from omp `core/query-intent.ts`.
//!
//! Classifies queries into categories (temporal, factual, entity, preference,
//! procedural, general) and adjusts the vec/fts/importance weights accordingly.

use regex::Regex;
use std::sync::LazyLock;

/// Query intent category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum IntentCategory {
    Temporal,
    Factual,
    Entity,
    Preference,
    Procedural,
    #[default]
    General,
}

/// Classified query intent with confidence and weight biases.
#[derive(Debug, Clone)]
pub struct QueryIntent {
    pub category: IntentCategory,
    pub confidence: f32,
    pub signals: Vec<IntentCategory>,
    pub vec_bias: f32,
    pub fts_bias: f32,
    pub importance_bias: f32,
}

/// Weight biases per intent category — ported from omp `INTENT_WEIGHTS`.
fn intent_weights(category: IntentCategory) -> (f32, f32, f32) {
    match category {
        IntentCategory::Temporal => (0.6, 1.5, 0.8),
        IntentCategory::Factual => (1.0, 1.2, 0.9),
        IntentCategory::Entity => (1.1, 1.0, 1.3),
        IntentCategory::Preference => (0.9, 0.8, 1.5),
        IntentCategory::Procedural => (1.3, 0.9, 0.7),
        IntentCategory::General => (1.0, 1.0, 1.0),
    }
}

// ── Intent patterns (compiled once) ──────────────────────────────────────

struct IntentPatterns {
    temporal: Vec<Regex>,
    factual: Vec<Regex>,
    entity: Vec<Regex>,
    preference: Vec<Regex>,
    procedural: Vec<Regex>,
}

static PATTERNS: LazyLock<IntentPatterns> = LazyLock::new(|| {
    IntentPatterns {
    temporal: vec![
        Regex::new(r"(?i)\b(when|last|yesterday|today|tomorrow|ago|before|after|since|until|during|recently|lately)\b").unwrap(),
        Regex::new(r"(?i)\b(monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b").unwrap(),
        Regex::new(r"(?i)\b(january|february|march|april|may|june|july|august|september|october|november|december)\b").unwrap(),
        Regex::new(r"\b\d{4}-\d{2}-\d{2}\b").unwrap(),
        Regex::new(r"\b\d{1,2}[/-]\d{1,2}[/-]\d{2,4}\b").unwrap(),
        Regex::new(r"(?i)\b(this|next|last)\s+(week|month|year)\b").unwrap(),
        Regex::new(r"(?i)\b\d+\s+(day|week|month|year|hour|minute)s?\s+(ago|later|earlier)\b").unwrap(),
    ],
    factual: vec![
        Regex::new(r"(?i)\bwhat\s+is\b").unwrap(),
        Regex::new(r"(?i)\bwho\s+is\b").unwrap(),
        Regex::new(r"(?i)\bwhere\s+is\b").unwrap(),
        Regex::new(r"(?i)\b(definition|define|explain|meaning)\b").unwrap(),
        Regex::new(r"(?i)\bhow\s+(many|much|long|far)\b").unwrap(),
    ],
    entity: vec![
        Regex::new(r"(?i)\b(tell\s+me\s+about|what\s+do\s+you\s+know\s+about)\b").unwrap(),
        Regex::new(r"(?i)\b(who\s+is|what\s+does)\s+[a-z]+\b").unwrap(),
        Regex::new(r"(?i)\b(about|regarding|concerning)\s+[a-z]+\b").unwrap(),
    ],
    preference: vec![
        Regex::new(r"(?i)\b(prefer|like|dislike|want|hate|love|enjoy|favorite|best|worst)\b").unwrap(),
        Regex::new(r"(?i)\b(should\s+i|would\s+you|do\s+you\s+recommend)\b").unwrap(),
        Regex::new(r"(?i)\b(choose|pick|select|option|choice|decide)\b").unwrap(),
    ],
    procedural: vec![
        Regex::new(r"(?i)\bhow\s+(to|do|can|should|would)\b").unwrap(),
        Regex::new(r"(?i)\b(step|process|procedure|workflow|guide|tutorial)\b").unwrap(),
        Regex::new(r"(?i)\b(setup|install|configure|build|deploy|run|execute|start|stop)\b").unwrap(),
    ],
}
});

/// Classify a query's intent.
///
/// Ported from omp `classifyIntent`. Tests each category's patterns against
/// the query and picks the category with the most matches.
pub fn classify_intent(query: &str) -> QueryIntent {
    let query_lower = query.to_lowercase();
    let mut best_category = IntentCategory::General;
    let mut best_score = 0.0f32;
    let mut signals = Vec::new();

    let categories: [(IntentCategory, &[Regex]); 5] = [
        (IntentCategory::Temporal, &PATTERNS.temporal),
        (IntentCategory::Factual, &PATTERNS.factual),
        (IntentCategory::Entity, &PATTERNS.entity),
        (IntentCategory::Preference, &PATTERNS.preference),
        (IntentCategory::Procedural, &PATTERNS.procedural),
    ];

    for (category, patterns) in &categories {
        let mut matches = 0;
        for pattern in *patterns {
            if pattern.is_match(&query_lower) {
                matches += 1;
                signals.push(*category);
            }
        }
        if matches > 0 {
            let score = (0.3 + matches as f32 * 0.15).min(1.0);
            if score > best_score {
                best_score = score;
                best_category = *category;
            }
        }
    }

    let (vec_bias, fts_bias, importance_bias) = intent_weights(best_category);
    QueryIntent {
        category: best_category,
        confidence: best_score,
        signals,
        vec_bias,
        fts_bias,
        importance_bias,
    }
}

/// Adjust recall weights based on classified intent.
///
/// Ported from omp `adjustWeights`. Multiplies base weights by the intent's
/// bias factors.
pub fn adjust_weights(
    intent: &QueryIntent,
    vec_weight: f32,
    fts_weight: f32,
    importance_weight: f32,
) -> (f32, f32, f32) {
    (
        vec_weight * intent.vec_bias,
        fts_weight * intent.fts_bias,
        importance_weight * intent.importance_bias,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_temporal() {
        let intent = classify_intent("when was the last deployment?");
        assert_eq!(intent.category, IntentCategory::Temporal);
        assert!(intent.confidence > 0.0);
    }

    #[test]
    fn classify_factual() {
        let intent = classify_intent("what is the meaning of life");
        assert_eq!(intent.category, IntentCategory::Factual);
    }

    #[test]
    fn classify_procedural() {
        let intent = classify_intent("how to install the package");
        assert_eq!(intent.category, IntentCategory::Procedural);
    }

    #[test]
    fn classify_general() {
        let intent = classify_intent("hello world");
        assert_eq!(intent.category, IntentCategory::General);
        assert_eq!(intent.vec_bias, 1.0);
    }
}
