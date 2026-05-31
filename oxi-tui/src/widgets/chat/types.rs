//! Core types for the chat widget.

// ── Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    Requested,
    Executing,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text {
        content: String,
    },
    Thinking {
        content: String,
        collapsed: bool,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
        result: Option<(String, bool)>,
        status: ToolCallStatus,
        /// Formatted execution duration (e.g. "1.2s").
        duration: Option<String>,
    },
    ToolResult {
        tool_name: String,
        content: String,
        is_error: bool,
    },
    Error {
        title: String,
        message: String,
        retryable: bool,
    },
    Image {
        mime_type: String,
        base64_data: String,
    },
    /// Welcome dashboard panel with environment info.
    Dashboard {
        info: crate::widgets::chat::DashboardInfo,
    },
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content_blocks: Vec<ContentBlock>,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct StreamingState {
    pub message: ChatMessage,
}
