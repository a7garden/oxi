//! SimpleMessage utilities for formatting and processing messages
//!
//! Provides utilities for formatting messages, truncating long content,
//! estimating token counts, and summarizing conversation history.

/// A simple message type for utilities
#[derive(Debug, Clone)]
pub struct SimpleMessage {
/// pub.
    pub role: String,
/// pub.
    pub content: String,
}

/// Maximum message length before truncation
pub const DEFAULT_MAX_MESSAGE_LENGTH: usize = 10_000;

/// Maximum summary length
pub const DEFAULT_MAX_SUMMARY_LENGTH: usize = 500;

/// Estimated tokens per character (for rough estimation)
pub const ESTIMATED_TOKENS_PER_CHAR: f64 = 0.25;

/// Format a message for display
pub fn format_message(role: &str, content: &str) -> String {
    let role_display = match role {
        "user" => "You",
        "assistant" => "Assistant",
        "system" => "System",
        "tool" => "Tool",
        "tool_result" => "Result",
        "bashExecution" => "Bash",
        "custom" => "Custom",
        "branchSummary" | "compactionSummary" => "Summary",
        _ => role,
    };

    format!("[{}]\n{}", role_display, content)
}

/// Format a message with a prefix
pub fn format_message_with_prefix(role: &str, content: &str, prefix: &str) -> String {
    let role_display = match role {
        "user" => "You",
        "assistant" => "Assistant",
        "system" => "System",
        "tool" => "Tool",
        "tool_result" => "Result",
        _ => role,
    };

    if prefix.is_empty() {
        format!("[{}]\n{}", role_display, content)
    } else {
        format!("[{}] {}\n{}", role_display, prefix, content)
    }
}

/// Truncate a message to a maximum length
///
/// # Arguments
/// * `content` - The content to truncate
/// * `max_length` - Maximum length (defaults to DEFAULT_MAX_MESSAGE_LENGTH)
/// * `suffix` - Suffix to append when truncated (defaults to "... [truncated]")
///
/// # Returns
/// The truncated content
pub fn truncate_message(content: &str, max_length: usize, suffix: &str) -> String {
    if content.len() <= max_length {
        return content.to_string();
    }

    // Find a good break point (end of line, sentence, or just max_length)
    let truncated = &content[..max_length];

    // Try to break at end of line
    if let Some(last_newline) = truncated.rfind('\n') {
        if last_newline > max_length / 2 {
            return format!("{}{}", &content[..last_newline], suffix);
        }
    }

    // Try to break at sentence end
    if let Some(last_period) = truncated.rfind(". ") {
        if last_period > max_length / 2 {
            return format!("{}{}", &content[..last_period + 1], suffix);
        }
    }

    format!("{}{}", truncated, suffix)
}

/// Truncate a message with default suffix
pub fn truncate_message_default(content: &str) -> String {
    truncate_message(
        content,
        DEFAULT_MAX_MESSAGE_LENGTH,
        "\n\n... [message truncated]",
    )
}

/// Estimate token count for a message
pub fn estimate_tokens(content: &str) -> usize {
    // Rough estimation: ~4 characters per token for English
    (content.len() as f64 * ESTIMATED_TOKENS_PER_CHAR) as usize
}

/// Estimate token count for multiple messages
pub fn estimate_messages_tokens(messages: &[SimpleMessage]) -> usize {
    messages.iter().map(|m| estimate_tokens(&m.content)).sum()
}

/// Check if content exceeds token limit
pub fn exceeds_token_limit(content: &str, limit: usize) -> bool {
    estimate_tokens(content) > limit
}

/// Summarize a conversation by extracting key points
///
/// This is a simple extractive summarization that takes the first
/// and last messages plus any tool results.
pub fn summarize_conversation(messages: &[SimpleMessage], max_length: usize) -> String {
    if messages.is_empty() {
        return String::new();
    }

    let mut summary_parts = Vec::new();

    // Add first user message
    if let Some(first) = messages.first() {
        if first.role == "user" {
            let content = truncate_message(&first.content, 200, "...");
            summary_parts.push(format!("Started with: {}", content));
        }
    }

    // Count messages by type
    let mut user_count = 0;
    let mut assistant_count = 0;
    let mut tool_count = 0;

    for msg in messages {
        match msg.role.as_str() {
            "user" => user_count += 1,
            "assistant" => assistant_count += 1,
            "tool" | "tool_result" => tool_count += 1,
            _ => {}
        }
    }

    summary_parts.push(format!(
        "{} user message(s), {} assistant response(s), {} tool use(s)",
        user_count, assistant_count, tool_count
    ));

    // Add last assistant message if present
    if let Some(last) = messages.last() {
        if last.role == "assistant" {
            let content = truncate_message(&last.content, 300, "...");
            summary_parts.push(format!("Last response: {}", content));
        }
    }

    let summary = summary_parts.join("\n");
    truncate_message(&summary, max_length, "...")
}

