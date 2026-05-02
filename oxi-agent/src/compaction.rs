//! Context compaction events
//!
//! Events emitted during the compaction process.

use oxi_ai::CompactedContext as AiCompactedContext;

/// Event emitted during compaction
#[derive(Debug, Clone)]
pub enum CompactionEvent {
    /// Compaction was triggered
    Triggered {
        /// Estimated context tokens at trigger time
        context_tokens: usize,
        /// Current iteration
        iteration: usize,
    },
    /// Compaction started
    Started {
        /// Number of messages being compacted
        message_count: usize,
    },
    /// Compaction completed
    Completed {
        /// The compacted context
        result: CompactedContext,
        /// Time taken in milliseconds
        duration_ms: u64,
    },
    /// Compaction was skipped (no compactor configured)
    Skipped {
        reason: String,
    },
    /// Compaction failed
    Failed {
        /// Error message
        error: String,
    },
}

/// Compacted context wrapper for oxi-agent
#[derive(Debug, Clone)]
pub struct CompactedContext {
    /// Summary of the compacted messages
    pub summary: String,
    /// Messages that were kept (typically recent ones)
    pub kept_messages: Vec<oxi_ai::Message>,
    /// Number of messages that were compacted
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
