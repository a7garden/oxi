//! oxicode-hashline — line-anchored patch format for AI-assisted code editing.
//!
//! Pure-function library: no filesystem, no agent runtime, no schema library.
//! The host (oxicode-agent) injects a [`patcher::HashlineFs`] implementation.
//!
//! Ported from omp's `packages/hashline/` (TypeScript). Same algorithms,
//! same test contracts, Rust idioms.
#![warn(missing_docs)]

pub mod apply;
pub mod diff_preview;
pub mod format;
pub mod grammar;
pub mod messages;
pub mod mismatch;
pub mod normalize;
pub mod parser;
pub mod patcher;
pub mod recovery;
pub mod snapshots;
pub mod tokenizer;
pub mod types;

// Re-export the most-used items at crate root.
pub use apply::apply_edits;
pub use format::compute_file_hash;
pub use normalize::{normalize_to_lf, strip_bom};
pub use parser::{Patch, PatchSection, split_patch_input};
pub use patcher::{HashlineFs, Patcher};
pub use snapshots::{InMemorySnapshotStore, Snapshot, SnapshotStore};
pub use types::{Anchor, Cursor, Edit};
