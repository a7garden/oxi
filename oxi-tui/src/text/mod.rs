//! Text shaping helpers for the oxi-tui v2 widget library.
//!
//! The wrap module is the foundation of streaming markdown rendering
//! (Plan B / Task 6) and the chat widgets. It mirrors the legacy
//! `oxi-tui-legacy/src/widgets/chat/markdown.rs` algorithm but lives in
//! the new crate so that plain text shaping can be exercised in
//! isolation from the markdown AST.
//!
//! ## Why CJK-aware wrapping
//!
//! Ratatui's built-in `Paragraph::wrap` only breaks on ASCII whitespace.
//! Korean / Chinese / Japanese text lacks spaces between characters, so
//! without CJK awareness the entire paragraph is treated as one giant
//! "word" and the line overflows the viewport. This module treats
//! every CJK character as its own breakable token while keeping ASCII
//! word-boundary semantics.

pub mod wrap;
pub mod streaming_md;

pub use wrap::{wrap_lines, wrap_lines_styled};
pub use streaming_md::StreamingMarkdown;
