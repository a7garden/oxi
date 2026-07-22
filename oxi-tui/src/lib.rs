//! oxi-tui v2 — terminal-first rendering pipeline + widget library.
//!
//! Greenfield rewrite. See `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md`.
//!
//! ## Module map
//!
//! - `pipeline`: terminal-first frame lifecycle (`draw_frame`, `CursorState`, `DiffBackend`)
//! - `widget`: retained tree + memoization (`Renderable`, `RetainedTree`, `RenderCtx`)
//! - `theme`: capability-aware palette (`palette`, `capability`, `serializer`)
//! - `text`: CJK-aware word wrapping (`wrap_lines`, `wrap_lines_styled`)
//! - `input`: prompt input area (`InputArea` — stock `ratatui-textarea` wrapper)
//!
//! Higher-level modules (`content`, `link`, `widget/{chat,panel,primitive}`)
//! are added in Plans B/C.
#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod content;
pub mod input;
pub mod pipeline;
pub mod text;
pub mod theme;
pub mod widget;
