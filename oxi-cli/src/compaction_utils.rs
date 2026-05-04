//! Shared utilities for compaction and branch summarization.
//!
//! Ported from `pi-mono/packages/coding-agent/src/core/compaction/utils.ts`.
//!
//! Provides:
//! - File operation tracking (`FileOperations`, `extract_file_ops`, `compute_file_lists`)
//! - Message serialization for summarization (`serialize_conversation`)
//! - Context token estimation (`estimate_context_tokens`, `calculate_context_tokens`)
//! - Compaction threshold checking (`should_compact`, `CompactionThresholds`, `ContextUsage`)
//! - Compaction data preparation (`collect_entries_for_compaction`, `prepare_compaction`)

use std::collections::HashSet;

use oxi_ai::estimate as estimate_tokens;
use oxi_ai::{ContentBlock, Message, MessageContent, ToolCall};

use crate::session::{AgentMessage, SessionEntry};

// ============================================================================
// Compaction Thresholds & Context Usage
// ============================================================================

/// Thresholds controlling when compaction is triggered.
#[derive(Debug, Clone)]
pub struct CompactionThresholds {
    /// Maximum tokens the model's context window can hold.
    pub max_context_tokens: usize,
    /// Fraction (0.0–1.0) at which compaction kicks in. e.g. 0.8 = 80%.
    pub compact_threshold_percent: f64,
}

impl CompactionThresholds {
    /// Convenience constructor.
    pub fn new(max_context_tokens: usize, compact_threshold_percent: f64) -> Self {
        Self {
            max_context_tokens,
            compact_threshold_percent,
        }
    }
}

impl Default for CompactionThresholds {
    fn default() -> Self {
        Self {
            max_context_tokens: 128_000,
            compact_threshold_percent: 0.8,
        }
    }
}

/// Snapshot of current context utilisation.
#[derive(Debug, Clone)]
pub struct ContextUsage {
    /// Estimated token count of the current context.
    pub estimated_tokens: usize,
    /// Maximum tokens allowed (from thresholds).
    pub max_tokens: usize,
    /// `estimated_tokens / max_tokens` as a percentage (0–100).
    pub usage_percent: f64,
    /// Whether compaction should be triggered.
    pub should_compact: bool,
}

impl ContextUsage {
    fn new(estimated_tokens: usize, thresholds: &CompactionThresholds) -> Self {
        let max_tokens = thresholds.max_context_tokens;
        let usage_percent = if max_tokens > 0 {
            (estimated_tokens as f64 / max_tokens as f64) * 100.0
        } else {
            0.0
        };
        let should_compact = usage_percent >= (thresholds.compact_threshold_percent * 100.0);
        Self {
            estimated_tokens,
            max_tokens,
            usage_percent,
            should_compact,
        }
    }
}

// ============================================================================
// Token Estimation
// ============================================================================

/// Rough token estimation for a single [`Message`].
///
/// Uses the same heuristic as `oxi_ai::high_level::tokens::estimate` but
/// accounts for the overhead of role tags, tool-call arguments, etc.
pub fn estimate_message_tokens(msg: &Message) -> usize {
    let base = match msg {
        Message::User(u) => match &u.content {
            MessageContent::Text(s) => estimate_tokens(s),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| estimate_content_block_tokens(b))
                .sum(),
        },
        Message::Assistant(a) => a
            .content
            .iter()
            .map(|b| estimate_content_block_tokens(b))
            .sum(),
        Message::ToolResult(t) => t
            .content
            .iter()
            .map(|b| estimate_content_block_tokens(b))
            .sum(),
    };

    // Add a small overhead for role / metadata (≈4 tokens per message)
    base + 4
}

/// Estimate tokens for a slice of messages.
pub fn estimate_context_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Estimate total context tokens from session entries.
///
/// This converts each [`SessionEntry`] to a rough token count based on its
/// text content, using `oxi_ai::high_level::tokens::estimate`.
pub fn calculate_context_tokens(entries: &[SessionEntry]) -> usize {
    entries
        .iter()
        .map(|e| {
            let text = e.message.content();
            estimate_tokens(&text) + 4 // overhead per entry
        })
        .sum()
}

/// Check if compaction is needed based on thresholds.
pub fn should_compact(entries: &[SessionEntry], thresholds: &CompactionThresholds) -> ContextUsage {
    let tokens = calculate_context_tokens(entries);
    ContextUsage::new(tokens, thresholds)
}

