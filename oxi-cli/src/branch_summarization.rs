//! Branch summarization for tree navigation.
//!
//! When navigating to a different point in the session tree, this generates
//! a summary of the branch being left so context isn't lost.

use crate::session::{SessionEntry, AgentMessage as SessionAgentMessage};
use oxi_ai::{
    AssistantMessage, Model, Provider, UserMessage, ContentBlock, TextContent,
    complete, Context as AiContext,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// Types
// ============================================================================

/// Result of branch summarization
#[derive(Debug, Clone)]
pub struct BranchSummaryResult {
    pub summary: Option<String>,
    pub read_files: Option<Vec<String>>,
    pub modified_files: Option<Vec<String>>,
    pub aborted: bool,
    pub error: Option<String>,
}

impl BranchSummaryResult {
    /// Create a successful result
    pub fn success(summary: String, read_files: Vec<String>, modified_files: Vec<String>) -> Self {
        Self {
            summary: Some(summary),
            read_files: Some(read_files),
            modified_files: Some(modified_files),
            aborted: false,
            error: None,
        }
    }

    /// Create an aborted result
    pub fn aborted() -> Self {
        Self {
            summary: None,
            read_files: None,
            modified_files: None,
            aborted: true,
            error: None,
        }
    }

    /// Create an error result
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            summary: None,
            read_files: None,
            modified_files: None,
            aborted: false,
            error: Some(msg.into()),
        }
    }

    /// Create a no-content result
    pub fn no_content() -> Self {
        Self {
            summary: Some("No content to summarize".to_string()),
            read_files: None,
            modified_files: None,
            aborted: false,
            error: None,
        }
    }
}

/// Details stored for file tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// File operations collected from tool calls
#[derive(Debug, Clone, Default)]
pub struct FileOperations {
    pub read: HashSet<String>,
    pub written: HashSet<String>,
    pub edited: HashSet<String>,
}

impl FileOperations {
    /// Create a new empty FileOperations
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a read file
    pub fn add_read(&mut self, path: impl Into<String>) {
        self.read.insert(path.into());
    }

    /// Add a written file
    pub fn add_written(&mut self, path: impl Into<String>) {
        self.written.insert(path.into());
    }

    /// Add an edited file
    pub fn add_edited(&mut self, path: impl Into<String>) {
        self.edited.insert(path.into());
    }

    /// Compute final file lists from file operations.
    /// Returns read_files (files only read, not modified) and modified_files.
    pub fn compute_file_lists(&self) -> (Vec<String>, Vec<String>) {
        let mut modified: HashSet<String> = self.edited.clone();
        modified.extend(self.written.iter().cloned());
        
        // Collect read files that are not in modified
        let mut read_only: Vec<String> = self.read.iter()
            .filter(|f| !modified.contains(f.as_str()))
            .cloned()
            .collect();
        read_only.sort();
        
        let mut modified_files: Vec<String> = modified.into_iter().collect();
        modified_files.sort();
        
        (read_only, modified_files)
    }
}

/// Details about prepared entries for summarization
#[derive(Debug, Clone)]
pub struct BranchPreparation {
    /// Messages extracted for summarization, in chronological order
    pub messages: Vec<oxi_ai::Message>,
    /// File operations extracted from tool calls
    pub file_ops: FileOperations,
    /// Total estimated tokens in messages
    pub total_tokens: usize,
}

/// Result of collecting entries for branch summary
#[derive(Debug, Clone)]
pub struct CollectEntriesResult {
    /// Entries to summarize, in chronological order
    pub entries: Vec<SessionEntry>,
    /// Common ancestor between old and new position, if any
    pub common_ancestor_id: Option<String>,
}

/// Options for generating branch summary
#[derive(Debug, Clone)]
pub struct GenerateBranchSummaryOptions {
    /// API key for the model
    pub api_key: Option<String>,
    /// Request headers for the model
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// Temperature for summarization (default 0.3)
    pub temperature: Option<f32>,
    /// Optional custom instructions for summarization
    pub custom_instructions: Option<String>,
    /// If true, customInstructions replaces the default prompt instead of being appended
    pub replace_instructions: bool,
    /// Tokens reserved for prompt + LLM response (default 16384)
    pub reserve_tokens: usize,
}

impl Default for GenerateBranchSummaryOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            headers: None,
            temperature: Some(0.3),
            custom_instructions: None,
            replace_instructions: false,
            reserve_tokens: 16384,
        }
    }
}

