//! Stream outcome types for TTSR integration.
//!
//! Extends the return type of [`super::streaming::stream_assistant_response`]
//! to signal TTSR rule violations without repurposing the existing cancel /
//! error mechanisms.

use oxi_ai::AssistantMessage;
use super::ttsr::Rule;

/// Result of a streaming completion attempt.
pub enum StreamOutcome {
    /// Normal completion — the assistant message is complete.
    Complete(AssistantMessage),

    /// User-initiated cancellation (Ctrl+C).
    Cancelled(AssistantMessage),

    /// TTSR rule violation detected during streaming.
    /// The caller should inject the rule as a system reminder and retry.
    RuleInterrupt {
        /// The partial assistant message at the point of interruption.
        partial: AssistantMessage,
        /// The rule that was violated.
        rule: Rule,
    },

    /// Provider error (stream ended with an error event).
    Error(AssistantMessage),
}

impl StreamOutcome {
    /// Extract the assistant message regardless of outcome.
    pub fn into_message(self) -> AssistantMessage {
        match self {
            StreamOutcome::Complete(m)
            | StreamOutcome::Cancelled(m)
            | StreamOutcome::Error(m) => m,
            StreamOutcome::RuleInterrupt { partial, .. } => partial,
        }
    }
}
