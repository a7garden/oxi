//! Provider streaming events

use std::sync::Arc;

use crate::{AssistantMessage, StopReason, ToolCall};

/// Reason for a model fallback in MultiProvider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// Rate limit exceeded (HTTP 429).
    RateLimit,
    /// Context window exceeded.
    ContextOverflow,
    /// Auth / quota error (HTTP 401/403).
    AuthError,
    /// Network error or connection failure.
    NetworkError,
    /// Server-side error (HTTP 5xx).
    ServerError,
    /// Model returned an error response.
    ModelError,
    /// Circuit breaker is open.
    CircuitBreaker,
    /// Unknown or custom reason.
    Unknown,
}

impl FallbackReason {
    /// Returns a string representation for serialization/logging.
    pub fn as_str(&self) -> &'static str {
        match self {
            FallbackReason::RateLimit => "rate_limit",
            FallbackReason::ContextOverflow => "context_overflow",
            FallbackReason::AuthError => "auth_error",
            FallbackReason::NetworkError => "network_error",
            FallbackReason::ServerError => "server_error",
            FallbackReason::ModelError => "model_error",
            FallbackReason::CircuitBreaker => "circuit_breaker",
            FallbackReason::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Streaming events emitted by providers
///
/// Note: We use crate::AssistantMessage directly to avoid type alias conflicts
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ProviderEvent {
    /// Stream started with partial assistant message.
    Start {
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Text content block started.
    TextStart {
        /// Index of the content block in the message.
        content_index: usize,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Incremental text delta received.
    TextDelta {
        /// Index of the content block in the message.
        content_index: usize,
        /// The text delta to append.
        delta: String,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Text content block finished.
    TextEnd {
        /// Index of the content block in the message.
        content_index: usize,
        /// The complete text content.
        content: String,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Thinking content block started.
    ThinkingStart {
        /// Index of the content block in the message.
        content_index: usize,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Incremental thinking delta received.
    ThinkingDelta {
        /// Index of the content block in the message.
        content_index: usize,
        /// The thinking text delta to append.
        delta: String,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Thinking content block finished.
    ThinkingEnd {
        /// Index of the content block in the message.
        content_index: usize,
        /// The complete thinking content.
        content: String,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Tool call block started.
    ToolCallStart {
        /// Index of the content block in the message.
        content_index: usize,
        /// The tool call ID from the provider, if available at start time.
        /// Providers that only surface the ID later (in deltas/end) leave this `None`.
        tool_call_id: Option<String>,
        /// The tool name, if available at start time.
        tool_name: Option<String>,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Tool call delta received (partial JSON arguments).
    ToolCallDelta {
        /// Index of the content block in the message.
        content_index: usize,
        /// The delta string to append to tool arguments.
        delta: String,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
    },

    /// Tool call block finished.
    ToolCallEnd {
        /// Index of the content block in the message.
        content_index: usize,
        /// The complete tool call with resolved arguments.
        tool_call: ToolCall,
        /// Partial assistant message state.
        partial: Arc<AssistantMessage>,
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

    // ── Routing / Fallback events ─────────────────────────────────────────
    /// Model fallback occurred — primary model replaced by fallback.
    ///
    /// Emitted by `MultiProvider` when it switches from one model to another
    /// in the candidate list due to errors, circuit breaker opens, etc.
    FallbackStart {
        /// Model that was being attempted.
        from_model: String,
        /// Model that will be used instead.
        to_model: String,
        /// Reason for the fallback.
        reason: FallbackReason,
    },

    /// Fallback chain exhausted — all models failed.
    ///
    /// Emitted by `MultiProvider` when all candidates in the fallback chain
    /// have been exhausted without success.
    FallbackExhausted {
        /// All models that were tried, in order.
        models_tried: Vec<String>,
        /// Final error from the last model.
        final_error: String,
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
            _ => None,
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

    /// Check if this is a fallback event
    pub fn is_fallback(&self) -> bool {
        matches!(self, ProviderEvent::FallbackStart { .. })
    }

    /// Check if this is a fallback exhausted event
    pub fn is_fallback_exhausted(&self) -> bool {
        matches!(self, ProviderEvent::FallbackExhausted { .. })
    }
}