/// Compact messages for context window efficiency
///
/// Returns a compacted version that keeps the most recent messages
/// and summarizes older ones.
pub fn compact_messages(
    messages: &[SimpleMessage],
    max_messages: usize,
    summary_prefix: &str,
    summary_suffix: &str,
) -> Vec<SimpleMessage> {
    if messages.len() <= max_messages {
        return messages.to_vec();
    }

    let to_keep = max_messages / 2;
    let _to_summarize = messages.len() - to_keep;

    // Keep the first few messages
    let kept: Vec<SimpleMessage> = messages.iter().take(to_keep).cloned().collect();

    // Summarize the rest
    let to_summarize_msgs = &messages[to_keep..messages.len()];
    let summary = summarize_conversation(to_summarize_msgs, 300);

    let mut result = kept;
    result.push(SimpleMessage {
        role: "system".to_string(),
        content: format!("{}{}{}", summary_prefix, summary, summary_suffix),
    });

    // Add the most recent messages
    result.extend_from_slice(&messages[messages.len().saturating_sub(to_keep)..]);

    result
}

/// Format bash execution for display
pub fn format_bash_execution(command: &str, output: &str, exit_code: Option<i32>) -> String {
    let mut result = format!("$ {}\n", command);

    if !output.is_empty() {
        result.push_str(output);
        if !output.ends_with('\n') {
            result.push('\n');
        }
    }

    if let Some(code) = exit_code {
        if code == 0 {
            result.push_str(&format!("[exited with code {}]", code));
        } else {
            result.push_str(&format!("[error: exited with code {}]", code));
        }
    }

    result
}

/// Format a tool result for display
pub fn format_tool_result(tool_name: &str, result: &str) -> String {
    format!("[Tool: {}]\n{}\n", tool_name, result)
}

/// Get a short preview of content
pub fn get_preview(content: &str, max_length: usize) -> String {
    let trimmed = content.trim();
    if trimmed.len() <= max_length {
        return trimmed.to_string();
    }

    let preview = &trimmed[..max_length];
    if let Some(last_newline) = preview.rfind('\n') {
        if last_newline > max_length / 2 {
            return format!("{}...", &trimmed[..last_newline]);
        }
    }

    format!("{}...", preview.trim_end())
}

/// Count messages by role
pub fn count_messages_by_role(
    messages: &[SimpleMessage],
) -> std::collections::HashMap<String, usize> {
    let mut counts = std::collections::HashMap::new();
    for msg in messages {
        *counts.entry(msg.role.clone()).or_insert(0) += 1;
    }
    counts
}

/// Calculate context window usage
pub fn calculate_context_usage(messages: &[SimpleMessage], context_window: usize) -> (usize, f64) {
    let total_tokens = estimate_messages_tokens(messages);
    let usage = (total_tokens as f64 / context_window as f64) * 100.0;
    (total_tokens, usage)
}

/// Format tokens for display
pub fn format_tokens(tokens: usize) -> String {
    if tokens < 1000 {
        format!("{} tokens", tokens)
    } else if tokens < 1_000_000 {
        format!("{:.1}K tokens", tokens as f64 / 1000.0)
    } else {
        format!("{:.1}M tokens", tokens as f64 / 1_000_000.0)
    }
}

/// Check if a message is empty or only whitespace
pub fn is_empty_message(msg: &SimpleMessage) -> bool {
    msg.content.trim().is_empty()
}

