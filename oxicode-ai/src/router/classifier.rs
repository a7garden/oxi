//! Classifiers for routing decisions.
//!
//! Two-stage pipeline:
//! 1. **Heuristic** — fast, no LLM call, based on **language-agnostic structural signals**.
//! 2. **LLM** (optional) — refines ambiguous cases by asking a small model.
//!
//! The heuristic stage uses only quantitative/structural signals:
//! message length, line count, code blocks, file paths, symbol density,
//! question form, context tokens, turn count. No keyword matching.
//!
//! The LLM stage is only activated when `classifier_model` is configured
//! and the heuristic score falls in the ambiguous zone (0.25–0.75).

use anyhow::Result;

// ── Classifier Input ─────────────────────────────────────────────────────────

/// Input for the classifier — metadata about the current request.
#[derive(Debug, Clone)]
pub struct ClassifierInput {
    /// User message text.
    pub message: String,
    /// Estimated context token count.
    pub context_tokens: usize,
    /// Number of conversation turns so far.
    pub turn_count: usize,
    /// Tool names available in this session.
    pub available_tools: Vec<String>,
}

impl ClassifierInput {
    /// Check if the message contains code blocks (``` markers).
    pub fn contains_code_blocks(&self) -> bool {
        self.message.contains("```")
    }

    /// Check if the message references file paths.
    ///
    /// Detects patterns like `src/main.rs`, `lib/config.ts`, `/etc/hosts`.
    /// Language-agnostic — relies on path structure, not words.
    pub fn contains_file_paths(&self) -> bool {
        let msg = self.message.as_bytes();
        let mut i = 0;
        while i < msg.len() {
            if msg[i] == b'/' || msg[i] == b'\\' {
                for j in (i + 1)..std::cmp::min(i + 20, msg.len()) {
                    if msg[j] == b'.' && j + 1 < msg.len() && msg[j + 1].is_ascii_alphabetic() {
                        return true;
                    }
                }
            }
            i += 1;
        }
        false
    }

    /// Count the number of lines in the message.
    pub fn line_count(&self) -> usize {
        self.message.lines().count().max(1)
    }

    /// Compute symbol density — ratio of code-like characters.
    ///
    /// Counts `{`, `}`, `(`, `)`, `[`, `]`, `<`, `>`, `=`, `;`, `:`,
    /// `|`, `&`, `!`, `@`, `#`, `$`, `%`, `^`, `*`, `+`, `-`, `/`.
    /// Higher density suggests code, configuration, or technical content.
    pub fn symbol_density(&self) -> f64 {
        if self.message.is_empty() {
            return 0.0;
        }
        let code_symbols: &[u8] = b"{}()[]<>=;|&!@#$%^*+-/:\\";
        let count = self
            .message
            .bytes()
            .filter(|b| code_symbols.contains(b))
            .count();
        count as f64 / self.message.len() as f64
    }

    /// Check if the message ends with a question mark.
    pub fn is_question(&self) -> bool {
        self.message.trim().ends_with('?')
    }

    /// Check if the message appears to be a short, single-sentence statement.
    ///
    /// Heuristic: ≤ 3 words and no newlines.
    pub fn is_single_sentence(&self) -> bool {
        let trimmed = self.message.trim();
        !trimmed.contains('\n') && trimmed.split_whitespace().count() <= 3
    }

    /// Count distinct file path references.
    pub fn file_path_count(&self) -> usize {
        let msg = self.message.as_bytes();
        let mut count = 0;
        let mut i = 0;
        while i < msg.len() {
            if msg[i] == b'/' || msg[i] == b'\\' {
                for j in (i + 1)..std::cmp::min(i + 20, msg.len()) {
                    if msg[j] == b'.' && j + 1 < msg.len() && msg[j + 1].is_ascii_alphabetic() {
                        count += 1;
                        // Skip past this path
                        i = j + 1;
                        break;
                    }
                }
            }
            i += 1;
        }
        count
    }
}

// ── Heuristic Classifier ─────────────────────────────────────────────────────

/// Fast classifier based entirely on **language-agnostic structural signals**.
///
/// No keyword matching. Works identically for English, Korean, Japanese,
/// or any other language. Signals used:
///
/// | Signal | What it measures |
/// |--------|-----------------|
/// | Message length | Longer = more complex request |
/// | Line count | Multi-line = structured request |
/// | Code blocks (```) | Code-related task |
/// | File paths | File operation task |
/// | Symbol density | Technical/code content |
/// | Question form (?) | Likely Q&A → lower tier |
/// | Single sentence | Simple → lower tier |
/// | Context tokens | Rich conversation → higher |
/// | Turn count | More turns → richer context |
///
/// Produces a score in `[0.0, 1.0]`:
/// - `≤ 0.3` → simple Q&A, greeting, single-line → `Low` tier
/// - `0.3–0.7` → moderate → `Medium` tier
/// - `≥ 0.7` → complex, multi-file, code-heavy → `High` tier
#[derive(Debug, Clone)]
pub struct HeuristicClassifier {
    /// Context length threshold for "high" (tokens).
    context_threshold_high: usize,
    /// Context length threshold for "low" (tokens).
    context_threshold_low: usize,
}