/// Branch summary settings
#[derive(Debug, Clone)]
pub struct BranchSummarySettings {
    /// Reserve tokens for prompt + response (default 16384)
    pub reserve_tokens: usize,
    /// Maximum tokens for summary response (default 2048)
    pub max_summary_tokens: usize,
    /// Temperature for summarization (default 0.3)
    pub temperature: f32,
}

impl Default for BranchSummarySettings {
    fn default() -> Self {
        Self {
            reserve_tokens: 16384,
            max_summary_tokens: 2048,
            temperature: 0.3,
        }
    }
}

// ============================================================================
// Constants
// ============================================================================

/// Preamble added to branch summaries
const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.
Summary of that exploration:

";

/// Prompt for branch summarization
const BRANCH_SUMMARY_PROMPT: &str = r#"Create a structured summary of this conversation branch for context when returning later.

Use this EXACT format:

## Goal
[What was the user trying to accomplish in this branch?]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Work that was started but not finished]

### Blocked
[Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next to continue this work]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

/// System prompt for summarization
const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// Maximum characters for a tool result in serialized summaries
const TOOL_RESULT_MAX_CHARS: usize = 2000;

// ============================================================================
// Entry Collection
// ============================================================================

/// Collect entries that should be summarized when navigating from one position to another.
///
/// Walks from old_leaf_id back to the common ancestor with target_id, collecting entries
/// along the way. Does NOT stop at compaction boundaries - those are included and their
/// summaries become context.
///
/// # Arguments
/// * `entries` - All entries in the session
/// * `old_leaf_id` - Current position (where we're navigating from)
/// * `target_id` - Target position (where we're navigating to)
///
/// # Returns
/// Entries to summarize and the common ancestor
pub fn collect_entries_for_branch_summary(
    entries: &[SessionEntry],
    old_leaf_id: Option<String>,
    target_id: String,
) -> CollectEntriesResult {
    // If no old position, nothing to summarize
    let old_leaf_id = match old_leaf_id {
        Some(id) => id,
        None => return CollectEntriesResult {
            entries: vec![],
            common_ancestor_id: None,
        },
    };

    // Build path from old leaf to root
    let old_path = build_path_to_root(entries, &old_leaf_id);
    let old_path_set: HashSet<String> = old_path.iter().map(|e| e.id.clone()).collect();

    // Build path from target to root
    let target_path = build_path_to_root(entries, &target_id);

    // target_path is root-first, so iterate backwards to find deepest common ancestor
    let mut common_ancestor_id: Option<String> = None;
    for entry in target_path.iter().rev() {
        if old_path_set.contains(&entry.id.clone()) {
            common_ancestor_id = Some(entry.id.clone());
            break;
        }
    }

    // Collect entries from old leaf back to common ancestor
    let mut entries_to_summarize: Vec<SessionEntry> = Vec::new();
    let mut current_id: Option<String> = Some(old_leaf_id);

    while let Some(ref id) = current_id {
        if common_ancestor_id.as_ref() == Some(id) {
            break;
        }

        if let Some(entry) = entries.iter().find(|e| e.id == *id) {
            entries_to_summarize.push(entry.clone());
            current_id = entry.parent_id.clone();
        } else {
            break;
        }
    }

    // Reverse to get chronological order
    entries_to_summarize.reverse();

    CollectEntriesResult {
        entries: entries_to_summarize,
        common_ancestor_id,
    }
}

/// Build path from an entry to the root (entry, parent, grandparent, etc.)
fn build_path_to_root(entries: &[SessionEntry], start_id: &str) -> Vec<SessionEntry> {
    let mut path = Vec::new();
    let mut current_id: Option<String> = Some(start_id.to_string());

    while let Some(id) = current_id {
        if let Some(entry) = entries.iter().find(|e| e.id == id) {
            path.push(entry.clone());
            current_id = entry.parent_id.clone();
        } else {
            break;
        }
    }

    path
}

// ============================================================================
// Entry to Message Conversion
// ============================================================================

