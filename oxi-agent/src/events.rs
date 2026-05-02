//! Agent event system

use crate::compaction::CompactionEvent;

/// Agent events emitted during agent execution
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Agent started processing a request
    Start { prompt: String },
    /// Thinking started
    Thinking,
    /// Text chunk received (for streaming)
    TextChunk { text: String },
    /// Tool call requested
    ToolCall { tool_call: oxi_ai::ToolCall },
    /// Tool execution started
    ToolStart { tool_call_id: String, tool_name: String },
    /// Tool execution in progress with progress update
    ToolProgress { tool_call_id: String, message: String },
    /// Tool execution completed
    ToolComplete { result: oxi_ai::ToolResult },
    /// Tool execution failed
    ToolError { tool_call_id: String, error: String },
    /// Response generation completed
    Complete { content: String, stop_reason: String },
    /// Error occurred
    Error { message: String },
    /// Iteration completed
    Iteration { number: usize },
    /// Token usage update
    Usage { input_tokens: usize, output_tokens: usize },
    /// Compaction event
    Compaction { event: CompactionEvent },
    /// Retry attempt for a transient error
    Retry {
        attempt: usize,
        max_retries: usize,
        retry_after_secs: u64,
        reason: String,
    },
    /// Falling back to a different model
    Fallback {
        from_model: String,
        to_model: String,
    },
}
