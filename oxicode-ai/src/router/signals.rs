//! Signal extraction for routing decisions.

#![allow(missing_docs)]

//! Four signal types are extracted from the conversation context:
//! - **StructuralSignal**: message count, tool-call density, context size.
//! - **BehavioralSignal**: conversation phase, recent tool-use patterns.
//! - **ContextBudgetSignal**: token-budget pressure and cost constraints.
//! - **VisionSignal**: image content requiring vision-capable models.

use crate::messages::{ContentBlock, Message};

use super::types::{RouterPhase, RouterTier, RoutingDecision};

// ── Structural Signal ─────────────────────────────────────────────────────────

/// Signals derived from the structure of the conversation.
#[derive(Debug, Clone, Default)]
pub struct StructuralSignal {
    pub message_count: usize,
    pub tool_call_count: usize,
    pub tool_result_count: usize,
    pub estimated_tokens: usize,
    pub user_message_count: usize,
}

impl StructuralSignal {
    /// Extract structural signals from the conversation messages.
    pub fn extract(messages: &[Message]) -> Self {
        let mut signal = Self {
            message_count: messages.len(),
            ..Default::default()
        };
        let mut total_chars: usize = 0;

        for msg in messages {
            match msg {
                Message::User(u) => {
                    signal.user_message_count += 1;
                    total_chars += match &u.content {
                        crate::messages::MessageContent::Text(s) => s.len(),
                        crate::messages::MessageContent::Blocks(blocks) => blocks
                            .iter()
                            .map(|b| match b {
                                ContentBlock::Text(t) => t.text.len(),
                                ContentBlock::Image(img) => img.data.len() / 4,
                                _ => 16,
                            })
                            .sum(),
                    };
                }
                Message::Assistant(a) => {
                    for block in &a.content {
                        match block {
                            ContentBlock::Text(t) => total_chars += t.text.len(),
                            ContentBlock::Thinking(t) => total_chars += t.thinking.len(),
                            ContentBlock::ToolCall(_) => signal.tool_call_count += 1,
                            ContentBlock::Image(img) => total_chars += img.data.len() / 4,
                            ContentBlock::Unknown(v) => total_chars += v.to_string().len(),
                        }
                    }
                }
                Message::ToolResult(t) => {
                    signal.tool_result_count += 1;
                    for block in &t.content {
                        if let ContentBlock::Text(txt) = block {
                            total_chars += txt.text.len();
                        }
                    }
                }
            }
        }

        signal.estimated_tokens = total_chars / 4;
        signal
    }

    /// Normalize to `[0, 1]` for scoring.
    pub fn normalized(&self) -> f64 {
        let msg_factor = (self.message_count as f64).ln_1p() / 10.0_f64.ln_1p();
        let tool_factor = (self.tool_call_count as f64).ln_1p() / 20.0_f64.ln_1p();
        let token_factor = (self.estimated_tokens as f64).ln_1p() / 100_000.0_f64.ln_1p();
        (0.3 * msg_factor + 0.4 * tool_factor + 0.3 * token_factor).clamp(0.0, 1.0)
    }
}

// ── Behavioral Signal ─────────────────────────────────────────────────────────

/// Signals derived from recent conversation behavior.
#[derive(Debug, Clone, Default)]
pub struct BehavioralSignal {
    pub phase: RouterPhase,
    pub recent_tool_count: usize,
    pub phase_transitions: usize,
    pub is_question: bool,
}

impl BehavioralSignal {
    /// Extract behavioral signals from context and decision history.
    pub fn extract(messages: &[Message], history: &[RoutingDecision]) -> Self {
        let phase = Self::detect_phase(messages);
        let recent_tool_count = Self::count_recent_tools(messages, 10);
        let phase_transitions = Self::count_phase_transitions(history);
        let is_question = Self::detect_question(messages);

        Self {
            phase,
            recent_tool_count,
            phase_transitions,
            is_question,
        }
    }

