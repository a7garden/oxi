// Vendored from grok-build (Apache-2.0, © 2023-2026 SpaceXAI). See NOTICE-vendored.md.
// Lint relaxations: third-party code copied verbatim. We do NOT chase upstream
// lint churn here; first-party crates keep the workspace `-D warnings` gate.
#![allow(deprecated)]
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_features)]
#![allow(unexpected_cfgs)]
#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(rustdoc::broken_intra_doc_links)]

mod common;
mod resize;
mod scrollback;
mod segment;
mod terminal;

#[cfg(test)]
mod tests;

pub use self::{
    common::{TerminalLike, with_synchronized_output},
    resize::{resize_purge_rerender, resize_viewport_height},
    scrollback::emit_to_scrollback,
    segment::split_into_line_segments,
    terminal::{LinkSpan, Terminal},
};
