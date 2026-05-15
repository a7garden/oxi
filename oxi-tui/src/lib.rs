#![warn(missing_docs)]
#![warn(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]

//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides ratatui-based TUI widgets, theme system, and event types
//! for building terminal-based user interfaces.

pub mod cell;
pub mod fuzzy;
pub mod table_renderer;
pub mod text;
pub mod theme;
pub mod widgets;

/// Color representation for TUI rendering.
pub use cell::Color;

/// Fuzzy matching utilities for search/filter.
pub use fuzzy::{fuzzy_match, fuzzy_rank, FuzzyResult};

/// Truncate text to a terminal display width.
pub use text::truncate_to_width;

/// Theme system: color schemes, spacing, style management.
pub use theme::{ColorScheme, Spacing, Theme, ThemeFile, ThemeManager, ThemeStyles};
