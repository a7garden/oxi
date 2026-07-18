//! Compaction trait seams — ported from grok-build
//! `xai-grok-compaction/src/{item,sampler,token}.rs` (Apache-2.0).
//!
//! These traits decouple the compaction algorithm from the specific
//! conversation type. Hosts implement [`CompactionItem`] for their
//! message enum and [`CompactionSampler`] for their LLM transport; the
//! shared algorithm operates on the trait, not the concrete type.

use std::pin::Pin;

use crate::{ContentBlock, Message, MessageContent};

/// Harness-agnostic role of a single conversation item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionRole {
    /// System prompt.
    System,
    /// User message.
    User,
    /// Assistant message (may carry tool calls).
    Assistant,
    /// Tool result.
    Tool,
}

/// Contract: one turn/item in a conversation, as seen by the shared
/// compaction algorithms.
///
/// All methods are **required** (no defaults) because a forgotten
/// implementation would silently drop prior summaries on re-compaction.
pub trait CompactionItem {
    /// The harness-agnostic role of this item.
    fn role(&self) -> CompactionRole;
    /// Text content, if any. Tool-only turns may return `None`.
    fn text(&self) -> Option<String>;
    /// Whether this is a tool result message.
    fn is_tool_result(&self) -> bool;
    /// Whether this assistant item has at least one tool call.
    fn has_tool_requests(&self) -> bool;
    /// Whether this item carries a prior compaction summary.
    fn is_compaction_summary(&self) -> bool;
}

/// Interface for the LLM call that produces compaction summaries.
pub trait CompactionSampler: Send + Sync {
    /// Produce a summary for `prompt`.
    fn sample<'a>(
        &'a self,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CompactionSampleError>> + Send + 'a>>;
    /// Human-readable backend name for diagnostics.
    fn name(&self) -> &str;
}

/// Error from a compaction sampling call.
#[derive(Debug)]
pub enum CompactionSampleError {
    /// Nothing to compact (empty input).
    NothingToCompact,
    /// The model returned no usable text.
    EmptyResponse,
    /// A transport error. `deterministic` flags whether retrying could help.
    Transport {
        /// Error message.
        message: String,
        /// Whether re-sending the same input cannot help.
        deterministic: bool,
    },
}

impl std::fmt::Display for CompactionSampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToCompact => write!(f, "nothing to compact"),
            Self::EmptyResponse => {
                write!(f, "compaction model returned an empty summary")
            }
            Self::Transport { message, .. } => {
                write!(f, "compaction sampling failed: {message}")
            }
        }
    }
}

impl std::error::Error for CompactionSampleError {}
/// Token counter for compaction budget calculations.
pub trait ItemTokenCounter: Send + Sync {
    /// Estimate the token count of `text`.
    fn count_tokens(&self, text: &str) -> usize;
}

/// Heuristic token counter: `bytes / 4`.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicTokenCounter;

impl ItemTokenCounter for HeuristicTokenCounter {
    fn count_tokens(&self, text: &str) -> usize {
        (text.len() / 4).max(1)
    }
}

// ── impl CompactionItem for Message ───────────────────────────────

impl CompactionItem for Message {
    fn role(&self) -> CompactionRole {
        match self {
            Message::User(_) => CompactionRole::User,
            Message::Assistant(_) => CompactionRole::Assistant,
            Message::ToolResult(_) => CompactionRole::Tool,
        }
    }

    fn text(&self) -> Option<String> {
        match self {
            Message::User(u) => extract_text_from_content(&u.content),
            Message::Assistant(a) => {
                let texts: Vec<&str> = a.content.iter().filter_map(|b| b.as_text()).collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
            }
            Message::ToolResult(t) => {
                let texts: Vec<&str> = t.content.iter().filter_map(|b| b.as_text()).collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
            }
        }
    }

    fn is_tool_result(&self) -> bool {
        matches!(self, Message::ToolResult(_))
    }

    fn has_tool_requests(&self) -> bool {
        match self {
            Message::Assistant(a) => a
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolCall(_))),
            _ => false,
        }
    }

    fn is_compaction_summary(&self) -> bool {
        match self {
            Message::User(u) => match &u.content {
                MessageContent::Text(s) => s.starts_with("[Branch summary"),
                _ => false,
            },
            _ => false,
        }
    }
}

fn extract_text_from_content(content: &MessageContent) -> Option<String> {
    match content {
        MessageContent::Text(s) => Some(s.clone()),
        MessageContent::Blocks(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter_map(|b| b.as_text()).collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
    }
}

/// Select a split point that keeps tool-request/result pairs together.
/// Returns the index of the first item to **summarise**.
pub fn select_split_point(items: &[Message], keep_recent: usize) -> usize {
    if items.len() <= keep_recent {
        return items.len();
    }
    let candidate = items.len().saturating_sub(keep_recent);
    let mut cut = candidate;
    while cut > 0 {
        let item = &items[cut];
        if item.is_tool_result() {
            cut -= 1;
            continue;
        }
        if cut > 0 {
            let prev = &items[cut - 1];
            if prev.has_tool_requests() {
                cut -= 1;
                continue;
            }
        }
        break;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssistantMessage, Message, UserMessage};

    #[test]
    fn role_classification() {
        assert_eq!(
            Message::User(UserMessage::new("hi")).role(),
            CompactionRole::User
        );
    }

    #[test]
    fn text_extraction_user() {
        let msg = Message::User(UserMessage::new("hello world"));
        assert_eq!(msg.text().as_deref(), Some("hello world"));
    }

    #[test]
    fn is_tool_result_classification() {
        assert!(!Message::User(UserMessage::new("hi")).is_tool_result());
    }

    #[test]
    fn has_tool_requests_false_for_plain_text() {
        let msg = Message::Assistant(AssistantMessage::new(
            crate::Api::OpenAiCompletions,
            "test",
            "test-model",
        ));
        assert!(!msg.has_tool_requests());
    }

    #[test]
    fn is_compaction_summary_detects_branch_marker() {
        let msg = Message::User(UserMessage::new(
            "[Branch summary of 5 msgs] topics: memory",
        ));
        assert!(msg.is_compaction_summary());
    }

    #[test]
    fn is_compaction_summary_false_for_regular_user() {
        assert!(!Message::User(UserMessage::new("just a question")).is_compaction_summary());
    }

    #[test]
    fn heuristic_token_counter_basic() {
        let c = HeuristicTokenCounter;
        assert!(c.count_tokens("hello world") > 0);
    }

    #[test]
    fn select_split_point_keeps_recent() {
        let msgs = vec![
            Message::User(UserMessage::new("a")),
            Message::User(UserMessage::new("b")),
            Message::User(UserMessage::new("c")),
            Message::User(UserMessage::new("d")),
            Message::User(UserMessage::new("e")),
        ];
        let split = select_split_point(&msgs, 2);
        assert_eq!(split, 3);
    }

    #[test]
    fn select_split_point_all_kept_when_under_threshold() {
        let msgs = vec![
            Message::User(UserMessage::new("a")),
            Message::User(UserMessage::new("b")),
        ];
        assert_eq!(select_split_point(&msgs, 5), 2);
    }

    #[test]
    fn sample_error_display() {
        assert_eq!(
            format!("{}", CompactionSampleError::NothingToCompact),
            "nothing to compact"
        );
    }
}
