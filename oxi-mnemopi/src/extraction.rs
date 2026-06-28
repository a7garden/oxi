//! Fact extraction — ported from omp `core/extraction/`.
//!
//! Splits text into atomic facts for memory retention. Two strategies:
//! - [`HeuristicExtractor`] — rule-based sentence splitting (always available).
//! - [`LlmExtractor`] — LLM-based extraction (host provides the model).
//!
//! The host application implements [`FactExtractor`] and injects it into
//! the Mnemopi engine. When no extractor is provided, the heuristic
//! fallback is used.

use crate::types::{RememberOptions, Veracity};

/// A single extracted fact.
#[derive(Debug, Clone)]
pub struct ExtractedFact {
    pub content: String,
    pub importance: f64,
    pub veracity: Veracity,
    pub memory_type: Option<String>,
}

/// Fact extraction trait.
///
/// The host application implements this to provide LLM-based extraction.
/// When the LLM is unavailable, fall back to [`HeuristicExtractor`].
pub trait FactExtractor: Send + Sync {
    /// Extract atomic facts from `text`.
    fn extract(&self, text: &str) -> crate::error::Result<Vec<ExtractedFact>>;
}

/// Heuristic fact extractor — always available, no LLM required.
///
/// Splits text into sentences and classifies each as a potential fact
/// based on simple heuristics (presence of verbs, factual keywords, etc.).
#[derive(Debug, Clone, Default)]
pub struct HeuristicExtractor;

impl FactExtractor for HeuristicExtractor {
    fn extract(&self, text: &str) -> crate::error::Result<Vec<ExtractedFact>> {
        Ok(heuristic_extract(text))
    }
}

/// Heuristic fact extraction.
///
/// Splits by sentence boundaries (`. `, `! `, `? `, newlines) and filters
/// out questions, commands, and very short fragments. Assigns importance
/// based on keyword presence.
pub fn heuristic_extract(text: &str) -> Vec<ExtractedFact> {
    let sentences = split_sentences(text);
    let mut facts = Vec::new();

    for sentence in sentences {
        let trimmed = sentence.trim();
        if trimmed.len() < 10 {
            continue;
        }

        let lower = trimmed.to_lowercase();

        // Skip questions (check both trailing ? and question-word starts)
        if trimmed.ends_with('?') || is_question_start(&lower) {
            continue;
        }

        // Skip imperative commands (starts with a verb)
        if is_imperative(&lower) {
            continue;
        }

        // Assign importance based on keywords
        let importance = score_importance(&lower);
        if importance < 0.2 {
            continue;
        }

        // Determine veracity from language
        let veracity = detect_veracity(&lower);

        facts.push(ExtractedFact {
            content: trimmed.to_string(),
            importance,
            veracity,
            memory_type: Some(detect_type(&lower)),
        });
    }

    facts
}

/// Split text into sentences.
fn split_sentences(text: &str) -> Vec<String> {
    text.split(['\n', '.'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .flat_map(|s| {
            // Further split on '! ' and '? '
            s.split(['!', '?'])
                .map(|p| p.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Check if a sentence starts with a question word.
fn is_question_start(lower: &str) -> bool {
    let first_word = lower.split_whitespace().next().unwrap_or("");
    matches!(
        first_word,
        "what"
            | "who"
            | "where"
            | "when"
            | "why"
            | "how"
            | "is"
            | "are"
            | "do"
            | "does"
            | "did"
            | "can"
            | "could"
            | "should"
            | "would"
            | "will"
            | "which"
            | "whose"
    )
}
/// Check if a sentence is an imperative command.
fn is_imperative(lower: &str) -> bool {
    let first_word = lower.split_whitespace().next().unwrap_or("");
    matches!(
        first_word,
        "do" | "don't"
            | "please"
            | "let's"
            | "make"
            | "run"
            | "use"
            | "add"
            | "remove"
            | "create"
            | "delete"
            | "update"
            | "set"
            | "check"
            | "try"
            | "start"
            | "stop"
            | "install"
            | "build"
    )
}

/// Score importance based on keyword presence.
fn score_importance(lower: &str) -> f64 {
    let high_keywords = [
        "important",
        "critical",
        "must",
        "always",
        "never",
        "required",
        "essential",
        "crucial",
        "mandatory",
        "remember",
    ];
    let medium_keywords = [
        "prefer",
        "should",
        "recommend",
        "suggest",
        "better",
        "best",
        "works",
        "using",
        "configured",
        "setup",
        "version",
        "path",
    ];

    if high_keywords.iter().any(|k| lower.contains(k)) {
        return 0.9;
    }
    if medium_keywords.iter().any(|k| lower.contains(k)) {
        return 0.6;
    }
    0.4
}

/// Detect veracity from language cues.
fn detect_veracity(lower: &str) -> Veracity {
    if lower.contains("i think") || lower.contains("probably") || lower.contains("might") {
        Veracity::Inferred
    } else if lower.contains("the user")
        || lower.contains("user prefers")
        || lower.contains("user wants")
    {
        Veracity::Stated
    } else if lower.starts_with("fact:") || lower.starts_with("note:") {
        Veracity::True
    } else {
        Veracity::Unknown
    }
}

/// Detect memory type from content.
fn detect_type(lower: &str) -> String {
    if lower.contains("prefer") || lower.contains("like") || lower.contains("dislike") {
        "preference".to_string()
    } else if lower.contains("how to") || lower.contains("step") || lower.contains("process") {
        "procedural".to_string()
    } else {
        "fact".to_string()
    }
}

/// Convert an extracted fact into RememberOptions.
impl From<&ExtractedFact> for RememberOptions {
    fn from(fact: &ExtractedFact) -> Self {
        Self {
            importance: Some(fact.importance),
            veracity: Some(fact.veracity.clone()),
            memory_type: fact.memory_type.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_basic_facts() {
        let text = "The user prefers dark mode. What time is it? Run the tests. The database is at localhost:5432.";
        let facts = heuristic_extract(text);
        // Should extract: "The user prefers dark mode" and "The database is at localhost:5432"
        // Skips question and imperative
        assert!(facts.len() >= 2);
        assert!(facts.iter().any(|f| f.content.contains("dark mode")));
        assert!(facts.iter().any(|f| f.content.contains("database")));
        assert!(!facts.iter().any(|f| f.content.contains("What time")));
    }

    #[test]
    fn extract_assigns_importance() {
        let facts = heuristic_extract("This is critically important. This is just a note.");
        let important = facts
            .iter()
            .find(|f| f.content.contains("important"))
            .unwrap();
        assert!(important.importance >= 0.9);
    }

    #[test]
    fn extract_detects_preference() {
        let facts = heuristic_extract("The user prefers Vim over Emacs.");
        assert!(
            facts
                .iter()
                .any(|f| f.memory_type.as_deref() == Some("preference"))
        );
    }

    #[test]
    fn heuristic_extractor_via_trait() {
        let extractor = HeuristicExtractor;
        let facts = extractor
            .extract("Rust is a systems language. What about Go?")
            .unwrap();
        assert!(facts.iter().any(|f| f.content.contains("Rust")));
    }
}
