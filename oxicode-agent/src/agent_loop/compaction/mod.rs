//! Mechanical (LLM-free) context compaction strategies.
//!
//! These strategies elide token-heavy regions of the message log without
//! invoking the model. They complement the LLM-driven compactor wired
//! through `oxicode_ai::CompactionManager`.

/// Shake compaction — elide large tool results and code blocks from
/// older context while preserving a recent protect window.
///
/// Ported from omp `packages/agent/src/compaction/shake.ts`.
pub mod shake;