    fn detect_phase(messages: &[Message]) -> RouterPhase {
        let recent = messages.len().saturating_sub(6);
        let recent_msgs = &messages[recent..];

        let tool_calls_in_recent: usize = recent_msgs
            .iter()
            .map(|m| match m {
                Message::Assistant(a) => a
                    .content
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::ToolCall(_)))
                    .count(),
                _ => 0,
            })
            .sum();

        let user_msgs_in_recent: usize = recent_msgs
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .count();

        if tool_calls_in_recent >= 3 {
            RouterPhase::Implementation
        } else if user_msgs_in_recent >= 2 && tool_calls_in_recent == 0 {
            RouterPhase::Planning
        } else {
            RouterPhase::Lightweight
        }
    }

    /// Count tool-result messages in the last `n` messages.
    pub fn count_recent_tools(messages: &[Message], n: usize) -> usize {
        let start = messages.len().saturating_sub(n);
        messages[start..]
            .iter()
            .filter(|m| matches!(m, Message::ToolResult(_)))
            .count()
    }

    /// Count phase changes in the decision history.
    pub fn count_phase_transitions(history: &[RoutingDecision]) -> usize {
        if history.len() < 2 {
            return 0;
        }
        history
            .windows(2)
            .filter(|w| w[0].phase != w[1].phase)
            .count()
    }

    fn detect_question(messages: &[Message]) -> bool {
        messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::User(u) => Some(u.content.as_str().unwrap_or("").to_lowercase()),
                _ => None,
            })
            .map(|text| {
                text.ends_with('?')
                    || text.starts_with("what")
                    || text.starts_with("how")
                    || text.starts_with("why")
                    || text.starts_with("when")
                    || text.starts_with("where")
                    || text.starts_with("who")
                    || text.starts_with("explain")
            })
            .unwrap_or(false)
    }

    /// Normalize to `[0, 1]` for scoring.
    pub fn normalized(&self) -> f64 {
        let phase_weight = self.phase.weight();
        let tool_factor = (self.recent_tool_count as f64 / 10.0).min(1.0);
        (0.5 * phase_weight + 0.3 * tool_factor + 0.2 * self.phase_transitions as f64)
            .clamp(0.0, 1.0)
    }
}

// ── Context / Budget Signal ───────────────────────────────────────────────────

/// Signals derived from token budget and cost constraints.
#[derive(Debug, Clone, Default)]
pub struct ContextBudgetSignal {
    pub estimated_tokens: usize,
    pub accumulated_cost: f64,
    pub budget_limit: Option<f64>,
    pub context_upgrade_threshold: Option<usize>,
}

impl ContextBudgetSignal {
    /// Extract budget signals from token estimate, cost, and config.
    pub fn extract(
        estimated_tokens: usize,
        accumulated_cost: f64,
        budget_limit: Option<f64>,
        context_upgrade_threshold: Option<usize>,
    ) -> Self {
        Self {
            estimated_tokens,
            accumulated_cost,
            budget_limit,
            context_upgrade_threshold,
        }
    }

    /// Returns `true` if context length exceeds the upgrade threshold.
    pub fn should_upgrade_context(&self) -> bool {
        self.context_upgrade_threshold
            .map(|t| self.estimated_tokens > t)
            .unwrap_or(false)
    }

    /// Returns `true` if accumulated cost exceeds budget.
    pub fn is_over_budget(&self) -> bool {
        self.budget_limit
            .map(|l| self.accumulated_cost >= l)
            .unwrap_or(false)
    }

    /// Budget utilization ratio `[0, 1]` (1.0 = at/over limit).
    pub fn budget_utilization(&self) -> f64 {
        self.budget_limit
            .map(|l| (self.accumulated_cost / l).min(1.0))
            .unwrap_or(0.0)
    }

    /// Normalize to `[0, 1]` for scoring (higher = more resource pressure).
    pub fn normalized(&self) -> f64 {
        let token_factor = (self.estimated_tokens as f64 / 200_000.0).min(1.0);
        let budget_factor = self.budget_utilization();
        (0.6 * token_factor + 0.4 * budget_factor).clamp(0.0, 1.0)
    }
}

