//! Context compaction events

use oxi_ai::CompactedContext as AiCompactedContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// CompactionEvent.
pub enum CompactionEvent {
/// Variant.
    Triggered {
        context_tokens: usize,
        iteration: usize,
    },
/// Variant.
    Started {
        message_count: usize,
    },
/// Variant.
    Completed {
        result: CompactedContext,
        duration_ms: u64,
    },
/// Variant.
    Skipped {
        reason: String,
    },
/// Variant.
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// CompactedContext.
pub struct CompactedContext {
    pub summary: String,
    pub kept_messages: Vec<oxi_ai::Message>,
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
