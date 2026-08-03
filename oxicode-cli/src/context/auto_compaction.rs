//! Context management — auto-compaction types
//!
//! Provides the configuration and reason types used by the agent session
//! for context compaction.

/// Reason why compaction was triggered
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CompactionReason {
    /// User manually requested compaction
    Manual,
    /// Token usage crossed the configured threshold
    Threshold,
    /// Automatically triggered by the system
    Automatic,
    /// Context overflow detected
    Overflow,
    /// Compaction after a specific number of iterations
    Iteration {
        /// The iteration number that triggered compaction
        iteration: u32,
    },
}

/// Configuration for auto-compaction behavior
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Whether auto-compaction is enabled
    pub enabled: bool,
    #[allow(dead_code)]
    /// Token usage ratio threshold (0.0–1.0) that triggers compaction
    pub threshold: f32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.8,
        }
    }
}
