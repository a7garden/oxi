/// Context compaction events
use oxicode_ai::CompactedContext as AiCompactedContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// CompactionEvent.
pub enum CompactionEvent {
    /// Variant.
    Triggered {
        /// context_tokens.
        context_tokens: usize,
        /// iteration.
        iteration: usize,
        /// Source of the `context_tokens` value:
        /// - `"provider-reported"`: the agent state's
        ///   `last_input_tokens` (ground truth from the most recent
        ///   provider `usage.input_tokens`).
        /// - `"bytes/4 heuristic (cold start)"`: the legacy
        ///   `serialized_json.len() / 4` estimate — used only on the
        ///   very first turn before any provider count is available.
        /// - `"empty"`: no messages have been sent yet.
        ///
        /// Added for issue #28 (gap 2) so consumers can tell whether
        /// a compaction trigger is based on real token accounting or
        /// the heuristic.
        source: String,
    },
    /// Variant.
    Started {
        /// message_count.
        message_count: usize,
    },
    /// Variant.
    Completed {
        /// result.
        result: CompactedContext,
        /// duration_ms.
        duration_ms: u64,
    },
    /// Variant.
    Skipped {
        /// reason.
        reason: String,
    },
    /// Variant.
    Failed {
        /// error.
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// CompactedContext.
pub struct CompactedContext {
    /// pub.
    pub summary: String,
    /// pub.
    pub kept_messages: Vec<oxicode_ai::Message>,
    /// pub.
    pub compacted_count: usize,
}

impl From<AiCompactedContext> for CompactedContext {
    fn from(ctx: AiCompactedContext) -> Self {
        Self {
            summary: ctx.summary,
            kept_messages: ctx.kept_messages,
            compacted_count: ctx.compacted_count,
        }
    }
}