/// Extract LLM message from a session entry.
/// Similar to getMessageFromEntry in compaction.rs but also handles compaction entries.
fn get_message_from_entry(entry: &SessionEntry) -> Option<oxi_ai::Message> {
    match &entry.message {
        crate::session::AgentMessage::User { content } => {
            let text = match content {
                crate::session::ContentValue::String(s) => s.clone(),
                crate::session::ContentValue::Blocks(blocks) => {
                    let mut t = String::new();
                    for block in blocks {
                        if let crate::session::ContentBlock::Text { text } = block {
                            t.push_str(text);
                            t.push('\n');
                        }
                    }
                    t.trim().to_string()
                }
            };
            Some(oxi_ai::Message::User(UserMessage::new(text)))
        }
        crate::session::AgentMessage::Assistant { content, .. } => {
            let mut text = String::new();
            for block in content {
                if let crate::session::AssistantContentBlock::Text { text: t } = block {
                    text.push_str(t);
                    text.push('\n');
                }
            }
            Some(oxi_ai::Message::Assistant({
                let mut msg = AssistantMessage::new(
                    oxi_ai::Api::AnthropicMessages,
                    "session",
                    "unknown",
                );
                msg.content = vec![ContentBlock::Text(TextContent::new(text.trim().to_string()))];
                msg
            }))
        }
        crate::session::AgentMessage::System { content } => {
            let text = match content {
                crate::session::ContentValue::String(s) => s.clone(),
                crate::session::ContentValue::Blocks(blocks) => {
                    let mut t = String::new();
                    for block in blocks {
                        if let crate::session::ContentBlock::Text { text } = block {
                            t.push_str(text);
                            t.push('\n');
                        }
                    }
                    t.trim().to_string()
                }
            };
            // System messages should be converted to user messages for summarization
            Some(oxi_ai::Message::User(UserMessage::new(text)))
        }
        // Other message types don't contribute to context
        _ => None,
    }
}

/// Prepare entries for summarization with token budget.
///
/// Walks entries from NEWEST to OLDEST, adding messages until we hit the token budget.
/// This ensures we keep the most recent context when the branch is too long.
///
/// Also collects file operations from:
/// - Tool calls in assistant messages
/// - Existing branch_summary entries' details (for cumulative tracking)
///
/// # Arguments
/// * `entries` - Entries in chronological order
/// * `token_budget` - Maximum tokens to include (0 = no limit)
pub fn prepare_branch_entries(
    entries: &[SessionEntry],
    token_budget: usize,
) -> BranchPreparation {
    let mut messages: Vec<oxi_ai::Message> = Vec::new();
    let mut file_ops = FileOperations::new();
    let mut total_tokens: usize = 0;

    // First pass: collect file ops from ALL entries (even if they don't fit in token budget)
    // This ensures we capture cumulative file tracking from nested branch summaries
    // Only extract from pi-generated summaries (from_hook !== true), not extension-generated ones
    for _entry in entries {
        // For now, we don't have a from_hook field in our SessionEntry
        // If the entry has details, we would check them here
        // In the TypeScript code, branch_summary entries have this field
    }

    // Second pass: walk from newest to oldest, adding messages until token budget
    for i in (0..entries.len()).rev() {
        let entry = &entries[i];
        let message = match get_message_from_entry(entry) {
            Some(msg) => msg,
            None => continue,
        };

        // Extract file ops from assistant messages (tool calls)
        extract_file_ops_from_message(&message, &mut file_ops);

        let tokens = estimate_tokens_for_message(&message);

        // Check budget before adding
        if token_budget > 0 && total_tokens + tokens > token_budget {
            // If this is a summary entry (compaction or branch_summary), try to fit it anyway
            // Note: In the TypeScript code, it checks entry.type === "compaction" || "branch_summary"
            // We don't have that distinction in our SessionEntry, so we skip this optimization
            // Stop - we've hit the budget
            break;
        }

        messages.insert(0, message);
        total_tokens += tokens;
    }

    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

/// Estimate tokens for a message (rough approximation)
fn estimate_tokens_for_message(message: &oxi_ai::Message) -> usize {
    // Rough approximation: chars / 4
    let text = message_text_content(message);
    (text.len() / 4).max(1)
}

/// Get text content from a message
fn message_text_content(message: &oxi_ai::Message) -> String {
    match message {
        oxi_ai::Message::User(u) => match &u.content {
            oxi_ai::MessageContent::Text(s) => s.clone(),
            oxi_ai::MessageContent::Blocks(blocks) => {
                blocks.iter()
                    .filter_map(|b| b.as_text())
                    .collect::<Vec<_>>()
                    .join("")
            }
        },
        oxi_ai::Message::Assistant(a) => a.text_content(),
        oxi_ai::Message::ToolResult(t) => {
            t.text_content().unwrap_or_default()
        }
    }
}

/// Extract file operations from tool calls in an assistant message.
fn extract_file_ops_from_message(message: &oxi_ai::Message, file_ops: &mut FileOperations) {
    let assistant_msg = match message {
        oxi_ai::Message::Assistant(a) => a,
        _ => return,
    };

    for block in &assistant_msg.content {
        let tool_call = match block.as_tool_call() {
            Some(tc) => tc,
            None => continue,
        };

        // Extract path from arguments
        let path = extract_path_arg(&tool_call.arguments);
        let Some(path) = path else {
            continue;
        };

        match tool_call.name.as_str() {
            "read" => {
                file_ops.add_read(path);
            }
            "write" => {
                file_ops.add_written(path);
            }
            "edit" => {
                file_ops.add_edited(path);
            }
            // Also check for common tool aliases
            "file_read" | "read_file" => {
                file_ops.add_read(path);
            }
            "file_write" | "write_file" | "create_file" => {
                file_ops.add_written(path);
            }
            "file_edit" | "edit_file" | "modify_file" => {
                file_ops.add_edited(path);
            }
            _ => {}
        }
    }
}

/// Extract path argument from tool call arguments JSON
fn extract_path_arg(args: &serde_json::Value) -> Option<String> {
    // Check for "path" field
    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        return Some(path.to_string());
    }
    // Check for "file_path" field
    if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
        return Some(path.to_string());
    }
    // Check for "file" field
    if let Some(path) = args.get("file").and_then(|v| v.as_str()) {
        return Some(path.to_string());
    }
    // Check for "filename" field
    if let Some(path) = args.get("filename").and_then(|v| v.as_str()) {
        return Some(path.to_string());
    }
    None
}

