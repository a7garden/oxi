/// Helper functions for agent loop
use oxi_ai::{ContentBlock, TextContent, ToolCall, ToolResultMessage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

/// Check if loop should stop after a turn.
///
/// pi-mono: shouldStopAfterTurn is an optional hook. If no hook is defined,
/// the loop never stops here — it continues until no more tool calls AND
/// no more steering/follow-up messages.
///
/// oxi: We check external_stop (Ctrl+C) and max_iterations.
/// Stop/Length reasons are handled by the inner loop's has_more_tool_calls
/// condition, NOT here.
pub fn should_stop_after_turn(
    _messages: &[oxi_ai::Message],
    _assistant_message: &oxi_ai::AssistantMessage,
    max_iterations: usize,
    external_stop: &Arc<AtomicBool>,
    turn_number: usize,
) -> bool {
    // External stop (Ctrl+C)
    if external_stop.load(Ordering::SeqCst) {
        return true;
    }

    // Max iterations guard — uses caller-provided turn_number
    // instead of re-counting messages each time (O(n) → O(1)).
    if turn_number >= max_iterations {
        return true;
    }

    false
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
