//! Helper functions for agent loop

use crate::compaction::CompactableMessage;
use oxi_ai::{ContentBlock, ToolCall};
use crate::AgentToolResult;
use oxi_ai::ToolResultMessage;

/// Resolve model from model ID string.
///
/// Returns the resolved model or None if not found.
pub fn resolve_model(model_id: &str) -> Option<oxi_ai::Model> {
    crate::model_id::resolve_model_from_id(model_id)
}

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
pub fn create_tool_result_message(finalized: &FinalizedToolCallRef) -> ToolResultMessage {
    let content_blocks = if let Some(ref blocks) = finalized.result.content_blocks {
        blocks.clone()
    } else {
        vec![ContentBlock::Text(oxi_ai::TextContent::new(finalized.result.output.clone()))]
    };

    ToolResultMessage::new(
        finalized.tool_call.id.clone(),
        &finalized.tool_call.name,
        content_blocks,
    )
}

/// Check if a batch of finalized tool calls should terminate the loop.
pub fn should_terminate_batch(finalized_calls: &[FinalizedToolCallRef]) -> bool {
    finalized_calls.iter().any(|f| f.result.terminate)
}

/// Reference type for finalized tool calls (used by helper functions).
pub struct FinalizedToolCallRef<'a> {
    pub tool_call: &'a oxi_ai::ToolCall,
    pub result: &'a AgentToolResult,
    pub is_error: bool,
}