/// Overload that works directly with [`Message`] slices.
pub fn should_compact_messages(
    messages: &[Message],
    thresholds: &CompactionThresholds,
) -> ContextUsage {
    let tokens = estimate_context_tokens(messages);
    ContextUsage::new(tokens, thresholds)
}

// ============================================================================
// Compaction Data Preparation
// ============================================================================

/// Entries selected for compaction, split into "to compact" and "to keep".
#[derive(Debug, Clone)]
pub struct CompactionSelection {
    /// Entries that will be summarised / discarded.
    pub to_compact: Vec<SessionEntry>,
    /// Recent entries that are kept verbatim.
    pub to_keep: Vec<SessionEntry>,
}

/// Gather entries for compaction, keeping the last `keep_recent` entries intact.
///
/// Returns `None` when there are not enough entries to justify compaction
/// (i.e. total ≤ `keep_recent`).
pub fn collect_entries_for_compaction(
    entries: &[SessionEntry],
    keep_recent: usize,
) -> Option<CompactionSelection> {
    if entries.len() <= keep_recent {
        return None;
    }
    let split = entries.len() - keep_recent;
    Some(CompactionSelection {
        to_compact: entries[..split].to_vec(),
        to_keep: entries[split..].to_vec(),
    })
}

/// Prepared compaction data ready to be sent to the summariser.
#[derive(Debug, Clone)]
pub struct PreparedCompaction {
    /// Serialised text of the entries to be compacted.
    pub conversation_text: String,
    /// File operations extracted from the compacted range.
    pub file_operations: FileOperations,
    /// Number of entries being compacted.
    pub compacted_count: usize,
    /// Estimated tokens in the compacted range.
    pub estimated_tokens: usize,
}

/// Prepare compaction data from a slice of session entries.
///
/// 1. Serialises the conversation entries to plain text.
/// 2. Extracts file operation tracking info.
/// 3. Returns everything needed to build a summarisation prompt.
pub fn prepare_compaction(entries: &[SessionEntry]) -> PreparedCompaction {
    let compacted_count = entries.len();
    let estimated_tokens = calculate_context_tokens(entries);

    // Build serialised conversation from entries
    let mut file_ops = FileOperations::new();
    let conversation_text = serialize_session_entries(entries, &mut file_ops);

    PreparedCompaction {
        conversation_text,
        file_operations: file_ops,
        compacted_count,
        estimated_tokens,
    }
}

/// Prepare compaction data from LLM [`Message`]s (e.g. when working directly
/// with the AI message history rather than session entries).
pub fn prepare_compaction_messages(messages: &[Message]) -> PreparedCompaction {
    let compacted_count = messages.len();
    let estimated_tokens = estimate_context_tokens(messages);

    let mut file_ops = FileOperations::new();
    let conversation_text = serialize_conversation(messages, &mut file_ops);

    PreparedCompaction {
        conversation_text,
        file_operations: file_ops,
        compacted_count,
        estimated_tokens,
    }
}

// ============================================================================
// File Operation Tracking
// ============================================================================

/// Tracks which files have been read, written, or edited during a conversation.
#[derive(Debug, Clone, Default)]
pub struct FileOperations {
    pub read: HashSet<String>,
    pub written: HashSet<String>,
    pub edited: HashSet<String>,
}

impl FileOperations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge another set of file operations into this one.
    pub fn merge(&mut self, other: &FileOperations) {
        self.read.extend(other.read.iter().cloned());
        self.written.extend(other.written.iter().cloned());
        self.edited.extend(other.edited.iter().cloned());
    }
}

/// Compute final file lists from file operations.
///
/// Returns `(read_only_files, modified_files)` where:
/// - `read_only_files` are files that were read but never modified.
/// - `modified_files` are files that were edited or written.
pub fn compute_file_lists(
    file_ops: &FileOperations,
) -> (Vec<String>, Vec<String>) {
    let modified: HashSet<&String> = file_ops.edited.union(&file_ops.written).collect();
    let mut read_only: Vec<String> = file_ops
        .read
        .iter()
        .filter(|f| !modified.contains(f))
        .cloned()
        .collect();
    let mut modified_files: Vec<String> = modified.into_iter().cloned().collect();
    read_only.sort();
    modified_files.sort();
    (read_only, modified_files)
}

/// Format file operations as XML tags for inclusion in a summary.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}

// ============================================================================
// File Operation Extraction
// ============================================================================

