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

pub mod renderable;

pub use renderable::{Renderable, hash_combine, hash_str};

// Forward-declared — RenderCtx comes in Task 8.