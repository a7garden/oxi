/// Helper functions for agent loop
use oxi_ai::{ContentBlock, TextContent, ToolCall, ToolResultMessage};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Extract tool calls from an assistant message.
pub fn extract_tool_calls(message: &oxi_ai::AssistantMessage) -> Vec<ToolCall> {
    let mut tool_calls = Vec::new();

    for block in &message.content {
        if let ContentBlock::ToolCall(tc) = block {
            tool_calls.push(tc.clone());
        }
    }

    tool_calls
}

/// Create a tool result message from a finalized tool call.
pub fn create_tool_result_message(finalized: &FinalizedToolCall) -> ToolResultMessage {
    let content_blocks = if let Some(ref blocks) = finalized.result.content_blocks {
        blocks.clone()
    } else {
        vec![ContentBlock::Text(TextContent::new(
            finalized.result.output.clone(),
        ))]
    };

    ToolResultMessage::new(
        finalized.tool_call.id.clone(),
        &finalized.tool_call.name,
        content_blocks,
    )
}

/// Check if a batch of finalized tool calls should terminate the loop.
/// pi-mono: ALL finalized results must have `terminate === true` for the
/// batch to terminate. This is the unanimous consent pattern.
pub fn should_terminate_batch(finalized_calls: &[FinalizedToolCall]) -> bool {
    if finalized_calls.is_empty() {
        return false;
    }
    finalized_calls.iter().all(|f| f.result.terminate)
}

/// Check if the loop should stop after a turn due to external cancellation.
///
/// The loop exits naturally when the LLM stops making tool calls (text-only
/// response). This function only checks for out-of-band cancellation (Ctrl+C).
pub fn should_stop_after_turn(external_stop: &Arc<AtomicBool>) -> bool {
    external_stop.load(Ordering::SeqCst)
}

use crate::AgentToolResult;

/// Finalized tool call with result.
pub struct FinalizedToolCall {
    /// pub.
    pub tool_call: oxi_ai::ToolCall,
    /// pub.
    pub result: AgentToolResult,
    /// pub.
    pub is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_stop_returns_false_when_no_external_stop() {
        let external_stop = Arc::new(AtomicBool::new(false));
        assert!(!should_stop_after_turn(&external_stop));
    }

    #[test]
    fn test_should_stop_returns_true_on_external_stop() {
        let external_stop = Arc::new(AtomicBool::new(true));
        assert!(should_stop_after_turn(&external_stop));
    }

    #[test]
    fn test_sanitize_no_orphans() {
        use oxi_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
        let mut messages = vec![
            Message::User(oxi_ai::UserMessage::new("hello")),
            Message::Assistant({
                let mut m = oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::OpenAiCompletions,
                    "agent",
                    "gpt-4",
                );
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_1",
                    "bash",
                    serde_json::json!({"cmd": "ls"}),
                )));
                m
            }),
            Message::ToolResult(ToolResultMessage::new(
                "call_1",
                "bash",
                vec![ContentBlock::Text(TextContent::new("output"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        assert_eq!(removed, 0);
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn test_sanitize_removes_orphans() {
        use oxi_ai::{ContentBlock, Message, TextContent, ToolResultMessage};
        let mut messages = vec![
            Message::User(oxi_ai::UserMessage::new("hello")),
            // This ToolResult has no preceding Assistant with tool_calls — orphaned.
            Message::ToolResult(ToolResultMessage::new(
                "orphan_1",
                "bash",
                vec![ContentBlock::Text(TextContent::new("orphan output"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        assert_eq!(removed, 1);
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0], Message::User(_)));
    }

    #[test]
    fn test_sanitize_tool_result_after_user_is_orphan() {
        use oxi_ai::{ContentBlock, Message, TextContent, ToolResultMessage};
        // A user message resets the tool_calls context.
        let mut messages = vec![
            Message::User(oxi_ai::UserMessage::new("hello")),
            // No assistant with tool_calls before this — orphaned.
            Message::ToolResult(ToolResultMessage::new(
                "call_x",
                "bash",
                vec![ContentBlock::Text(TextContent::new("result"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        assert_eq!(removed, 1);
    }

    #[test]
    fn test_sanitize_multiple_orphans_removes_only_orphans() {
        use oxi_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
        let mut messages = vec![
            // Orphan 1
            Message::ToolResult(ToolResultMessage::new(
                "orphan_1",
                "bash",
                vec![ContentBlock::Text(TextContent::new("o1"))],
            )),
            // Orphan 2
            Message::ToolResult(ToolResultMessage::new(
                "orphan_2",
                "bash",
                vec![ContentBlock::Text(TextContent::new("o2"))],
            )),
            // Valid pair: assistant with tool_calls + tool result
            Message::Assistant({
                let mut m = oxi_ai::AssistantMessage::new(
                    oxi_ai::Api::OpenAiCompletions,
                    "agent",
                    "gpt-4",
                );
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_1",
                    "read",
                    serde_json::json!({"path": "foo"}),
                )));
                m
            }),
            Message::ToolResult(ToolResultMessage::new(
                "call_1",
                "read",
                vec![ContentBlock::Text(TextContent::new("valid"))],
            )),
            // This one is orphaned — no preceding assistant with tool_calls
            Message::ToolResult(ToolResultMessage::new(
                "orphan_3",
                "write",
                vec![ContentBlock::Text(TextContent::new("o3"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        // Should remove 3 orphans (orphan_1, orphan_2, orphan_3)
        assert_eq!(removed, 3);
        // Only the valid assistant + valid tool result remain
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], Message::Assistant(_)));
        assert!(matches!(messages[1], Message::ToolResult(_)));
    }
}
/// Remove orphaned `ToolResult` messages that lack a preceding
/// `Assistant` message with `tool_calls` content blocks.
///
/// Some providers (e.g. OpenAI) reject messages where a `tool` role message
/// doesn't follow an `assistant` message containing `tool_calls`. This can
/// happen after compaction or state restoration. Orphaned tool results are
/// useless anyway — without the tool_calls they reference, the model has no
/// context for what tool was called.
///
/// Returns the number of orphaned tool results removed.
pub fn sanitize_orphaned_tool_results(messages: &mut Vec<oxi_ai::Message>) -> usize {
    use oxi_ai::Message;

    let mut removed = 0;
    let mut seen_tool_calls = false;

    messages.retain(|msg| {
        match msg {
            Message::Assistant(a) => {
                let has_tool_calls = a.content.iter().any(|b| matches!(b, oxi_ai::ContentBlock::ToolCall(_)));
                seen_tool_calls = has_tool_calls;
                true
            }
            Message::ToolResult(_) => {
                if seen_tool_calls {
                    // This tool result is properly preceded — keep it, but reset
                    // the flag since the result "consumes" the tool_calls context.
                    seen_tool_calls = false;
                    true
                } else {
                    // Orphaned — no preceding tool_calls.
                    removed += 1;
                    false
                }
            }
            // User messages reset the tool_calls context.
            Message::User(_) => {
                seen_tool_calls = false;
                true
            }
        }
    });

    removed
}