/// Extract file operations from an LLM assistant [`Message`].
///
/// Inspects `toolCall` content blocks for `read`, `write`, and `edit` tool
/// invocations and records the `path` argument.
pub fn extract_file_ops_from_message(msg: &Message, file_ops: &mut FileOperations) {
    let content_blocks = match msg {
        Message::Assistant(a) => &a.content,
        _ => return,
    };

    for block in content_blocks {
        if let ContentBlock::ToolCall(tc) = block {
            extract_file_ops_from_tool_call(tc, file_ops);
        }
    }
}

/// Extract file operations from a session entry's agent message.
///
/// For assistant entries the content is plain text so we parse simple patterns
/// like `read(path)`, `write(path)`, `edit(path)`. For the more accurate
/// extraction from LLM messages, use [`extract_file_ops_from_message`].
pub fn extract_file_ops_from_entry(entry: &SessionEntry, file_ops: &mut FileOperations) {
    // Session entries store plain-text content; we do a simple scan for
    // tool-call-like patterns that appear in the serialised conversation.
    let content = entry.message.content();

    // Simple pattern matching: "read(path/to/file)", "write(...)", "edit(...)"
    for tool_name in &["read", "write", "edit"] {
        let pattern = format!("{}(", tool_name);
        let mut start = 0;
        while let Some(pos) = content[start..].find(&pattern) {
            let abs_pos = start + pos + pattern.len();
            if let Some(end) = content[abs_pos..].find(')') {
                let path = content[abs_pos..abs_pos + end].trim().to_string();
                if !path.is_empty() && !path.contains('\n') {
                    match *tool_name {
                        "read" => {
                            file_ops.read.insert(path);
                        }
                        "write" => {
                            file_ops.written.insert(path);
                        }
                        "edit" => {
                            file_ops.edited.insert(path);
                        }
                        _ => {}
                    }
                }
            }
            start = abs_pos;
        }
    }
}

fn extract_file_ops_from_tool_call(tc: &ToolCall, file_ops: &mut FileOperations) {
    let path = tc
        .arguments
        .get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let path = match path {
        Some(p) => p,
        None => return,
    };

    match tc.name.as_str() {
        "read" | "file_read" => {
            file_ops.read.insert(path);
        }
        "write" | "file_write" => {
            file_ops.written.insert(path);
        }
        "edit" | "file_edit" => {
            file_ops.edited.insert(path);
        }
        _ => {}
    }
}

// ============================================================================
// Message Serialization
// ============================================================================

/// Maximum characters for a tool result in serialized summaries.
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Truncate text for summarisation, keeping the beginning and appending a
/// truncation marker.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let truncated_chars = text.len() - max_chars;
    format!(
        "{}\n\n[... {} more characters truncated]",
        &text[..max_chars],
        truncated_chars
    )
}

/// The system prompt used when asking the LLM to summarise a conversation.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "\
You are a context summarization assistant. Your task is to read a conversation \
between a user and an AI coding assistant, then produce a structured summary \
following the exact format specified.\n\n\
Do NOT continue the conversation. Do NOT respond to any questions in the \
conversation. ONLY output the structured summary.";

/// Serialize LLM [`Message`]s to text suitable for summarisation.
///
/// Tool results are truncated to [`TOOL_RESULT_MAX_CHARS`] to keep the
/// summarisation request within reasonable token budgets.
pub fn serialize_conversation(messages: &[Message], file_ops: &mut FileOperations) -> String {
    let mut parts: Vec<String> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(u) => {
                let content = match &u.content {
                    MessageContent::Text(s) => s.clone(),
                    MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| b.as_text().map(|t| t.to_string()))
                        .collect::<Vec<_>>()
                        .join(""),
                };
                if !content.is_empty() {
                    parts.push(format!("[User]: {}", content));
                }
            }
            Message::Assistant(a) => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<String> = Vec::new();

                for block in &a.content {
                    match block {
                        ContentBlock::Text(t) => {
                            text_parts.push(t.text.clone());
                        }
                        ContentBlock::Thinking(t) => {
                            thinking_parts.push(t.thinking.clone());
                        }
                        ContentBlock::ToolCall(tc) => {
                            extract_file_ops_from_tool_call(tc, file_ops);
                            let args_str = format_tool_call_args(&tc.arguments);
                            tool_calls.push(format!("{}({})", tc.name, args_str));
                        }
                        _ => {}
                    }
                }

                if !thinking_parts.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking_parts.join("\n")
                    ));
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!(
                        "[Assistant tool calls]: {}",
                        tool_calls.join("; ")
                    ));
                }
            }
            Message::ToolResult(t) => {
                let content = t
                    .content
                    .iter()
                    .filter_map(|b| b.as_text().map(|t| t.to_string()))
                    .collect::<Vec<_>>()
                    .join("");
                if !content.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
        }
    }

    parts.join("\n\n")
}

