/// Helper functions for agent loop
use oxicode_ai::{ContentBlock, TextContent, ToolCall, ToolResultMessage};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Extract tool calls from an assistant message.
pub fn extract_tool_calls(message: &oxicode_ai::AssistantMessage) -> Vec<ToolCall> {
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
    pub tool_call: oxicode_ai::ToolCall,
    /// pub.
    pub result: AgentToolResult,
    /// pub.
    pub is_error: bool,
}

/// Remove orphaned `ToolResult` messages and orphaned `ToolCall` blocks
/// from `Assistant` messages.
///
/// Some providers (e.g. OpenAI) reject messages where:
/// 1. A `tool` role message doesn't follow an `assistant` message containing
///    `tool_calls` (orphaned ToolResult).
/// 2. An assistant message contains `tool_calls` that are not followed by
///    the corresponding `ToolResult` messages before the next user or
///    assistant turn (orphaned ToolCall).
///
/// Both cases can happen after compaction, state restoration, or partial
/// tool execution failure. This function restores a valid
/// tool_call/tool_result adjacency that the provider will accept.
///
/// Returns the number of orphaned items removed.
pub fn sanitize_orphaned_tool_results(messages: &mut Vec<oxicode_ai::Message>) -> usize {
    use oxicode_ai::{ContentBlock, Message};
    use std::collections::HashSet;

    if messages.is_empty() {
        return 0;
    }

    // ---- Pass 1: forward scan, collect metadata for assistants and results ----
    // For each assistant message with tool_calls, record the set of
    // tool_call_ids it issued. For each tool result, remember its id.
    //
    // We use a sliding window: a user message (or the start of a new
    // assistant turn) closes the current tool-calling "batch" and starts
    // a fresh one.
    struct AssistantBatch {
        /// Index into `messages` of the assistant message.
        msg_idx: usize,
        /// tool_call_ids issued by this assistant.
        issued: HashSet<String>,
        /// tool_call_ids that have been matched by a ToolResult below.
        matched: HashSet<String>,
    }

    let mut batches: Vec<AssistantBatch> = Vec::new();
    let mut current: Option<AssistantBatch> = None;

    // Track which tool_results are valid (matched to some assistant's id).
    let mut valid_result: Vec<bool> = vec![false; messages.len()];

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            Message::Assistant(a) => {
                // Close any prior batch — a new assistant turn starts a fresh
                // tool-call window even if its tool_calls haven't completed
                // (those become orphans to be stripped).
                if let Some(b) = current.take() {
                    batches.push(b);
                }
                let issued: HashSet<String> = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall(tc) => Some(tc.id.clone()),
                        _ => None,
                    })
                    .collect();
                if !issued.is_empty() {
                    current = Some(AssistantBatch {
                        msg_idx: i,
                        issued,
                        matched: HashSet::new(),
                    });
                }
            }
            Message::ToolResult(t) => {
                if let Some(ref mut b) = current
                    && b.issued.contains(&t.tool_call_id)
                {
                    b.matched.insert(t.tool_call_id.clone());
                    valid_result[i] = true;
                }
                // Else: orphan result (no active batch, or batch doesn't have
                // this id) — marked invalid, will be removed.
            }
            Message::User(_) => {
                if let Some(b) = current.take() {
                    batches.push(b);
                }
            }
        }
    }
    if let Some(b) = current {
        batches.push(b);
    }

    // ---- Pass 2: build the result vec, stripping orphans ----
    let mut removed = 0;
    let mut kept: Vec<Message> = Vec::with_capacity(messages.len());

    // For each batch, compute the set of unmatched tool_call_ids to strip
    // from the corresponding assistant message.
    let mut strip_from_assistant: HashSet<usize> = HashSet::new();
    for b in &batches {
        if b.matched.len() < b.issued.len() {
            // Some tool_calls were not answered. We strip the orphan
            // ToolCall blocks; if that empties the assistant, the whole
            // message is removed.
            strip_from_assistant.insert(b.msg_idx);
        }
    }

    for (i, msg) in messages.drain(..).enumerate() {
        match msg {
            Message::ToolResult(_) => {
                if valid_result[i] {
                    kept.push(msg);
                } else {
                    removed += 1;
                }
            }
            Message::Assistant(mut a) => {
                if strip_from_assistant.contains(&i) {
                    let before = a.content.len();
                    a.content
                        .retain(|b| !matches!(b, ContentBlock::ToolCall(_)));
                    removed += before - a.content.len();
                    if a.content.is_empty() {
                        // Drop the assistant entirely — it has no text and
                        // no tool_calls left, so it would be a no-op.
                        removed += 1;
                    } else {
                        kept.push(Message::Assistant(a));
                    }
                } else {
                    kept.push(Message::Assistant(a));
                }
            }
            other => kept.push(other),
        }
    }

    *messages = kept;
    removed
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
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
        let mut messages = vec![
            Message::User(oxicode_ai::UserMessage::new("hello")),
            Message::Assistant({
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
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
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolResultMessage};
        let mut messages = vec![
            Message::User(oxicode_ai::UserMessage::new("hello")),
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
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolResultMessage};
        // A user message resets the tool_calls context.
        let mut messages = vec![
            Message::User(oxicode_ai::UserMessage::new("hello")),
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
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
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
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
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

    #[test]
    fn test_sanitize_multi_tool_call_assistant_preserves_all_results() {
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
        // Regression test: an assistant with 2+ tool_calls must preserve ALL
        // corresponding ToolResult messages, not just the first one.
        let mut messages = vec![
            Message::User(oxicode_ai::UserMessage::new("do two things")),
            Message::Assistant({
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
                    "agent",
                    "gpt-4",
                );
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_1",
                    "read",
                    serde_json::json!({"path": "a.txt"}),
                )));
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_2",
                    "read",
                    serde_json::json!({"path": "b.txt"}),
                )));
                m
            }),
            Message::ToolResult(ToolResultMessage::new(
                "call_1",
                "read",
                vec![ContentBlock::Text(TextContent::new("aaa"))],
            )),
            Message::ToolResult(ToolResultMessage::new(
                "call_2",
                "read",
                vec![ContentBlock::Text(TextContent::new("bbb"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        assert_eq!(removed, 0, "no tool results should be orphaned");
        assert_eq!(messages.len(), 4, "all 4 messages should be kept");
    }

    #[test]
    fn test_sanitize_orphan_tool_call_stripped_from_assistant() {
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
        // When an assistant's tool_call has no matching result before a
        // new assistant turn, the orphan tool_call block must be stripped
        // (or the whole assistant dropped) so the provider doesn't reject
        // the request.
        let mut messages = vec![
            Message::Assistant({
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
                    "agent",
                    "gpt-4",
                );
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_1",
                    "read",
                    serde_json::json!({"path": "a.txt"}),
                )));
                m
            }),
            // No ToolResult for call_1 — it's an orphan tool_call.
            // A new assistant starts a fresh batch:
            Message::Assistant({
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
                    "agent",
                    "gpt-4",
                );
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_2",
                    "bash",
                    serde_json::json!({"cmd": "ls"}),
                )));
                m
            }),
            Message::ToolResult(ToolResultMessage::new(
                "call_2",
                "bash",
                vec![ContentBlock::Text(TextContent::new("ok"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        // call_1's tool_call is orphan → stripped from the first assistant.
        // The first assistant is now empty (no text, no tool_calls) → dropped.
        // call_2's tool_call + result is valid → kept.
        assert_eq!(
            removed, 2,
            "1 tool_call block stripped + 1 empty assistant dropped"
        );
        assert_eq!(messages.len(), 2);
        // Only the second assistant and its result remain.
        assert!(matches!(messages[0], Message::Assistant(_)));
        assert!(matches!(messages[1], Message::ToolResult(_)));
    }

    #[test]
    fn test_sanitize_assistant_with_text_and_orphan_tool_call_keeps_text() {
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolCall};
        // An assistant that has BOTH text content AND a tool_call whose
        // result is missing should keep its text but lose the tool_call.
        let mut messages = vec![
            Message::Assistant({
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
                    "agent",
                    "gpt-4",
                );
                m.content
                    .push(ContentBlock::Text(TextContent::new("let me check")));
                m.content.push(ContentBlock::ToolCall(ToolCall::new(
                    "call_1",
                    "read",
                    serde_json::json!({"path": "a.txt"}),
                )));
                m
            }),
            // No ToolResult for call_1.
            Message::User(oxicode_ai::UserMessage::new("hi")),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        // The orphan tool_call is stripped (1 item); the assistant's text
        // and the user message are kept.
        assert_eq!(removed, 1);
        assert_eq!(messages.len(), 2);
        if let Message::Assistant(a) = &messages[0] {
            assert_eq!(a.content.len(), 1, "only the text block should remain");
            if let ContentBlock::Text(t) = &a.content[0] {
                assert_eq!(t.text, "let me check");
            } else {
                panic!("expected text block");
            }
        } else {
            panic!("expected assistant message");
        }
    }

    #[test]
    fn test_sanitize_orphan_tool_result_with_no_assistant_removed() {
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolResultMessage};
        // A tool result with no preceding assistant that has tool_calls
        // is an orphan and should be removed.
        let mut messages = vec![
            Message::User(oxicode_ai::UserMessage::new("hello")),
            Message::ToolResult(ToolResultMessage::new(
                "orphan_1",
                "bash",
                vec![ContentBlock::Text(TextContent::new("orphan output"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        assert_eq!(removed, 1);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_sanitize_wrong_tool_call_id_removed() {
        use oxicode_ai::{ContentBlock, Message, TextContent, ToolCall, ToolResultMessage};
        // A ToolResult whose tool_call_id doesn't match any active
        // assistant's tool_call_id is an orphan.
        let mut messages = vec![
            Message::Assistant({
                let mut m = oxicode_ai::AssistantMessage::new(
                    oxicode_ai::Api::OpenAiCompletions,
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
                "wrong_id", // doesn't match call_1
                "bash",
                vec![ContentBlock::Text(TextContent::new("orphan"))],
            )),
        ];
        let removed = sanitize_orphaned_tool_results(&mut messages);
        // Breakdown:
        //   - 1 wrong-id ToolResult removed
        //   - 1 ToolCall block stripped from the assistant (call_1 had no match)
        //   - 1 empty assistant dropped (only contained the orphan tool_call)
        assert_eq!(removed, 3);
        assert_eq!(messages.len(), 0);
    }
}