// ============================================================================
// Summary Generation
// ============================================================================

/// Generate a summary of abandoned branch entries.
///
/// # Arguments
/// * `entries` - Session entries to summarize (chronological order)
/// * `model` - The model to use for summarization
/// * `provider` - The LLM provider
/// * `options` - Generation options
pub async fn generate_branch_summary(
    entries: &[SessionEntry],
    model: &Model,
    _provider: Arc<dyn Provider>,
    options: GenerateBranchSummaryOptions,
) -> BranchSummaryResult {
    let reserve_tokens = options.reserve_tokens;
    
    // Token budget = context window minus reserved space for prompt + response
    let context_window = model.context_window;
    let token_budget = context_window.saturating_sub(reserve_tokens);

    let BranchPreparation { messages, file_ops, .. } = prepare_branch_entries(entries, token_budget);

    if messages.is_empty() {
        return BranchSummaryResult::no_content();
    }

    // Serialize messages to text for summarization
    // This prevents the model from treating it as a conversation to continue
    let conversation_text = serialize_conversation(&messages);

    // Build prompt
    let instructions = if options.replace_instructions && options.custom_instructions.is_some() {
        options.custom_instructions.as_ref().unwrap().clone()
    } else if let Some(ref custom) = options.custom_instructions {
        format!("{}\n\nAdditional focus: {}", BRANCH_SUMMARY_PROMPT, custom)
    } else {
        BRANCH_SUMMARY_PROMPT.to_string()
    };
    
    let prompt_text = format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        conversation_text,
        instructions
    );

    // Create summarization messages
    let summarization_messages = vec![oxi_ai::Message::User(UserMessage::new(prompt_text))];

    // Build context for LLM
    let mut context = AiContext::new();
    context.set_system_prompt(SUMMARIZATION_SYSTEM_PROMPT);
    for msg in &summarization_messages {
        context.add_message(msg.clone());
    }

    // Set up options
    let llm_options = oxi_ai::StreamOptions {
        temperature: Some(options.temperature.unwrap_or(0.3) as f64),
        max_tokens: Some(2048),
        api_key: options.api_key.clone(),
        headers: options.headers.clone().unwrap_or_default(),
        ..Default::default()
    };

    // Call LLM for summarization
    let result = match complete(model, &context, Some(llm_options)).await {
        Ok(response) => {
            let text = response.text_content();
            
            // Prepend preamble to provide context about the branch summary
            let summary = format!("{}{}", BRANCH_SUMMARY_PREAMBLE, text);

            // Compute file lists and append to summary
            let (read_files, modified_files) = file_ops.compute_file_lists();
            let final_summary = format_file_operations(&summary, &read_files, &modified_files);

            BranchSummaryResult::success(final_summary, read_files, modified_files)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("abort") || msg.contains("cancelled") {
                BranchSummaryResult::aborted()
            } else {
                BranchSummaryResult::error(msg)
            }
        }
    };

    result
}