/// Serialize session entries to text for summarisation.
pub fn serialize_session_entries(
    entries: &[SessionEntry],
    file_ops: &mut FileOperations,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    for entry in entries {
        extract_file_ops_from_entry(entry, file_ops);
        let content = entry.message.content();
        if content.is_empty() {
            continue;
        }
        match &entry.message {
            AgentMessage::User { .. } => {
                parts.push(format!("[User]: {}", content));
            }
            AgentMessage::Assistant { .. } => {
                // Check for tool-call-like patterns and separate them
                if content.contains("tool calls]:") || content.contains("read(") || content.contains("edit(") || content.contains("write(") {
                    parts.push(format!("[Assistant tool calls]: {}", content));
                } else {
                    parts.push(format!("[Assistant]: {}", content));
                }
            }
            AgentMessage::System { .. } => {
                parts.push(format!("[System]: {}", content));
            }
            // Handle other message types
            _ => {
                if !content.is_empty() {
                    parts.push(format!("[System]: {}", content));
                }
            }
        }
    }

    parts.join("\n\n")
}

// ============================================================================
// Helpers
// ============================================================================

/// Estimate tokens for a single content block.
fn estimate_content_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text(t) => estimate_tokens(&t.text),
        ContentBlock::Thinking(t) => estimate_tokens(&t.thinking),
        ContentBlock::Image(img) => {
            // Images are typically represented as ~85 tokens for low-res
            // or many more for high-res. We use a conservative estimate.
            let _ = img;
            85
        }
        ContentBlock::ToolCall(tc) => {
            let name_tokens = estimate_tokens(&tc.name);
            let args_tokens = estimate_tokens(&tc.arguments.to_string());
            name_tokens + args_tokens
        }
        ContentBlock::Unknown(v) => estimate_tokens(&v.to_string()),
    }
}

