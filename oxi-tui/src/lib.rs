#![warn(missing_docs)]

//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides ratatui-based TUI widgets, theme system, and event types
//! for building terminal-based user interfaces.

pub mod cell;
pub mod event;
pub mod fuzzy;
pub mod theme;
pub mod widgets;

pub use cell::{Attributes, Color};
pub use event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, ResizeEvent,
};
pub use fuzzy::{fuzzy_match, fuzzy_rank, FuzzyResult};
pub use theme::{ColorScheme, FontScheme, Spacing, Theme, ThemeFile, ThemeManager, ThemeStyles};