// ── Vision Signal ──────────────────────────────────────────────────────────────

/// Signal derived from image content in the conversation.
///
/// Detects image blocks in user messages and tool results (e.g. screenshots)
/// to determine whether a vision-capable model is needed.
#[derive(Debug, Clone, Default)]
pub struct VisionSignal {
    /// Number of image blocks found in the recent window.
    pub recent_image_count: usize,
    /// Whether the latest user turn contains an image.
    pub has_image_in_latest_turn: bool,
    /// Tool names that produced images (e.g. "browse", "browse_script").
    pub image_producing_tools: Vec<String>,
}

impl VisionSignal {
    /// Extract vision signal from the last `window` messages.
    pub fn extract(messages: &[Message], window: usize) -> Self {
        let start = messages.len().saturating_sub(window);
        let recent = &messages[start..];

        let mut signal = Self::default();

        // Check if the latest user message has images
        for msg in messages.iter().rev() {
            if let Message::User(u) = msg {
                if let crate::messages::MessageContent::Blocks(blocks) = &u.content {
                    for b in blocks {
                        if let ContentBlock::Image(_) = b {
                            signal.has_image_in_latest_turn = true;
                        }
                    }
                }
                break; // Only check the last user message
            }
        }

        // Count images in recent window and identify source tools
        for msg in recent {
            match msg {
                Message::User(u) => {
                    if let crate::messages::MessageContent::Blocks(blocks) = &u.content {
                        for b in blocks {
                            if let ContentBlock::Image(_) = b {
                                signal.recent_image_count += 1;
                            }
                        }
                    }
                }
                Message::ToolResult(t) => {
                    let has_image = t
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Image(_)));
                    if has_image {
                        signal.recent_image_count += 1;
                        // Track which tool produced the image
                        if !signal.image_producing_tools.contains(&t.tool_name) {
                            signal.image_producing_tools.push(t.tool_name.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        signal
    }

    /// Whether any image content requires a vision-capable model.
    pub fn requires_vision(&self) -> bool {
        self.recent_image_count > 0 || self.has_image_in_latest_turn
    }

    /// Normalize to `[0, 1]` for scoring.
    ///
    /// - 0 images → 0.0 (no effect)
    /// - 1 image → 0.7
    /// - 2+ images → 0.9 → approaching 1.0
    pub fn normalized(&self) -> f64 {
        if self.recent_image_count == 0 && !self.has_image_in_latest_turn {
            return 0.0;
        }
        let count = self
            .recent_image_count
            .max(if self.has_image_in_latest_turn { 1 } else { 0 });
        // Sharper curve: 1→0.7, 2→0.9, 3→0.95, 4+→~1.0
        1.0 - (-0.8 * count as f64).exp()
    }
}

// ── Message Content Signal ────────────────────────────────────────────────────

/// Signal derived from the **structural** properties of the last user message.
///
/// Language-agnostic — measures length, line count, code blocks, file paths,
/// symbol density, etc. No keyword matching.
#[derive(Debug, Clone, Default)]
pub struct MessageContentSignal {
    /// Character count of the last user message.
    pub message_length: usize,
    /// Number of lines.
    pub line_count: usize,
    /// Whether the message contains code fences (``` ```).
    pub has_code_blocks: bool,
    /// Number of distinct file path references detected.
    pub file_path_count: usize,
    /// Ratio of code-like symbols to total characters.
    pub symbol_density: f64,
    /// Whether the message ends with '?'.
    pub is_question: bool,
    /// Whether it's a short single-sentence (≤3 words, no newlines).
    pub is_single_sentence: bool,
}

impl MessageContentSignal {
    /// Extract from the last user message in the conversation.
    pub fn extract(messages: &[Message]) -> Self {
        let last_user_text = messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::User(u) => Some(u.content.as_str().unwrap_or("").to_string()),
                _ => None,
            })
            .unwrap_or_default();
        Self::from_text(&last_user_text)
    }

    /// Analyze a raw text string.
    pub fn from_text(text: &str) -> Self {
        let bytes = text.as_bytes();
        let message_length = text.len();
        let line_count = text.lines().count().max(1);
        let has_code_blocks = text.contains("```");

        // Count file paths: /foo/bar.rs or \foo\bar.rs
        let mut file_path_count = 0usize;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'/' || bytes[i] == b'\\' {
                for j in (i + 1)..std::cmp::min(i + 20, bytes.len()) {
                    if bytes[j] == b'.' && j + 1 < bytes.len() && bytes[j + 1].is_ascii_alphabetic()
                    {
                        file_path_count += 1;
                        i = j + 1;
                        break;
                    }
                }
            }
            i += 1;
        }

        // Symbol density
        let code_symbols: &[u8] = b"{}()[]<>=;|&!@#$%^*+-/:\\";
        let symbol_count = text.bytes().filter(|b| code_symbols.contains(b)).count();
        let symbol_density = if text.is_empty() {
            0.0
        } else {
            symbol_count as f64 / text.len() as f64
        };

        let trimmed = text.trim();
        let is_question = trimmed.ends_with('?');
        let is_single_sentence = !trimmed.contains('\n') && trimmed.split_whitespace().count() <= 3;

        Self {
            message_length,
            line_count,
            has_code_blocks,
            file_path_count,
            symbol_density,
            is_question,
            is_single_sentence,
        }
    }

    /// Normalize to `[0, 1]` for scoring.
    ///
    /// Contributions:
    /// - length: 0–0.25
    /// - line count: 0–0.15
    /// - code blocks: 0 or 0.15
    /// - file paths: 0–0.15
    /// - symbol density: 0–0.15
    /// - down-weight: question/short → −0.08/−0.06
    pub fn normalized(&self) -> f64 {
        let mut score = 0.0;

        // Length
        score += match self.message_length {
            0..=20 => 0.0,
            21..=60 => 0.05,
            61..=200 => 0.10,
            201..=600 => 0.15,
            601..=2000 => 0.20,
            _ => 0.25,
        };

        // Line count
        score += match self.line_count {
            1 => 0.0,
            2..=3 => 0.03,
            4..=10 => 0.08,
            _ => 0.15,
        };

        // Code blocks
        if self.has_code_blocks {
            score += 0.15;
        }

        // File paths
        score += (0.05 * self.file_path_count.min(3) as f64).min(0.15);

        // Symbol density
        score += match self.symbol_density {
            d if d < 0.03 => 0.0,
            d if d < 0.08 => 0.03,
            d if d < 0.15 => 0.08,
            _ => 0.15,
        };

        // Down-weight simple patterns
        if self.is_single_sentence {
            score -= 0.08;
        }
        if self.is_question && self.message_length < 80 {
            score -= 0.06;
        }

        score.clamp(0.0, 1.0)
    }

    /// Quick override: returns `Some(tier)` if the signal is decisive enough
    /// to skip the full scoring pipeline.
    ///
    /// Used by Layer 0 ("확실한가?") — only triggers when the signal is
    /// overwhelmingly simple or complex.
    pub fn decisive_tier(&self) -> Option<RouterTier> {
        // Very short, no structure → definitely low
        if self.message_length < 15 && self.is_single_sentence && !self.has_code_blocks {
            return Some(RouterTier::Low);
        }
        // Long + code blocks + file paths → definitely high
        if self.message_length > 500 && self.has_code_blocks && self.file_path_count >= 2 {
            return Some(RouterTier::High);
        }
        None
    }
}

#[cfg(test)]
mod vision_tests {
    use super::*;
    use crate::messages::{TextContent, ToolResultMessage, UserMessage};