impl Default for HeuristicClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl HeuristicClassifier {
    /// Create with default thresholds.
    pub fn new() -> Self {
        Self {
            context_threshold_high: 20_000,
            context_threshold_low: 2_000,
        }
    }

    /// Classify the input, returning a complexity score in `[0.0, 1.0]`.
    pub fn classify(&self, input: &ClassifierInput) -> f64 {
        let mut score = 0.0;

        // 1. Message length weight (longer → more complex)
        score += self.length_weight(input.message.len());

        // 2. Line count weight (multi-line → structured request)
        score += self.line_weight(input.line_count());

        // 3. Code block presence
        if input.contains_code_blocks() {
            score += 0.12;
        }

        // 4. File path references
        let path_count = input.file_path_count();
        if path_count > 0 {
            // More files → higher complexity (1 file: 0.08, 2: 0.14, 3+: 0.18)
            score += (0.08 + 0.06 * (path_count - 1).min(2) as f64).min(0.20);
        }

        // 5. Symbol density (high density → technical content)
        score += self.symbol_density_weight(input.symbol_density());

        // 6. Context token weight
        score += self.context_weight(input.context_tokens);

        // 7. Turn count (more turns → richer conversation → slightly higher)
        score += self.turn_weight(input.turn_count);

        // 8. Down-weight simple patterns
        if input.is_single_sentence() {
            score -= 0.08;
        }
        if input.is_question() && input.message.len() < 80 {
            // Short question → likely Q&A, not implementation
            score -= 0.06;
        }

        score.clamp(0.0, 1.0)
    }

    // ── Weight helpers ───────────────────────────────────────────────

    /// Weight from message length (characters).
    fn length_weight(&self, len: usize) -> f64 {
        if len < 20 {
            0.0
        } else if len < 60 {
            0.05
        } else if len < 200 {
            0.12
        } else if len < 600 {
            0.22
        } else if len < 2000 {
            0.32
        } else {
            0.38
        }
    }

    /// Weight from line count.
    fn line_weight(&self, lines: usize) -> f64 {
        if lines <= 1 {
            0.0
        } else if lines <= 3 {
            0.03
        } else if lines <= 10 {
            0.08
        } else {
            0.12
        }
    }

    /// Weight from symbol density.
    ///
    /// Normal prose: ~0.02–0.05, Code: ~0.15–0.30, Config/JSON: ~0.20–0.35
    fn symbol_density_weight(&self, density: f64) -> f64 {
        if density < 0.03 {
            0.0
        } else if density < 0.08 {
            0.02
        } else if density < 0.15 {
            0.06
        } else {
            // High symbol density — code/config heavy
            0.10
        }
    }

    /// Weight from context token count.
    fn context_weight(&self, tokens: usize) -> f64 {
        if tokens < self.context_threshold_low {
            0.0
        } else if tokens < self.context_threshold_high {
            let ratio = (tokens - self.context_threshold_low) as f64
                / (self.context_threshold_high - self.context_threshold_low) as f64;
            0.12 * ratio
        } else {
            0.12
        }
    }

    /// Weight from turn count.
    fn turn_weight(&self, turns: usize) -> f64 {
        if turns < 2 {
            0.0
        } else if turns < 5 {
            0.02
        } else if turns < 10 {
            0.04
        } else {
            0.06
        }
    }
}

// ── LLM Classifier ───────────────────────────────────────────────────────────

/// LLM-based classifier for refining ambiguous heuristic scores.
///
/// When `classifier_model` is configured (e.g. `"anthropic/claude-haiku-4"`),
/// this classifier uses a fast/cheap model to classify ambiguous requests.
/// It only activates when the heuristic score falls in the ambiguous zone (0.25–0.75).
///
/// The LLM understands any language natively — no keyword lists needed.
#[derive(Debug, Clone, Default)]
pub struct LlmClassifier {
    /// The model to use for classification, in `"provider/model-id"` format.
    pub model: Option<String>,
}

