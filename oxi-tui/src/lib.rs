#![warn(missing_docs)]
// Relax two test-idiom lints under `cfg(test)` so `cargo clippy --all-targets`
// stays clean without weakening the shipped library:
//   - `clippy::unwrap_used` — `unwrap()`/`unwrap_err()` are idiomatic in tests;
//     shipped (non-test) code still `warn`s on it (see the line below).
//   - `clippy::field_reassign_with_default` — the `let mut x = X::default();
//     x.f = ..;` test-setup pattern.
#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::field_reassign_with_default))]

//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides ratatui-based TUI widgets, theme system, and event types
//! for building terminal-based user interfaces.

pub mod cell;
pub mod fuzzy;
pub mod keybindings;
pub mod markdown_styles;
pub mod overlay_anchor;
pub mod render;
pub mod table_renderer;
pub mod text;
pub mod theme;
pub mod widgets;

/// Color representation for TUI rendering.
pub use cell::Color;

/// Fuzzy matching utilities for search/filter.
pub use fuzzy::{FuzzyResult, fuzzy_match, fuzzy_rank};

/// Truncate text to a terminal display width.
pub use text::truncate_to_width;

/// Theme system: color schemes, spacing, style management.
pub use theme::{ColorScheme, Spacing, Theme, ThemeFile, ThemeManager, ThemeStyles};
