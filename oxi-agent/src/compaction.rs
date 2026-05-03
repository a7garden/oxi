//! Context compaction events

use oxi_ai::CompactedContext as AiCompactedContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionEvent {
    Triggered {
        context_tokens: usize,
        iteration: usize,
    },
    Started {
        message_count: usize,
    },
    Completed {
        result: CompactedContext,
        duration_ms: u64,
    },
    Skipped {
        reason: String,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
