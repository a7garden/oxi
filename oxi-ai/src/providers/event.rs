//! Provider streaming events

use crate::{StopReason, AssistantMessage, ToolCall};

/// Streaming events emitted by providers
///
/// Note: We use crate::AssistantMessage directly to avoid type alias conflicts
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    /// Stream started
    Start {
        partial: AssistantMessage,
    },
    
    /// Text block started
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    
    /// Text delta received
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    
    /// Text block ended
    TextEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    
    /// Thinking block started
    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    
    /// Thinking delta received
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    
    /// Thinking block ended
    ThinkingEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    
    /// Tool call block started
    ToolCallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    
    /// Tool call delta received
    ToolCallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    
    /// Tool call block ended
    ToolCallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    
    /// Stream completed successfully
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },
    
    /// Stream ended with error
    Error {
        reason: StopReason,
        error: AssistantMessage,
    },
}

impl ProviderEvent {
    /// Extract the partial assistant message if present
    pub fn partial(&self) -> Option<&AssistantMessage> {
        match self {
            ProviderEvent::Start { partial } |
            ProviderEvent::TextStart { partial, .. } |
            ProviderEvent::TextDelta { partial, .. } |
            ProviderEvent::TextEnd { partial, .. } |
            ProviderEvent::ThinkingStart { partial, .. } |
            ProviderEvent::ThinkingDelta { partial, .. } |
            ProviderEvent::ThinkingEnd { partial, .. } |
            ProviderEvent::ToolCallStart { partial, .. } |
            ProviderEvent::ToolCallDelta { partial, .. } |
            ProviderEvent::ToolCallEnd { partial, .. } => Some(partial),
            ProviderEvent::Done { message, .. } => Some(message),
            ProviderEvent::Error { error, .. } => Some(error),
        }
    }
    
    /// Check if this is a done event
    pub fn is_done(&self) -> bool {
        matches!(self, ProviderEvent::Done { .. })
    }
    
    /// Check if this is an error event
    pub fn is_error(&self) -> bool {
        matches!(self, ProviderEvent::Error { .. })
    }
}