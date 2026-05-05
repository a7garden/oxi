/// Agent event system
//!
/// Defines all events emitted during an agent run, including lifecycle,
/// streaming, tool execution, compaction, retry, and steering events.

use crate::compaction::CompactionEvent;
use serde::{Deserialize, Serialize};

/// Events emitted during agent execution.
///
/// Events are tagged with `type` and serialized as camelCase for JSON consumers.
/// This enum is `#[non_exhaustive]` — new variants may be added in future releases.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum AgentEvent {
    // ── Lifecycle events ──────────────────────────────────────────────

    /// Emitted when the agent begins processing a batch of prompts.
    AgentStart {
        /// The initial prompt messages sent to the agent.
        prompts: Vec<oxi_ai::Message>,
        /// Optional session identifier for correlation.
        session_id: Option<String>,
    },

    /// Emitted when the agent finishes all processing.
    AgentEnd {
        /// Final conversation messages.
        messages: Vec<oxi_ai::Message>,
        /// Why the agent stopped (e.g. `"end_turn"`, `"tool_use"`).
        stop_reason: Option<String>,
        /// Optional session identifier for correlation.
        session_id: Option<String>,
    },

    /// Emitted at the start of each agent loop turn.
    TurnStart {
        /// Zero-based turn index.
        turn_number: u32,
    },

    /// Emitted when a turn completes, including the assistant reply and tool results.
    TurnEnd {
        /// Turn index that just completed.
        turn_number: u32,
        /// The assistant message produced this turn.
        assistant_message: oxi_ai::Message,
        /// Tool results collected during this turn.
        tool_results: Vec<oxi_ai::ToolResultMessage>,
    },

    // ── Message events ────────────────────────────────────────────────

    /// A new message has been created in the conversation.
    MessageStart {
        /// The message that started.
        message: oxi_ai::Message,
    },

    /// A message has been updated with new content.
    MessageUpdate {
        /// The message in its current state.
        message: oxi_ai::Message,
        /// Incremental text delta since the last update, if available.
        delta: Option<String>,
    },

    /// A message has been finalized.
    MessageEnd {
        /// The completed message.
        message: oxi_ai::Message,
    },

    // ── Tool execution events ────────────────────────────────────────

    /// A tool is about to be executed.
    ToolExecutionStart {
        /// Unique identifier for this tool call.
        tool_call_id: String,
        /// Name of the tool being invoked.
        tool_name: String,
        /// JSON arguments passed to the tool.
        args: serde_json::Value,
    },

    /// Partial progress from a running tool execution.
    ToolExecutionUpdate {
        /// Identifier of the tool call producing the update.
        tool_call_id: String,
        /// Name of the tool.
        tool_name: String,
        /// Partial result text so far.
        partial_result: String,
    },

    /// A tool execution has finished.
    ToolExecutionEnd {
        /// Identifier of the completed tool call.
        tool_call_id: String,
        /// Name of the tool.
        tool_name: String,
        /// The tool result payload.
        result: oxi_ai::ToolResult,
        /// Whether the tool execution resulted in an error.
        is_error: bool,
    },

    // ── Legacy events (kept for backward compatibility) ──────────

    /// Legacy: agent started processing a prompt.
    #[serde(rename = "start")]
    Start {
        /// The user prompt that triggered the run.
        prompt: String,
    },

    /// Agent is waiting for the first response token.
    Thinking,

    /// Incremental thinking / reasoning text from the model.
    ThinkingDelta {
        /// The reasoning text delta.
        text: String,
    },

    /// A chunk of generated text from the model.
    TextChunk {
        /// The text delta to append.
        text: String,
    },

    /// The model requested a tool call.
    ToolCall {
        /// The tool call descriptor from the provider.
        tool_call: oxi_ai::ToolCall,
    },

    /// A tool execution has started.
    ToolStart {
        /// Identifier of the tool call.
        tool_call_id: String,
        /// Name of the tool being invoked.
        tool_name: String,
    },

    /// Progress update from a running tool.
    ToolProgress {
        /// Identifier of the tool call.
        tool_call_id: String,
        /// Human-readable progress message.
        message: String,
    },

    /// A tool execution has completed.
    ToolComplete {
        /// The tool result payload.
        result: oxi_ai::ToolResult,
    },

    /// A tool execution failed.
    ToolError {
        /// Identifier of the failed tool call.
        tool_call_id: String,
        /// Error description.
        error: String,
    },

    /// The agent produced a final response.
    Complete {
        /// Full response text.
        content: String,
        /// Stop reason string (e.g. `"EndTurn"`).
        stop_reason: String,
    },

    /// An error occurred during agent execution.
    Error {
        /// Human-readable error message.
        message: String,
        /// Optional session identifier.
        session_id: Option<String>,
    },

    /// Agent loop iteration counter update.
    Iteration {
        /// Current iteration number.
        number: usize,
    },

    /// Token usage report for a completed turn.
    Usage {
        /// Number of prompt / input tokens consumed.
        input_tokens: usize,
        /// Number of completion / output tokens produced.
        output_tokens: usize,
    },

    /// Context compaction lifecycle event.
    Compaction {
        /// The underlying compaction event detail.
        event: CompactionEvent,
    },

    /// The agent is retrying after a transient error.
    Retry {
        /// Current retry attempt (1-based).
        attempt: usize,
        /// Maximum number of retries allowed.
        max_retries: usize,
        /// Seconds until the next attempt.
        retry_after_secs: u64,
        /// Why the previous attempt failed.
        reason: String,
        /// Optional session identifier.
        session_id: Option<String>,
    },

    /// The agent switched to a fallback model.
    Fallback {
        /// Model that was being used before the failure.
        from_model: String,
        /// Fallback model that will be used instead.
        to_model: String,
    },

    /// The agent run was cancelled by the caller.
    Cancelled,

    /// A partial response delivered mid-stream (useful for UI rendering).
    PartialResponse {
        /// Accumulated response content so far.
        content: String,
    },

    // ── Auto-retry events ─────────────────────────────────────────

    /// An automatic retry attempt is starting.
    AutoRetryStart {
        /// Current retry attempt (1-based).
        attempt: usize,
        /// Total retry attempts that will be made.
        max_attempts: usize,
        /// Milliseconds before this attempt is sent.
        delay_ms: u64,
        /// The error that triggered the retry.
        error_message: String,
    },

    /// An automatic retry attempt has concluded.
    AutoRetryEnd {
        /// Whether the retry succeeded.
        success: bool,
        /// Which attempt this was (1-based).
        attempt: usize,
        /// Final error if the retry failed, `None` on success.
        final_error: Option<String>,
    },

    // ── Loop-specific steering events ─────────────────────────────

    /// A system-level steering message injected into the conversation.
    SteeringMessage {
        /// The steering message to add to the context.
        message: oxi_ai::Message,
    },

    /// A follow-up message appended to continue the conversation.
    FollowUpMessage {
        /// The follow-up message.
        message: oxi_ai::Message,
    },
}

impl AgentEvent {
    /// Returns `true` if this event represents the end of the agent lifecycle.
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::AgentEnd { .. })
    }

    /// Returns the snake_case variant name of this event (useful for logging / serialization).
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
