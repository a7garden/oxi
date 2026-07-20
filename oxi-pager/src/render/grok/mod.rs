// Vendored from grok-build (Apache-2.0, © 2023-2026 SpaceXAI).
// Source: xai-grok-pager-render/src/render/ + sibling top-level modules.
// See NOTICE-vendored.md.
//
// Lint relaxations: this is third-party code copied verbatim (with the
// minimum API-surface edits needed for ratatui 0.30 and oxi path layout).
// We do NOT chase upstream lint churn here; first-party oxi-pager code
// remains under the workspace `-D warnings` gate.
#![allow(unsafe_code)]
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

//! Low-level rendering utilities — vendored from grok-build.
//!
//! Generic rendering primitives used by the scrollback and viewport.
//! Module layout flattened from upstream's `pager-render/src/{render,glyphs,
//! syntax,host,terminal}/` into this single `grok/` subtree; the path
//! rewrites `crate::render::<sub>` → `crate::render::grok::<sub>`,
//! `crate::{glyphs,syntax,host,terminal}::` → `crate::render::grok::*::`
//! have been applied to keep cross-module references intact.

pub mod color;
pub mod draw;
pub mod glyphs;
pub mod highlight;
pub mod host;
pub mod image_overlay;
pub mod line_utils;
pub mod link_opener;
pub mod osc8;
pub mod renderable;
pub mod safe_buf;
pub mod scrollbar;
pub mod syntax;
pub mod terminal;
pub mod terminal_output;
pub mod tool_paths;
pub mod wrapping;

pub use renderable::Renderable;
pub use safe_buf::SafeBuf;
