#![warn(missing_docs)]
#![warn(clippy::unwrap_used)]
#![allow(clippy::unwrap_used_in_tests)]

//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides ratatui-based TUI widgets, theme system, and event types
//! for building terminal-based user interfaces.

pub mod cell;
pub mod fuzzy;
pub mod theme;
pub mod widgets;

pub use cell::Color;
pub use fuzzy::{fuzzy_match, fuzzy_rank, FuzzyResult};
pub use theme::{ColorScheme, Spacing, Theme, ThemeFile, ThemeManager, ThemeStyles};
