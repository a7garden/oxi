//! oxi-tui v2 — terminal-first rendering pipeline + widget library.
//!
//! Greenfield rewrite. See `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md`.
//!
//! ## Module map
//!
//! - `pipeline`: terminal-first frame lifecycle (`draw_frame`, `CursorState`, `DiffBackend`)
//! - `widget`: retained tree + memoization (`Renderable`, `RetainedTree`, `RenderCtx`)
//! - `theme`: capability-aware palette (`palette`, `capability`, `serializer`)
//!
//! Higher-level modules (`content`, `text`, `link`, `input`, `widget/{chat,panel,primitive}`)
//! are added in Plans B/C.
#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod pipeline;
pub mod theme;
pub mod widget;
