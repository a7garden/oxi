//! Provider streaming events

use crate::{AssistantMessage, StopReason, ToolCall};

/// Streaming events emitted by providers
///
/// Note: We use crate::AssistantMessage directly to avoid type alias conflicts
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ProviderEvent {
    /// Stream started with partial assistant message.
    Start { partial: AssistantMessage },

    /// Text content block started.
    TextStart {
        /// Index of the content block in the message.
        content_index: usize,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Incremental text delta received.
    TextDelta {
        /// Index of the content block in the message.
        content_index: usize,
        /// The text delta to append.
        delta: String,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Text content block finished.
    TextEnd {
        /// Index of the content block in the message.
        content_index: usize,
        /// The complete text content.
        content: String,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Thinking content block started.
    ThinkingStart {
        /// Index of the content block in the message.
        content_index: usize,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Incremental thinking delta received.
    ThinkingDelta {
        /// Index of the content block in the message.
        content_index: usize,
        /// The thinking text delta to append.
        delta: String,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Thinking content block finished.
    ThinkingEnd {
        /// Index of the content block in the message.
        content_index: usize,
        /// The complete thinking content.
        content: String,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Tool call block started.
    ToolCallStart {
        /// Index of the content block in the message.
        content_index: usize,
        /// The tool call ID from the provider, if available at start time.
        /// Providers that only surface the ID later (in deltas/end) leave this `None`.
        tool_call_id: Option<String>,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Tool call delta received (partial JSON arguments).
    ToolCallDelta {
        /// Index of the content block in the message.
        content_index: usize,
        /// The delta string to append to tool arguments.
        delta: String,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Tool call block finished.
    ToolCallEnd {
        /// Index of the content block in the message.
        content_index: usize,
        /// The complete tool call with resolved arguments.
        tool_call: ToolCall,
        /// Partial assistant message state.
        partial: AssistantMessage,
    },

    /// Stream completed successfully.
    Done {
        /// Why the model stopped generating.
        reason: StopReason,
        /// The final assistant message.
        message: AssistantMessage,
    },

    /// Stream ended with an error.
    Error {
        /// The stop reason at time of error.
        reason: StopReason,
        /// Error details in assistant message form.
        error: AssistantMessage,
    },
}

impl ProviderEvent {
    /// Extract the partial assistant message if present
    pub fn partial(&self) -> Option<&AssistantMessage> {
        match self {
            ProviderEvent::Start { partial }
            | ProviderEvent::TextStart { partial, .. }
            | ProviderEvent::TextDelta { partial, .. }
            | ProviderEvent::TextEnd { partial, .. }
            | ProviderEvent::ThinkingStart { partial, .. }
            | ProviderEvent::ThinkingDelta { partial, .. }
            | ProviderEvent::ThinkingEnd { partial, .. }
            | ProviderEvent::ToolCallStart { partial, .. }
            | ProviderEvent::ToolCallDelta { partial, .. }
            | ProviderEvent::ToolCallEnd { partial, .. } => Some(partial),
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