impl LlmClassifier {
    /// Create a new LLM classifier.
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }

    /// Classify a user message to determine routing tier.
    ///
    /// Sends a minimal prompt to the configured classifier model and parses
    /// the response as "high", "medium", or "low".
    ///
    /// Returns a score:
    /// - `0.1` for `low`
    /// - `0.5` for `medium`
    /// - `0.9` for `high`
    ///
    /// Falls back to the provided `heuristic_score` on any error.
    pub async fn classify(&self, input: &ClassifierInput, heuristic_score: f64) -> Result<f64> {
        let model_str = self
            .model
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no classifier model configured"))?;

        let (provider_name, model_id) = model_str
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("invalid classifier model format: {model_str}"))?;

        // Resolve provider
        let provider = crate::providers::get_provider_arc(provider_name)
            .ok_or_else(|| anyhow::anyhow!("unknown provider: {provider_name}"))?;

        // Build a minimal model
        let model = crate::types::Model::new(
            model_id,
            model_id,
            crate::Api::AnthropicMessages,
            provider_name,
            "",
        );

        // Build the classifier prompt
        let prompt = build_classifier_prompt(input, heuristic_score);

        // Build a minimal context
        let context = crate::context::Context {
            system_prompt: Some(
                "You are a model router classifier. Reply with exactly one word: high, medium, or low."
                    .to_string(),
            ),
            messages: vec![crate::messages::Message::User(
                crate::messages::UserMessage {
                    role: crate::messages::UserRole::User,
                    content: crate::messages::MessageContent::Text(prompt),
                    timestamp: 0,
                    visible: true,
                },
            )],
            tools: vec![],
        };

        // Stream and collect response
        let stream = provider
            .stream(&model, &context, None)
            .await
            .map_err(|e| anyhow::anyhow!("classifier stream error: {e}"))?;

        let text = collect_stream_text(stream).await?;
        parse_tier_from_response(&text, heuristic_score)
    }
}

/// Build the classifier prompt from input and heuristic score.
fn build_classifier_prompt(input: &ClassifierInput, heuristic_score: f64) -> String {
    let msg_preview = if input.message.len() > 500 {
        format!("{}...", &input.message[..500])
    } else {
        input.message.clone()
    };

    format!(
        "Categorize this request into one tier:\n\
             - high: architecture, design, planning, complex debugging, large refactors\n\
             - medium: implementation, normal coding, multi-file edits\n\
             - low: summaries, formatting, quick questions, simple lookups\n\
             \n\
             Context tokens: {}\n\
             Turn count: {}\n\
             Heuristic score: {heuristic_score:.2}\n\
             \n\
             User request:\n\
             {msg_preview}\n\
             \n\
             Reply with exactly one word: high, medium, or low",
        input.context_tokens, input.turn_count
    )
}

/// Collect all text from a provider event stream.
async fn collect_stream_text(
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = crate::ProviderEvent> + Send>>,
) -> Result<String> {
    use futures::StreamExt;

    let mut text = String::new();
    let mut stream = stream;

    while let Some(event) = stream.next().await {
        match event {
            crate::ProviderEvent::TextDelta { delta, .. } => {
                text.push_str(&delta);
            }
            crate::ProviderEvent::Done { .. } => break,
            crate::ProviderEvent::Error { reason, .. } => {
                anyhow::bail!("classifier stream error: {reason:?}");
            }
            _ => {}
        }
    }

    Ok(text)
}