/// Format a JSON object of tool-call arguments as `key=value` pairs.
fn format_tool_call_args(args: &serde_json::Value) -> String {
    match args.as_object() {
        Some(obj) => obj
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(", "),
        None => args.to_string(),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_ai::{Api, AssistantMessage, TextContent};
    use crate::session::AssistantContentBlock;

    fn make_user_message(content: &str) -> Message {
        Message::User(oxi_ai::UserMessage::new(content))
    }

    fn make_assistant_message(content: &str) -> Message {
        Message::Assistant({
            let mut msg = AssistantMessage::new(Api::AnthropicMessages, "test", "test-model");
            msg.content = vec![ContentBlock::Text(TextContent::new(content))];
            msg
        })
    }

    fn make_tool_call_message(name: &str, args: &str) -> Message {
        let args_val: serde_json::Value = serde_json::from_str(args).unwrap();
        Message::Assistant({
            let mut msg = AssistantMessage::new(Api::AnthropicMessages, "test", "test-model");
            msg.content = vec![ContentBlock::ToolCall(ToolCall::new(
                "call_1",
                name,
                args_val,
            ))];
            msg
        })
    }

    // -- ContextUsage / should_compact --

    #[test]
    fn test_context_usage_below_threshold() {
        let thresholds = CompactionThresholds::new(128_000, 0.8);
        let usage = ContextUsage::new(50_000, &thresholds);
        assert!(!usage.should_compact);
        assert!((usage.usage_percent - 39.0625).abs() < 0.01);
    }

    #[test]
    fn test_context_usage_at_threshold() {
        let thresholds = CompactionThresholds::new(100_000, 0.8);
        let usage = ContextUsage::new(80_000, &thresholds);
        assert!(usage.should_compact);
    }

    #[test]
    fn test_context_usage_above_threshold() {
        let thresholds = CompactionThresholds::new(100_000, 0.8);
        let usage = ContextUsage::new(95_000, &thresholds);
        assert!(usage.should_compact);
    }

    #[test]
    fn test_should_compact_with_entries() {
        let entries: Vec<SessionEntry> = (0..10)
            .map(|i| SessionEntry::new(AgentMessage::User {
                content: ("Hello world, this is a test message with some content.".to_string()
                    + &"x".repeat(i * 100)).into(),
            }))
            .collect();

        let thresholds = CompactionThresholds::new(10, 0.5); // very low to force compaction
        let usage = should_compact(&entries, &thresholds);
        assert!(usage.should_compact);
    }

    // -- Token estimation --

    #[test]
    fn test_estimate_message_tokens_user() {
        let msg = make_user_message("Hello world, this is a test message.");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_message_tokens_assistant() {
        let msg = make_assistant_message("This is a response from the assistant.");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_context_tokens_multiple_messages() {
        let messages = vec![
            make_user_message("Hello world"),
            make_assistant_message("Hi there"),
            make_user_message("How are you?"),
        ];
        let tokens = estimate_context_tokens(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_calculate_context_tokens_entries() {
        let entries = vec![
            SessionEntry::new(AgentMessage::User {
                content: "Hello world".into(),
            }),
            SessionEntry::new(AgentMessage::Assistant {
                content: vec![AssistantContentBlock::Text { text: "Hi there".into() }],
                provider: None,
                model_id: None,
                usage: None,
                stop_reason: None,
            }),
        ];
        let tokens = calculate_context_tokens(&entries);
        assert!(tokens > 0);
    }

    // -- collect_entries_for_compaction --

    #[test]
    fn test_collect_entries_too_few() {
        let entries: Vec<SessionEntry> = (0..3)
            .map(|_| SessionEntry::new(AgentMessage::User {
                content: "test".into(),
            }))
            .collect();
        assert!(collect_entries_for_compaction(&entries, 4).is_none());
    }

    #[test]
    fn test_collect_entries_exact() {
        let entries: Vec<SessionEntry> = (0..4)
            .map(|_| SessionEntry::new(AgentMessage::User {
                content: "test".into(),
            }))
            .collect();
        assert!(collect_entries_for_compaction(&entries, 4).is_none());
    }

    #[test]
    fn test_collect_entries_enough() {
        let entries: Vec<SessionEntry> = (0..10)
            .map(|_| SessionEntry::new(AgentMessage::User {
                content: "test".into(),
            }))
            .collect();
        let sel = collect_entries_for_compaction(&entries, 4).unwrap();
        assert_eq!(sel.to_compact.len(), 6);
        assert_eq!(sel.to_keep.len(), 4);
    }

    // -- prepare_compaction --

    #[test]
    fn test_prepare_compaction_basic() {
        let entries = vec![
            SessionEntry::new(AgentMessage::User {
                content: "Hello".into(),
            }),
            SessionEntry::new(AgentMessage::Assistant {
                content: vec![AssistantContentBlock::Text { text: "Hi there".into() }],
                provider: None,
                model_id: None,
                usage: None,
                stop_reason: None,
            }),
        ];
        let prepared = prepare_compaction(&entries);
        assert_eq!(prepared.compacted_count, 2);
        assert!(prepared.estimated_tokens > 0);
        assert!(prepared.conversation_text.contains("[User]: Hello"));
        assert!(prepared.conversation_text.contains("[Assistant]: Hi there"));
    }

    #[test]
    fn test_prepare_compaction_messages() {
        let messages = vec![
            make_user_message("Hello"),
            make_assistant_message("World"),
        ];
        let prepared = prepare_compaction_messages(&messages);
        assert_eq!(prepared.compacted_count, 2);
        assert!(prepared.estimated_tokens > 0);
    }

    // -- FileOperations --

    #[test]
    fn test_file_operations_new() {
        let ops = FileOperations::new();
        assert!(ops.read.is_empty());
        assert!(ops.written.is_empty());
        assert!(ops.edited.is_empty());
    }

    #[test]
    fn test_file_operations_merge() {
        let mut a = FileOperations::new();
        a.read.insert("a.rs".to_string());
        a.written.insert("b.rs".to_string());

        let mut b = FileOperations::new();
        b.read.insert("c.rs".to_string());
        b.edited.insert("a.rs".to_string());

        a.merge(&b);
        assert!(a.read.contains("a.rs"));
        assert!(a.read.contains("c.rs"));
        assert!(a.written.contains("b.rs"));
        assert!(a.edited.contains("a.rs"));
    }

    #[test]
    fn test_compute_file_lists() {
        let mut ops = FileOperations::new();
        ops.read.insert("a.rs".to_string());
        ops.read.insert("b.rs".to_string());
        ops.edited.insert("a.rs".to_string());
        ops.written.insert("c.rs".to_string());

        let (read_only, modified) = compute_file_lists(&ops);
        assert_eq!(read_only, vec!["b.rs"]);
        assert_eq!(modified, vec!["a.rs", "c.rs"]);
    }

    #[test]
    fn test_format_file_operations() {
        let fmt = format_file_operations(&[], &[]);
        assert!(fmt.is_empty());

        let fmt = format_file_operations(
            &["readme.md".to_string()],
            &["main.rs".to_string()],
        );
        assert!(fmt.contains("<read-files>"));
        assert!(fmt.contains("readme.md"));
        assert!(fmt.contains("<modified-files>"));
        assert!(fmt.contains("main.rs"));
    }

    // -- extract_file_ops_from_message --

    #[test]
    fn test_extract_file_ops_from_tool_call_message() {
        let msg = make_tool_call_message(
            "read",
            r#"{"path": "/src/main.rs"}"#,
        );
        let mut ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut ops);
        assert!(ops.read.contains("/src/main.rs"));
    }

    #[test]
    fn test_extract_file_ops_from_edit_message() {
        let msg = make_tool_call_message(
            "edit",
            r#"{"path": "/lib.rs", "oldText": "foo", "newText": "bar"}"#,
        );
        let mut ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut ops);
        assert!(ops.edited.contains("/lib.rs"));
    }

    #[test]
    fn test_extract_file_ops_from_write_message() {
        let msg = make_tool_call_message(
            "write",
            r#"{"path": "/new_file.rs", "content": "fn main() {}"}"#,
        );
        let mut ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut ops);
        assert!(ops.written.contains("/new_file.rs"));
    }

    #[test]
    fn test_extract_file_ops_from_user_message_ignored() {
        let msg = make_user_message("Hello");
        let mut ops = FileOperations::new();
        extract_file_ops_from_message(&msg, &mut ops);
        assert!(ops.read.is_empty());
    }

    // -- Serialization --

    #[test]
    fn test_serialize_conversation_basic() {
        let messages = vec![
            make_user_message("What is Rust?"),
            make_assistant_message("Rust is a systems programming language."),
        ];
        let mut ops = FileOperations::new();
        let text = serialize_conversation(&messages, &mut ops);
        assert!(text.contains("[User]: What is Rust?"));
        assert!(text.contains("[Assistant]: Rust is a systems programming language."));
    }

    #[test]
    fn test_serialize_conversation_tool_result_truncated() {
        let long_content = "x".repeat(5000);
        let msg = Message::ToolResult(oxi_ai::ToolResultMessage::new(
            "call_1",
            "test_tool",
            vec![ContentBlock::Text(TextContent::new(long_content))],
        ));
        let messages = vec![msg];
        let mut ops = FileOperations::new();
        let text = serialize_conversation(&messages, &mut ops);
        assert!(text.contains("[Tool result]:"));
        assert!(text.contains("truncated"));
        // Should be shorter than the raw content
        assert!(text.len() < 5500);
    }

    #[test]
    fn test_serialize_session_entries() {
        let entries = vec![
            SessionEntry::new(AgentMessage::User {
                content: "Hello".into(),
            }),
            SessionEntry::new(AgentMessage::Assistant {
                content: vec![AssistantContentBlock::Text { text: "World".into() }],
                provider: None,
                model_id: None,
                usage: None,
                stop_reason: None,
            }),
            SessionEntry::new(AgentMessage::System {
                content: "System msg".into(),
            }),
        ];
        let mut ops = FileOperations::new();
        let text = serialize_session_entries(&entries, &mut ops);
        assert!(text.contains("[User]: Hello"));
        assert!(text.contains("[Assistant]: World"));
        assert!(text.contains("[System]: System msg"));
    }

    // -- Truncation --

    #[test]
    fn test_truncate_for_summary_short() {
        let text = "Hello";
        let truncated = truncate_for_summary(text, 100);
        assert_eq!(truncated, "Hello");
    }

    #[test]
    fn test_truncate_for_summary_long() {
        let text = "x".repeat(3000);
        let truncated = truncate_for_summary(&text, 2000);
        assert!(truncated.contains("truncated"));
        assert!(truncated.len() < 3000);
    }

    // -- CompactionThresholds default --

    #[test]
    fn test_thresholds_default() {
        let t = CompactionThresholds::default();
        assert_eq!(t.max_context_tokens, 128_000);
        assert!((t.compact_threshold_percent - 0.8).abs() < 0.001);
    }
}
