/// Context compaction events
use oxi_ai::CompactedContext as AiCompactedContext;
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
    pub kept_messages: Vec<oxi_ai::Message>,
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