    fn text_user_msg(s: &str) -> Message {
        Message::User(UserMessage {
            role: crate::messages::UserRole::User,
            content: crate::messages::MessageContent::Text(s.to_string()),
            timestamp: 0,
            visible: true,
        })
    }

    fn image_user_msg() -> Message {
        Message::User(UserMessage {
            role: crate::messages::UserRole::User,
            content: crate::messages::MessageContent::Blocks(vec![ContentBlock::Image(
                crate::messages::ImageContent {
                    content_type: crate::messages::ImageContentType::Image,
                    data: "fake".to_string(),
                    mime_type: "image/png".to_string(),
                },
            )]),
            timestamp: 0,
            visible: true,
        })
    }

    fn text_tool_result() -> Message {
        Message::ToolResult(ToolResultMessage {
            role: crate::messages::ToolResultRole::ToolResult,
            tool_call_id: "t1".to_string(),
            tool_name: "bash".to_string(),
            content: vec![ContentBlock::Text(TextContent {
                content_type: crate::messages::TextContentType::Text,
                text: "done".to_string(),
                text_signature: None,
            })],
            details: None,
            is_error: false,
            timestamp: 0,
        })
    }

    fn image_tool_result(tool: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            role: crate::messages::ToolResultRole::ToolResult,
            tool_call_id: "t2".to_string(),
            tool_name: tool.to_string(),
            content: vec![ContentBlock::Image(crate::messages::ImageContent {
                content_type: crate::messages::ImageContentType::Image,
                data: "fake".to_string(),
                mime_type: "image/png".to_string(),
            })],
            details: None,
            is_error: false,
            timestamp: 0,
        })
    }

    #[test]
    fn vision_no_images() {
        let msgs = vec![text_user_msg("hello")];
        let signal = VisionSignal::extract(&msgs, 10);
        assert!(!signal.requires_vision());
        assert_eq!(signal.recent_image_count, 0);
    }

    #[test]
    fn vision_user_image() {
        let msgs = vec![image_user_msg()];
        let signal = VisionSignal::extract(&msgs, 10);
        assert!(signal.requires_vision());
        assert!(signal.has_image_in_latest_turn);
    }

    #[test]
    fn vision_tool_result_image() {
        let msgs = vec![text_user_msg("look"), image_tool_result("browse")];
        let signal = VisionSignal::extract(&msgs, 10);
        assert!(signal.requires_vision());
        assert_eq!(signal.recent_image_count, 1);
        assert!(signal.image_producing_tools.contains(&"browse".to_string()));
    }

    #[test]
    fn vision_browse_screenshot() {
        let msgs = vec![image_tool_result("browse")];
        let signal = VisionSignal::extract(&msgs, 10);
        assert!(signal.requires_vision());
        assert!(signal.image_producing_tools.contains(&"browse".to_string()));
    }

    #[test]
    fn vision_normalized_zero() {
        let signal = VisionSignal::default();
        assert!((signal.normalized() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn vision_normalized_single() {
        let signal = VisionSignal {
            recent_image_count: 1,
            has_image_in_latest_turn: true,
            image_producing_tools: vec![],
        };
        let n = signal.normalized();
        assert!(n > 0.5, "single image normalized = {}", n);
        assert!(n <= 1.0);
    }

    #[test]
    fn vision_normalized_multiple() {
        let signal = VisionSignal {
            recent_image_count: 3,
            has_image_in_latest_turn: true,
            image_producing_tools: vec![],
        };
        let n = signal.normalized();
        assert!(n > 0.9, "3 images normalized = {}", n);
    }

    #[test]
    fn vision_window_respected() {
        let msgs: Vec<Message> = (0..20)
            .flat_map(|_| vec![text_user_msg("hi"), image_tool_result("browse")])
            .collect();
        let signal_full = VisionSignal::extract(&msgs, 100);
        let signal_windowed = VisionSignal::extract(&msgs, 4);
        assert!(signal_full.recent_image_count > signal_windowed.recent_image_count);
    }

    #[test]
    fn vision_text_only_tool_result() {
        let msgs = vec![text_user_msg("run"), text_tool_result()];
        let signal = VisionSignal::extract(&msgs, 10);
        assert!(!signal.requires_vision());
    }
}

#[cfg(test)]
mod message_content_tests {
    use super::*;
    use crate::messages::UserMessage;

    fn text_user_msg(s: &str) -> Message {
        Message::User(UserMessage {
            role: crate::messages::UserRole::User,
            content: crate::messages::MessageContent::Text(s.to_string()),
            timestamp: 0,
            visible: true,
        })
    }

    // ── Low-tier: short, simple messages ─────────────────────────────

    #[test]
    fn msg_empty() {
        let sig = MessageContentSignal::from_text("");
        assert!(sig.normalized() < 0.05);
    }

    #[test]
    fn msg_greeting() {
        let sig = MessageContentSignal::from_text("hello");
        assert!(sig.normalized() < 0.1);
        assert!(sig.is_single_sentence);
    }

    #[test]
    fn msg_korean_greeting() {
        let sig = MessageContentSignal::from_text("안녕하세요");
        assert!(sig.normalized() < 0.1);
    }

    #[test]
    fn msg_short_question() {
        let sig = MessageContentSignal::from_text("what is rust?");
        assert!(sig.normalized() < 0.1);
        assert!(sig.is_question);
    }

    // ── Medium-tier: moderate length ─────────────────────────────────

    #[test]
    fn msg_moderate() {
        let sig = MessageContentSignal::from_text(
            "Modify the config file to add the new endpoint for the auth service",
        );
        assert!((0.02..0.25).contains(&sig.normalized()));
    }

    #[test]
    fn msg_multiline() {
        let sig =
            MessageContentSignal::from_text("I need to update:\n- config\n- router\n- middleware");
        assert!(sig.normalized() > 0.05);
        assert_eq!(sig.line_count, 4);
    }

    // ── High-tier: code, files, technical ─────────────────────────────

    #[test]
    fn msg_code_blocks() {
        let sig = MessageContentSignal::from_text(
            "Debug:\n```rust\nfn main() { panic!() }\n```\nStack trace shows null.",
        );
        assert!(sig.normalized() > 0.2);
        assert!(sig.has_code_blocks);
    }

    #[test]
    fn msg_multi_file() {
        let sig =
            MessageContentSignal::from_text("Update src/main.rs and lib/config.rs for the new API");
        assert!(sig.file_path_count >= 2);
        assert!(sig.normalized() > 0.1);
    }

    #[test]
    fn msg_high_symbol_density() {
        let sig = MessageContentSignal::from_text(
            "{\"type\": \"router\", \"config\": {\"high\": {\"model\": \"opus\"}}}",
        );
        assert!(sig.symbol_density > 0.15);
    }

    // ── Extract from messages ────────────────────────────────────────

    #[test]
    fn extract_from_messages() {
        let msgs = vec![
            text_user_msg("system prompt"),
            text_user_msg("update src/main.rs"),
        ];
        let sig = MessageContentSignal::extract(&msgs);
        assert_eq!(sig.message_length, 18); // "update src/main.rs"
    }

    // ── Decisive tier ────────────────────────────────────────────────

    #[test]
    fn decisive_low() {
        let sig = MessageContentSignal::from_text("hi");
        assert_eq!(sig.decisive_tier(), Some(RouterTier::Low));
    }

    #[test]
    fn decisive_high() {
        let code = "x".repeat(600);
        let text = format!(
            "Refactor this:\n```rust\nfn main() {{}}\n```\n\nIn src/main.rs and lib/core.rs:\n{code}"
        );
        let sig = MessageContentSignal::from_text(&text);
        assert_eq!(sig.decisive_tier(), Some(RouterTier::High));
    }

    #[test]
    fn decisive_none_for_medium() {
        let sig = MessageContentSignal::from_text("Please update the router config");
        assert_eq!(sig.decisive_tier(), None);
    }

    // ── Score bounds ─────────────────────────────────────────────────

    #[test]
    fn normalized_always_in_bounds() {
        let inputs = [
            "",
            "x",
            "hello",
            &"x".repeat(10000),
            "```python\nprint('hello')\n```",
            "안녕하세요 세계",
        ];
        for input in &inputs {
            let sig = MessageContentSignal::from_text(input);
            let n = sig.normalized();
            assert!((0.0..=1.0).contains(&n), "out of bounds for '{input}': {n}");
        }
    }
}
