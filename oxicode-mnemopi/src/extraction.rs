//! Fact extraction — ported from omp `core/extraction/`.
//!
//! Splits text into atomic facts for memory retention. Two strategies:
//! - [`HeuristicExtractor`] — rule-based sentence splitting (always available).
//! - `LlmExtractor` — LLM-based extraction (host provides the model).
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

/// LLM-backed fact extractor.
///
/// Wraps a [`crate::llm::LlmBackend`] with an extraction prompt template
/// (containing `{text}` and optional `{lang}` placeholders). The model's
/// response is parsed as one fact per line; lines may be marked with a
/// trailing ` | <importance>` to override the default 0.5 importance.
///
/// Failures (network, malformed output) fall back to
/// [`HeuristicExtractor`] so a flaky LLM endpoint cannot disrupt the
/// synchronous `remember` path.
pub struct LlmExtractor {
    backend: std::sync::Arc<dyn crate::llm::LlmBackend>,
    prompt_template: String,
    fallback: HeuristicExtractor,
}

impl std::fmt::Debug for LlmExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmExtractor")
            .field("backend", &self.backend.backend_name())
            .field("prompt_template_len", &self.prompt_template.len())
            .finish()
    }
}

impl LlmExtractor {
    /// Create a new LLM extractor with a custom prompt template.
    ///
    /// The template must contain a `{text}` placeholder; an optional
    /// `{lang}` placeholder receives an ISO-639-1 code (default `"en"`).
    pub fn new(
        backend: std::sync::Arc<dyn crate::llm::LlmBackend>,
        prompt_template: impl Into<String>,
    ) -> Self {
        Self {
            backend,
            prompt_template: prompt_template.into(),
            fallback: HeuristicExtractor,
        }
    }

    /// Default extraction prompt — compact, instructs the model to emit
    /// one atomic fact per line. Designed for small / fast models
    /// (TinyLlama, gpt-4.1-mini, qwen2.5:3b).
    pub fn default_prompt() -> &'static str {
        "Extract atomic facts from the text below. Output one fact per \
         line, no numbering, no bullets, no commentary. If a fact is \
         especially important, append \" | 0.9\" (range 0.0 to 1.0); \
         otherwise omit the suffix. Stop after the facts.\n\n\
         Language: {lang}\n\nText:\n{text}\n"
    }

    /// Build the prompt for a given text.
    fn render_prompt(&self, text: &str) -> String {
        self.prompt_template
            .replace("{lang}", "en")
            .replace("{text}", text)
    }
}

impl FactExtractor for LlmExtractor {
    fn extract(&self, text: &str) -> crate::error::Result<Vec<ExtractedFact>> {
        let prompt = self.render_prompt(text);
        let opts = crate::llm::CompleteOptions {
            max_tokens: Some(1024),
            temperature: Some(0.0),
            timeout: Some(std::time::Duration::from_secs(15)),
        };
        match self.backend.complete(&prompt, &opts) {
            Ok(raw) => Ok(parse_llm_facts(&raw)),
            Err(e) => {
                tracing::warn!(
                    backend = self.backend.backend_name(),
                    error = %e,
                    "LLM fact extraction failed; falling back to heuristic"
                );
                self.fallback.extract(text)
            }
        }
    }
}

/// Parse one fact per non-empty line. Lines may end with ` | <f32>`
/// to override the default importance of 0.5.
fn parse_llm_facts(raw: &str) -> Vec<ExtractedFact> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip bullets / numbering if present.
        let stripped = trimmed
            .trim_start_matches(|c: char| c == '-' || c == '*' || c.is_ascii_digit())
            .trim_start_matches(['.', ')', ' '])
            .trim();
        if stripped.is_empty() {
            continue;
        }
        let (content, importance) = match stripped.rsplit_once('|') {
            Some((head, tail)) => {
                let parsed = tail
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|f| (0.0..=1.0).contains(f));
                match parsed {
                    Some(p) => (head.trim().to_string(), p),
                    None => (stripped.to_string(), 0.5),
                }
            }
            None => (stripped.to_string(), 0.5),
        };
        if content.is_empty() {
            continue;
        }
        let lower = content.to_lowercase();
        out.push(ExtractedFact {
            content,
            importance,
            veracity: detect_veracity(&lower),
            memory_type: Some(detect_type(&lower)),
        });
    }
    out
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

    #[test]
    fn parse_llm_facts_one_per_line() {
        let raw = "Rust is a systems language\nThe build uses cargo\n";
        let facts = parse_llm_facts(raw);
        assert_eq!(facts.len(), 2);
        assert!(facts[0].content.contains("Rust"));
        assert!(facts[1].content.contains("cargo"));
        // Default importance.
        assert!((facts[0].importance - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parse_llm_facts_importance_override() {
        let raw = "Critical fact | 0.95\nNormal fact\nOther | 0.7\n";
        let facts = parse_llm_facts(raw);
        assert_eq!(facts.len(), 3);
        assert!((facts[0].importance - 0.95).abs() < 1e-6);
        assert!((facts[1].importance - 0.5).abs() < 1e-6);
        assert!((facts[2].importance - 0.7).abs() < 1e-6);
    }

    #[test]
    fn parse_llm_facts_skips_empty_and_bullets() {
        let raw = "\n- bullet one\n2. numbered\n* star\n\n";
        let facts = parse_llm_facts(raw);
        assert_eq!(facts.len(), 3);
        assert!(facts.iter().all(|f| !f.content.is_empty()));
    }

    #[test]
    fn parse_llm_facts_rejects_invalid_importance() {
        // " | abc" is not a valid f64 → keep whole line as content.
        let facts = parse_llm_facts("The fact | abc\n");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "The fact | abc");
    }

    #[test]
    fn parse_llm_facts_clamps_importance_range() {
        // Out-of-range importance is rejected, line kept as plain content.
        let facts = parse_llm_facts("Fact | 5.0\n");
        assert_eq!(facts.len(), 1);
        assert!((facts[0].importance - 0.5).abs() < 1e-6);
    }

    #[test]
    fn llm_extractor_falls_back_on_backend_error() {
        use crate::llm::{CompleteOptions, LlmBackend};
        use std::sync::Arc;

        struct FailingBackend;
        impl LlmBackend for FailingBackend {
            fn complete(&self, _: &str, _: &CompleteOptions) -> crate::error::Result<String> {
                Err(crate::error::MnemopiError::Llm("simulated failure".into()))
            }
            fn backend_name(&self) -> &str {
                "failing"
            }
        }

        let extractor = LlmExtractor::new(Arc::new(FailingBackend), LlmExtractor::default_prompt());
        // Extraction should succeed via heuristic fallback, not propagate.
        let facts = extractor
            .extract("The user prefers Vim. This is critically important.")
            .unwrap();
        assert!(!facts.is_empty());
    }

    #[test]
    fn llm_extractor_uses_backend_output_when_available() {
        use crate::llm::StubLlmBackend;
        use std::sync::Arc;

        let stub = StubLlmBackend {
            response: "Fact one\nFact two | 0.9\n".into(),
            name: "stub".into(),
        };
        let extractor = LlmExtractor::new(Arc::new(stub), LlmExtractor::default_prompt());
        let facts = extractor.extract("ignored by stub").unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].content, "Fact one");
        assert!((facts[1].importance - 0.9).abs() < 1e-6);
    }
}