/// Serialize LLM messages to text for summarization.
/// This prevents the model from treating it as a conversation to continue.
fn serialize_conversation(messages: &[oxi_ai::Message]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for msg in messages {
        match msg {
            oxi_ai::Message::User(u) => {
                let content = match &u.content {
                    oxi_ai::MessageContent::Text(s) => s.clone(),
                    oxi_ai::MessageContent::Blocks(blocks) => {
                        blocks.iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("")
                    }
                };
                if !content.is_empty() {
                    parts.push(format!("[User]: {}", content));
                }
            }
            oxi_ai::Message::Assistant(a) => {
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
                            // Format tool call as function call
                            let args_str = tc.arguments
                                .as_object()
                                .map(|obj| {
                                    obj.iter()
                                        .map(|(k, v)| format!("{}={}", k, v))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            tool_calls.push(format!("{}({})", tc.name, args_str));
                        }
                        ContentBlock::Image(_) | ContentBlock::Unknown(_) => {
                            // Skip images and unknown blocks
                        }
                    }
                }

                if !thinking_parts.is_empty() {
                    parts.push(format!("[Assistant thinking]: {}", thinking_parts.join("\n")));
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            oxi_ai::Message::ToolResult(t) => {
                let content = t.text_content().unwrap_or_default();
                if !content.is_empty() {
                    let truncated = truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS);
                    parts.push(format!("[Tool result]: {}", truncated));
                }
            }
        }
    }

    parts.join("\n\n")
}

/// Truncate text to a maximum character length for summarization.
/// Keeps the beginning and appends a truncation marker.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let truncated_chars = text.len() - max_chars;
    format!("{}\n\n[... {} more characters truncated]", &text[..max_chars], truncated_chars)
}

