//! omp-aligned append-only tape rendering engine.
//!
//! Port of omp's (`packages/tui/src/tui.ts`) native-scrollback model to Rust.
//! The core innovation: finalized rows commit to the terminal's native
//! scrollback (immutable), and only the live region (mutable suffix) repaints
//! in-place. This eliminates full-viewport re-rendering on every frame.
//!
//! ## Architecture
//!
//! ```text
//! TapeEngine
//! ├── committed_prefix: Vec<String>   — finalized rows in scrollback
//! ├── previous_window: Vec<String>    — last visible frame (for diffing)
//! ├── committed_rows: usize           — prefix length
//! ├── window_top: usize               — scroll offset
//! └── live_region_start: Option<usize> — mutable suffix boundary
//! ```
//!
//! ## Rendering flow
//!
//! 1. Compose: `root.render(width)` → `Vec<String>` (ANSI lines)
//! 2. Classify: `fullPaint` (resize/session-replace) vs `update` (differential)
//! 3. Emit:
//!    - `emit_full_paint`: rewrite from home, optionally ED3 clear scrollback
//!    - `emit_update`: commit new chunk + repaint changed window rows only

pub mod component;
pub mod container;
pub mod engine;
pub mod line;

pub use component::{Component, LiveRegion, RenderResult};
pub use container::Container;
pub use engine::{FrameOutcome, PaintOptions, TapeEngine};
pub use line::line_to_ansi;
