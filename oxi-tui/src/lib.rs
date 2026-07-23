//! # oxi-tui — terminal-first rendering pipeline + widget library
//!
//! A greenfield rewrite of oxi's terminal UI layer. Instead of wrapping
//! ratatui's `Terminal::draw()`, oxi-tui **decomposes the frame lifecycle**
//! so the application owns the cursor-emission decision and can skip
//! rendering unchanged content entirely.
//!
//! One frame ([`pipeline::draw_frame`], ~14 LOC body):
//!
//! ```text
//! autoresize → hash-skip → render → flush(DiffBackend) → reconcile cursor → swap
//! ```
//!
//! - **Hash-skip**: if the retained tree's root hash is unchanged and the
//!   terminal wasn't resized, `draw_frame` skips rendering and flushes
//!   nothing — an idle frame costs ~zero.
//! - **Per-subtree memoization**: composite widgets wrap children in
//!   [`widget::RetainedChild`] so a streaming token change re-renders only
//!   the active subtree, not its siblings.
//! - **Cursor dedup**: [`pipeline::CursorState`] emits cursor escape bytes
//!   only on a real visibility/position change — the same position while
//!   visible emits zero bytes, which stops the cursor from flickering.
//!
//! ## Quick Start
//!
//! ```no_run
//! use oxi_tui::pipeline::diff_backend::DiffBackend;
//! use oxi_tui::pipeline::{CursorState, draw_frame_closure};
//! use oxi_tui::theme::{TerminalCaps, Theme};
//! use oxi_tui::widget::FocusTarget;
//! use ratatui::Terminal;
//!
//! let backend = DiffBackend::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()));
//! let mut terminal = Terminal::new(backend)?;
//! let mut cursor = CursorState::new();
//! let theme = Theme::dark();
//! let caps = TerminalCaps::detect();
//!
//! // Paint one frame. `render_fn` receives a `RenderCtx` whose buffer you
//! // write into; the pipeline wraps the writes in CSI 2026 synchronized
//! // output and reconciles the cursor afterwards.
//! draw_frame_closure(
//!     &mut terminal,
//!     &mut cursor,
//!     FocusTarget::None,
//!     &theme,
//!     &caps,
//!     |_ctx| { /* render your widgets here */ },
//! )?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! `draw_frame_closure` takes a transient closure — the cutover API. Once
//! your widgets implement [`widget::Renderable`], pass a
//! [`widget::RetainedTree`] to [`pipeline::draw_frame`] to get hash-memoized
//! skip for free.
//!
//! ## Module map
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`pipeline`] | frame lifecycle: `draw_frame`, `CursorState`, `DiffBackend` |
//! | [`widget`] | `Renderable` trait, `RetainedTree`, `RetainedChild`, `RenderCtx` |
//! | [`content`] | `ChatLog`, `ChatView`, streaming state |
//! | [`text`] | CJK-aware wrapping, streaming markdown, syntax highlighting |
//! | [`theme`] | capability-aware palette (`palette`, `capability`, `serializer`) |
//! | [`input`] | prompt input area (stock `ratatui-textarea` wrapper) |
//!
//! Spec: `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md`.
#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod content;
pub mod input;
pub mod pipeline;
pub mod text;
pub mod theme;
pub mod widget;