/// Filter out empty messages
pub fn filter_empty_messages(messages: &[SimpleMessage]) -> Vec<SimpleMessage> {
    messages
        .iter()
        .filter(|m| !is_empty_message(m))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_message() {
        let formatted = format_message("user", "Hello, world!");
        assert!(formatted.contains("You"));
        assert!(formatted.contains("Hello, world!"));
    }

    #[test]
    fn test_truncate_message_short() {
        let content = "Short message";
        let result = truncate_message(content, 100, "...");
        assert_eq!(result, content);
    }

    #[test]
    fn test_truncate_message_long() {
        let content = "a".repeat(200);
        let result = truncate_message(&content, 100, "...[truncated]");
        assert!(result.ends_with("...[truncated]"));
        assert!(result.len() <= 100 + "...[truncated]".len());
    }

    #[test]
    fn test_truncate_message_at_newline() {
        let content = format!("line1\nline2\nline3\n{}", "a".repeat(200));
        let result = truncate_message(&content, 20, "...");
        assert!(result.contains("line1\nline2\nline3"));
    }

    #[test]
    fn test_estimate_tokens() {
        let content = "Hello, world!";
        let tokens = estimate_tokens(content);
        // Should be roughly 3-4 tokens
        assert!(tokens >= 2 && tokens <= 6);
    }

    #[test]
    fn test_exceeds_token_limit() {
        let content = "a".repeat(1000);
        assert!(exceeds_token_limit(&content, 100));
        assert!(!exceeds_token_limit(&content, 500));
    }

    #[test]
    fn test_summarize_conversation_empty() {
        let messages: Vec<SimpleMessage> = vec![];
        let summary = summarize_conversation(&messages, 100);
        assert!(summary.is_empty());
    }

    #[test]
    fn test_summarize_conversation() {
        let messages = vec![
            SimpleMessage {
                role: "user".to_string(),
                content: "Hello, I need help with Rust".to_string(),
            },
            SimpleMessage {
                role: "assistant".to_string(),
                content: "I'd be happy to help with Rust! What specifically do you need?"
                    .to_string(),
            },
        ];
        let summary = summarize_conversation(&messages, 200);
        assert!(summary.contains("Started with"));
        assert!(summary.contains("1 user message"));
        assert!(summary.contains("1 assistant response"));
    }

    #[test]
    fn test_compact_messages() {
        let messages: Vec<SimpleMessage> = (0..10)
            .map(|i| SimpleMessage {
                role: "user".to_string(),
                content: format!("SimpleMessage {}", i),
            })
            .collect();

        let compacted = compact_messages(&messages, 4, "<summary>", "</summary>");
        // Should have summary and some recent messages
        assert!(compacted.len() < messages.len());
        // Should contain the summary marker
        assert!(compacted.iter().any(|m| m.content.contains("<summary>")));
    }

    #[test]
    fn test_format_bash_execution() {
        let result = format_bash_execution("echo hello", "hello\n", Some(0));
        assert!(result.contains("echo hello"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn test_get_preview() {
        let content = "This is a very long message that should be truncated";
        let preview = get_preview(content, 20);
        assert!(preview.len() <= 23); // 20 + "..."
        assert!(preview.starts_with("This is a very "));
    }

    #[test]
    fn test_count_messages_by_role() {
        let messages = vec![
            SimpleMessage {
                role: "user".to_string(),
                content: "msg1".to_string(),
            },
            SimpleMessage {
                role: "assistant".to_string(),
                content: "msg2".to_string(),
            },
            SimpleMessage {
                role: "user".to_string(),
                content: "msg3".to_string(),
            },
        ];
        let counts = count_messages_by_role(&messages);
        assert_eq!(counts.get("user"), Some(&2));
        assert_eq!(counts.get("assistant"), Some(&1));
    }

    #[test]
    fn test_calculate_context_usage() {
        let messages = vec![SimpleMessage {
            role: "user".to_string(),
            content: "a".repeat(1000),
        }];
        let (tokens, usage) = calculate_context_usage(&messages, 10000);
        assert!(tokens > 0);
        assert!(usage < 100.0);
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500 tokens");
        assert_eq!(format_tokens(1500), "1.5K tokens");
        assert_eq!(format_tokens(1_500_000), "1.5M tokens");
    }

    #[test]
    fn test_is_empty_message() {
        let empty = SimpleMessage {
            role: "user".to_string(),
            content: "   ".to_string(),
        };
        assert!(is_empty_message(&empty));

        let non_empty = SimpleMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        };
        assert!(!is_empty_message(&non_empty));
    }

    #[test]
    fn test_filter_empty_messages() {
        let messages = vec![
            SimpleMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
            SimpleMessage {
                role: "user".to_string(),
                content: "   ".to_string(),
            },
            SimpleMessage {
                role: "assistant".to_string(),
                content: "Hi there".to_string(),
            },
        ];
        let filtered = filter_empty_messages(&messages);
        assert_eq!(filtered.len(), 2);
    }
}