/// Format file operations as tags for summary.
fn format_file_operations(summary: &str, read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    
    if !read_files.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read_files.join("\n")));
    }
    if !modified_files.is_empty() {
        sections.push(format!("<modified-files>\n{}\n</modified-files>", modified_files.join("\n")));
    }
    
    if sections.is_empty() {
        return summary.to_string();
    }
    
    format!("{}\n\n{}", summary, sections.join("\n\n"))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(role: &str, content: &str, parent: Option<String>) -> SessionEntry {
        let message = match role {
            "user" => crate::session::AgentMessage::User { 
                content: crate::session::ContentValue::String(content.to_string()) 
            },
            _ => crate::session::AgentMessage::Assistant { 
                content: vec![crate::session::AssistantContentBlock::Text { 
                    text: content.to_string() 
                }],
                provider: None,
                model_id: None,
                usage: None,
                stop_reason: None,
            },
        };
        SessionEntry::new(message)
    }

    #[test]
    fn test_collect_entries_no_old_leaf() {
        let entries = vec![
            create_test_entry("user", "Hello", None),
        ];
        
        let result = collect_entries_for_branch_summary(
            &entries,
            None,
            entries[0].id.clone(),
        );
        
        assert!(result.entries.is_empty());
        assert!(result.common_ancestor_id.is_none());
    }

    #[test]
    fn test_collect_entries_same_path() {
        let mut entries = Vec::new();
        
        // Create a chain: root -> a -> b -> c
        let root = create_test_entry("user", "Root", None);
        let root_id = root.id.clone();
        entries.push(root);
        
        let a = create_test_entry("user", "A", Some(root_id));
        let a_id = a.id.clone();
        entries.push(a);
        
        let b = create_test_entry("user", "B", Some(a_id.clone()));
        let b_id = b.id.clone();
        entries.push(b);
        
        let c = create_test_entry("user", "C", Some(b_id.clone()));
        let c_id = c.id.clone();
        entries.push(c);
        
        // Navigate from c to a (common ancestor should be a)
        let result = collect_entries_for_branch_summary(
            &entries,
            Some(c_id),
            a_id,
        );
        
        // Should collect b and c
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.common_ancestor_id, Some(a_id));
        assert_eq!(result.entries[0].content(), "B");
        assert_eq!(result.entries[1].content(), "C");
    }

    #[test]
    fn test_collect_entries_different_branches() {
        let mut entries = Vec::new();
        
        // Create: root -> a -> b1
        //              -> a -> b2 (different branch)
        let root = create_test_entry("user", "Root", None);
        let root_id = root.id;
        entries.push(root);
        
        let a = create_test_entry("user", "A", Some(root_id));
        let a_id = a.id;
        entries.push(a);
        
        let b1 = create_test_entry("user", "B1", Some(a_id));
        let b1_id = b1.id;
        entries.push(b1);
        
        // Add another branch from a
        let b2 = create_test_entry("user", "B2", Some(a_id));
        let b2_id = b2.id;
        entries.push(b2);
        
        // Navigate from b1 to b2 (common ancestor should be a)
        let result = collect_entries_for_branch_summary(
            &entries,
            Some(b1_id),
            b2_id,
        );
        
        // Should collect b1
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.common_ancestor_id, Some(a_id));
        assert_eq!(result.entries[0].message.content(), "B1");
    }

    #[test]
    fn test_prepare_branch_entries_empty() {
        let entries: Vec<SessionEntry> = vec![];
        let result = prepare_branch_entries(&entries, 1000);
        
        assert!(result.messages.is_empty());
        assert_eq!(result.total_tokens, 0);
    }

    #[test]
    fn test_prepare_branch_entries_token_budget() {
        let mut entries = Vec::new();
        for i in 0..5 {
            let entry = create_test_entry("user", &format!("Message {}", i), None);
            entries.push(entry);
        }
        
        // Budget should allow only some messages
        let result = prepare_branch_entries(&entries, 50);
        
        // Should have fewer messages due to token budget
        assert!(result.messages.len() <= entries.len());
    }

    #[test]
    fn test_file_operations() {
        let mut ops = FileOperations::new();
        ops.add_read("file1.txt");
        ops.add_read("file2.txt");
        ops.add_written("file3.txt");
        ops.add_edited("file4.txt");
        
        let (read_files, modified_files) = ops.compute_file_lists();
        
        assert_eq!(read_files.len(), 1);
        assert!(read_files.contains(&"file1.txt".to_string()));
        assert!(!read_files.contains(&"file3.txt".to_string())); // written files are modified
        
        assert_eq!(modified_files.len(), 2);
        assert!(modified_files.contains(&"file3.txt".to_string()));
        assert!(modified_files.contains(&"file4.txt".to_string()));
    }

    #[test]
    fn test_branch_summary_result() {
        let result = BranchSummaryResult::success(
            "Test summary".to_string(),
            vec!["file1.txt".to_string()],
            vec!["file2.txt".to_string()],
        );
        
        assert!(result.summary.is_some());
        assert_eq!(result.summary.as_ref().unwrap(), "Test summary");
        assert!(!result.aborted);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_branch_summary_result_aborted() {
        let result = BranchSummaryResult::aborted();
        
        assert!(result.summary.is_none());
        assert!(result.aborted);
    }

    #[test]
    fn test_truncate_for_summary() {
        let text = "This is a long text that should be truncated.";
        let truncated = truncate_for_summary(text, 20);
        
        assert!(truncated.contains("truncated"));
        assert!(truncated.len() < text.len());
    }

    #[test]
    fn test_extract_path_arg() {
        let args = serde_json::json!({
            "path": "/tmp/test.txt"
        });
        let path = extract_path_arg(&args);
        assert_eq!(path, Some("/tmp/test.txt".to_string()));
        
        let args2 = serde_json::json!({
            "file_path": "/tmp/other.txt"
        });
        let path2 = extract_path_arg(&args2);
        assert_eq!(path2, Some("/tmp/other.txt".to_string()));
        
        let args3 = serde_json::json!({
            "content": "some content"
        });
        let path3 = extract_path_arg(&args3);
        assert!(path3.is_none());
    }

    #[test]
    fn test_format_file_operations() {
        let summary = "Test summary";
        let read_files = vec!["file1.txt".to_string()];
        let modified_files = vec!["file2.txt".to_string()];
        
        let result = format_file_operations(summary, &read_files, &modified_files);
        
        assert!(result.contains("<read-files>"));
        assert!(result.contains("<modified-files>"));
        assert!(result.contains("file1.txt"));
        assert!(result.contains("file2.txt"));
    }

    #[test]
    fn test_format_file_operations_empty() {
        let summary = "Test summary";
        let result = format_file_operations(summary, &[], &[]);
        
        assert_eq!(result, summary);
    }
}