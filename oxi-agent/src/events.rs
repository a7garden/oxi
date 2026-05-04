//! Agent event system

use crate::compaction::CompactionEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    // ── Lifecycle events (from pi-mono agent-loop) ──────────────────
    AgentStart {
        prompts: Vec<oxi_ai::Message>,
    },
    AgentEnd {
        messages: Vec<oxi_ai::Message>,
        stop_reason: Option<String>,
    },
    TurnStart {
        turn_number: u32,
    },
    TurnEnd {
        turn_number: u32,
        assistant_message: oxi_ai::Message,
        tool_results: Vec<oxi_ai::ToolResultMessage>,
    },

    // ── Message events (from pi-mono agent-loop) ────────────────────
    MessageStart {
        message: oxi_ai::Message,
    },
    MessageUpdate {
        message: oxi_ai::Message,
        delta: Option<String>,
    },
    MessageEnd {
        message: oxi_ai::Message,
    },

    // ── Tool execution events (from pi-mono agent-loop) ────────────
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        partial_result: String,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: oxi_ai::ToolResult,
        is_error: bool,
    },

    // ── Legacy events (kept for backward compatibility) ──────────
    #[serde(rename = "start")]
    Start {
        prompt: String,
    },
    Thinking,
    ThinkingDelta {
        text: String,
    },
    TextChunk {
        text: String,
    },
    ToolCall {
        tool_call: oxi_ai::ToolCall,
    },
    ToolStart {
        tool_call_id: String,
        tool_name: String,
    },
    ToolProgress {
        tool_call_id: String,
        message: String,
    },
    ToolComplete {
        result: oxi_ai::ToolResult,
    },
    ToolError {
        tool_call_id: String,
        error: String,
    },
    Complete {
        content: String,
        stop_reason: String,
    },
    Error {
        message: String,
    },
    Iteration {
        number: usize,
    },
    Usage {
        input_tokens: usize,
        output_tokens: usize,
    },
    Compaction {
        event: CompactionEvent,
    },
    Retry {
        attempt: usize,
        max_retries: usize,
        retry_after_secs: u64,
        reason: String,
    },
    Fallback {
        from_model: String,
        to_model: String,
    },
    Cancelled,
    PartialResponse {
        content: String,
    },

    // ── Auto-retry events ─────────────────────────────────────────
    AutoRetryStart {
        attempt: usize,
        max_attempts: usize,
        delay_ms: u64,
        error_message: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: usize,
        final_error: Option<String>,
    },

    // ── Loop-specific steering events ─────────────────────────────
    SteeringMessage {
        message: oxi_ai::Message,
    },
    FollowUpMessage {
        message: oxi_ai::Message,
    },
}

impl AgentEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::AgentEnd { .. })
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            AgentEvent::AgentStart { .. } => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart { .. } => "turn_start",
            AgentEvent::TurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } => "message_update",
            AgentEvent::MessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
            AgentEvent::Start { .. } => "start",
            AgentEvent::Thinking => "thinking",
            AgentEvent::ThinkingDelta { .. } => "thinking_delta",
            AgentEvent::TextChunk { .. } => "text_chunk",
            AgentEvent::ToolCall { .. } => "tool_call",
            AgentEvent::ToolStart { .. } => "tool_start",
            AgentEvent::ToolProgress { .. } => "tool_progress",
            AgentEvent::ToolComplete { .. } => "tool_complete",
            AgentEvent::ToolError { .. } => "tool_error",
            AgentEvent::Complete { .. } => "complete",
            AgentEvent::Error { .. } => "error",
            AgentEvent::Iteration { .. } => "iteration",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::Compaction { .. } => "compaction",
            AgentEvent::Retry { .. } => "retry",
            AgentEvent::Fallback { .. } => "fallback",
            AgentEvent::Cancelled => "cancelled",
            AgentEvent::PartialResponse { .. } => "partial_response",
            AgentEvent::AutoRetryStart { .. } => "auto_retry_start",
            AgentEvent::AutoRetryEnd { .. } => "auto_retry_end",
            AgentEvent::SteeringMessage { .. } => "steering_message",
            AgentEvent::FollowUpMessage { .. } => "follow_up_message",
        }
    }
}
