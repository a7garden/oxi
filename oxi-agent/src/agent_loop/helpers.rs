/// Helper functions for agent loop

use oxi_ai::{ContentBlock, ToolCall, TextContent, ToolResultMessage};

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
        vec![ContentBlock::Text(TextContent::new(finalized.result.output.clone()))]
    };

    ToolResultMessage::new(
        finalized.tool_call.id.clone(),
        &finalized.tool_call.name,
        content_blocks,
    )
}

/// Check if a batch of finalized tool calls should terminate the loop.
pub fn should_terminate_batch(finalized_calls: &[FinalizedToolCall]) -> bool {
    finalized_calls.iter().any(|f| f.result.terminate)
}

/// Check if loop should stop after a turn.
pub fn should_stop_after_turn(
    messages: &[oxi_ai::Message],
    assistant_message: &oxi_ai::AssistantMessage,
    max_iterations: usize,
) -> bool {
    let current_iteration = messages.iter().filter(|m| matches!(m, oxi_ai::Message::Assistant(_))).count();

    if current_iteration >= max_iterations {
        return true;
    }

    match assistant_message.stop_reason {
        oxi_ai::StopReason::Stop | oxi_ai::StopReason::Length => true,
        _ => false,
    }
}

use crate::AgentToolResult;

/// Finalized tool call with result.
pub struct FinalizedToolCall {
    pub tool_call: oxi_ai::ToolCall,
    pub result: AgentToolResult,
    pub is_error: bool,
}