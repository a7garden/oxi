//! Agent event system

use crate::types::{ToolCall, ToolResult};

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
    ToolCall { tool_call: ToolCall },
    /// Tool execution started
    ToolStart { tool_call_id: String, tool_name: String },
    /// Tool execution in progress with progress update
    ToolProgress { tool_call_id: String, message: String },
    /// Tool execution completed
    ToolComplete { result: ToolResult },
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
}
