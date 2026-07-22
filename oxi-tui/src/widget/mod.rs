//! Retained widget tree + memoization.
//!
//! Widgets live across frames (retained). Each frame:
//! 1. Pipeline calls `RetainedTree::any_hash_changed` — walks the tree,
//!    aggregates child hashes into root hash.
//! 2. If root hash unchanged AND no resize: pipeline skips render entirely.
//! 3. Otherwise: `RetainedTree::render` walks the tree, calling `render()`
//!    only on subtrees whose hash changed.
//!
//! See spec §5.

pub mod context;
pub mod primitive;
pub mod renderable;
pub mod tree;

pub use primitive::Text;

pub use crate::pipeline::diff_backend::{CellRange, LinkCollector, LinkTarget};
pub use context::{FocusTarget, RenderCtx};
pub use renderable::{Renderable, hash_combine, hash_str};
pub use tree::RetainedTree;