/// Parse the LLM response text to extract a tier score.
///
/// Looks for "high", "medium", or "low" in the response (case-insensitive).
/// Falls back to the heuristic score if parsing fails.
fn parse_tier_from_response(text: &str, fallback: f64) -> Result<f64> {
    let lower = text.to_lowercase();

    // Check for exact matches first
    if lower.contains("high") {
        return Ok(0.9);
    }
    if lower.contains("medium") {
        return Ok(0.5);
    }
    if lower.contains("low") {
        return Ok(0.1);
    }

    // Fallback to heuristic
    tracing::warn!(
        "LLM classifier returned unparseable response: '{text}', falling back to heuristic score {fallback:.2}"
    );
    Ok(fallback)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(message: &str) -> ClassifierInput {
        ClassifierInput {
            message: message.to_string(),
            context_tokens: 0,
            turn_count: 0,
            available_tools: vec![],
        }
    }

    fn make_input_with_context(message: &str, tokens: usize, turns: usize) -> ClassifierInput {
        ClassifierInput {
            message: message.to_string(),
            context_tokens: tokens,
            turn_count: turns,
            available_tools: vec![],
        }
    }

    // ── Low-tier: short, simple messages ─────────────────────────────

    #[test]
    fn simple_greeting() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input("hello"));
        assert!(score < 0.1, "greeting should score very low, got {score}");
    }

    #[test]
    fn simple_thanks() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input("thank you!"));
        assert!(score < 0.1, "thanks should score very low, got {score}");
    }

    #[test]
    fn korean_greeting() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input("안녕하세요"));
        assert!(
            score < 0.1,
            "korean greeting should score very low, got {score}"
        );
    }

    #[test]
    fn short_question() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input("what is rust?"));
        assert!(score < 0.1, "short question should score low, got {score}");
    }

    #[test]
    fn japanese_short() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input("これ何？"));
        assert!(score < 0.1, "japanese short should score low, got {score}");
    }

    // ── Medium-tier: moderate length, some structure ─────────────────

    #[test]
    fn medium_request() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input(
            "Modify the config file to add the new endpoint for the auth service",
        ));
        assert!(
            (0.05..0.35).contains(&score),
            "medium request should score modest, got {score}"
        );
    }

    #[test]
    fn multi_line_request() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input(
            "I need to update the following:\n- config file\n- router\n- middleware",
        ));
        assert!(
            score > 0.1,
            "multi-line should score higher than single line, got {score}"
        );
    }

    // ── High-tier: long, code, files, technical ──────────────────────

    #[test]
    fn long_request_with_code_blocks() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input(
            "Debug this error:\n```rust\nfn main() { panic!() }\n```\nThe stack trace shows a null pointer.",
        ));
        assert!(
            score >= 0.25,
            "code block request should score medium+, got {score}"
        );
    }

    #[test]
    fn multi_file_request() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input(
            "Update src/main.rs and lib/config.rs to implement the new API",
        ));
        assert!(
            score >= 0.15,
            "multi-file should score medium+, got {score}"
        );
    }

    #[test]
    fn long_technical_request() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input(&format!(
            "I need to implement a distributed event sourcing system with CQRS. \
             The system should support: (1) event store with append-only log, \
             (2) command bus with validation, (3) query side with materialized views, \
             (4) saga orchestration for distributed transactions. \
             Here's my current architecture:\n{}\nPlease review and suggest improvements.",
            "x".repeat(200)
        )));
        assert!(
            score >= 0.2,
            "long technical request should score medium+, got {score}"
        );
    }

    #[test]
    fn high_symbol_density() {
        let classifier = HeuristicClassifier::new();
        let score = classifier.classify(&make_input(
            r#"{"type": "router", "config": {"high": {"model": "opus"}, "low": {"model": "haiku"}}}"#,
        ));
        assert!(
            score >= 0.15,
            "json/config should score higher due to symbol density, got {score}"
        );
    }

    // ── Context and turn influence ───────────────────────────────────

    #[test]
    fn large_context_boosts_score() {
        let classifier = HeuristicClassifier::new();
        let low = classifier.classify(&make_input_with_context("hello", 0, 0));
        let high = classifier.classify(&make_input_with_context("hello", 30_000, 15));
        assert!(
            high > low,
            "large context should boost score: {low} vs {high}"
        );
    }

    #[test]
    fn turn_count_increases_score() {
        let classifier = HeuristicClassifier::new();
        let low = classifier.classify(&make_input_with_context("update this", 0, 0));
        let high = classifier.classify(&make_input_with_context("update this", 10_000, 12));
        assert!(
            high >= low,
            "more turns+context should boost score: {low} vs {high}"
        );
    }

    // ── Structural detection ─────────────────────────────────────────

    #[test]
    fn detect_file_paths() {
        let input = make_input("Look at src/main.rs for the bug");
        assert!(input.contains_file_paths());
        assert_eq!(input.file_path_count(), 1);
    }

    #[test]
    fn detect_multiple_file_paths() {
        let input = make_input("Update src/main.rs and lib/config.rs");
        assert_eq!(input.file_path_count(), 2);
    }

    #[test]
    fn no_file_paths() {
        let input = make_input("What is a closure?");
        assert!(!input.contains_file_paths());
    }

    #[test]
    fn detect_code_blocks() {
        let input = make_input("Here is the code:\n```rust\nfn main() {}\n```");
        assert!(input.contains_code_blocks());
    }

    #[test]
    fn no_code_blocks() {
        let input = make_input("Just a plain message");
        assert!(!input.contains_code_blocks());
    }

    #[test]
    fn detect_question() {
        let input = make_input("what is this?");
        assert!(input.is_question());
    }

    #[test]
    fn detect_single_sentence() {
        let input = make_input("hello world");
        assert!(input.is_single_sentence());
    }

    #[test]
    fn not_single_sentence() {
        let input = make_input("hello\nworld");
        assert!(!input.is_single_sentence());
    }

    #[test]
    fn symbol_density_plain_text() {
        let input = make_input("hello world this is a test");
        assert!(input.symbol_density() < 0.05);
    }

    #[test]
    fn symbol_density_code() {
        let input = make_input("fn main() -> Result<Vec<String>> { Ok(vec![]) }");
        let density = input.symbol_density();
        assert!(
            density > 0.10,
            "code should have high symbol density, got {density}"
        );
    }

    #[test]
    fn line_count_single() {
        let input = make_input("hello");
        assert_eq!(input.line_count(), 1);
    }

    #[test]
    fn line_count_multi() {
        let input = make_input("line1\nline2\nline3");
        assert_eq!(input.line_count(), 3);
    }

    // ── Score bounds ─────────────────────────────────────────────────

    #[test]
    fn score_always_in_bounds() {
        let classifier = HeuristicClassifier::new();
        let inputs = vec![
            make_input(""),
            make_input(&"x".repeat(10000)),
            make_input_with_context("", 100_000, 100),
            make_input("hello thanks 안녕 こんにちは"),
            make_input("```python\nprint('hello')\n```"),
        ];
        for input in &inputs {
            let score = classifier.classify(input);
            assert!((0.0..=1.0).contains(&score), "score out of bounds: {score}");
        }
    }

    // ── Language independence ─────────────────────────────────────────

    #[test]
    fn language_independence_short() {
        let classifier = HeuristicClassifier::new();
        // All short messages should score low regardless of language
        let short_messages = vec!["hello", "안녕", "こんにちは", "你好", "Привет", "مرحبا"];
        for msg in &short_messages {
            let score = classifier.classify(&make_input(msg));
            assert!(
                score < 0.1,
                "short message '{msg}' should score very low, got {score}"
            );
        }
    }

    #[test]
    fn language_independence_long_with_code() {
        let classifier = HeuristicClassifier::new();
        // Long messages with code should score high regardless of surrounding text language
        let messages = vec![
            format!(
                "Refactor this:\n```\nfn main() {{}}\n```\n{}",
                "x".repeat(100)
            ),
            format!(
                "이 코드를 수정해:\n```\nfn main() {{}}\n```\n{}",
                "x".repeat(100)
            ),
            format!(
                "このコードを修正:\n```\nfn main() {{}}\n```\n{}",
                "x".repeat(100)
            ),
        ];
        for msg in &messages {
            let score = classifier.classify(&make_input(msg));
            assert!(
                score > 0.2,
                "long+code message should score medium+, got {score}"
            );
        }
    }

    // ── LLM classifier stub ──────────────────────────────────────────

    #[tokio::test]
    async fn llm_classifier_no_model_configured() {
        let classifier = LlmClassifier::new(None);
        let input = make_input("test");
        let result = classifier.classify(&input, 0.5).await;
        assert!(result.is_err());
    }

    #[test]
    fn llm_classifier_default() {
        let classifier = LlmClassifier::default();
        assert!(classifier.model.is_none());
    }

    // ── parse_tier_from_response ─────────────────────────────────────

    #[test]
    fn parse_high_response() {
        let score = super::parse_tier_from_response("high", 0.5).unwrap();
        assert!((score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn parse_medium_response() {
        let score = super::parse_tier_from_response("medium", 0.5).unwrap();
        assert!((score - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parse_low_response() {
        let score = super::parse_tier_from_response("low", 0.5).unwrap();
        assert!((score - 0.1).abs() < 1e-6);
    }

    #[test]
    fn parse_case_insensitive() {
        let score = super::parse_tier_from_response("HIGH", 0.5).unwrap();
        assert!((score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn parse_with_extra_text() {
        let score = super::parse_tier_from_response("I think this is high tier", 0.5).unwrap();
        assert!((score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn parse_unparseable_falls_back() {
        let score = super::parse_tier_from_response("maybe", 0.42).unwrap();
        assert!((score - 0.42).abs() < 1e-6);
    }

    // ── build_classifier_prompt ──────────────────────────────────────

    #[test]
    fn prompt_contains_user_message() {
        let input = make_input("Debug the authentication module");
        let prompt = super::build_classifier_prompt(&input, 0.5);
        assert!(prompt.contains("Debug the authentication module"));
        assert!(prompt.contains("high"));
        assert!(prompt.contains("medium"));
        assert!(prompt.contains("low"));
    }

    #[test]
    fn prompt_truncates_long_message() {
        let input = make_input(&"x".repeat(600));
        let prompt = super::build_classifier_prompt(&input, 0.5);
        assert!(prompt.contains("..."));
        assert!(prompt.len() < 1000);
    }
}
