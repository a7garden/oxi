//! Widgets — ratatui-based UI components.
//!
//! This module provides TUI widgets built on top of ratatui's Widget/StatefulWidget
//! traits for the ratatui-based TUI implementation.
//!
//! # Architecture
//!
//! Each widget has two parts:
//! - **Widget struct** (`Foo<'a>`): holds references to shared data (theme, options).
//!   Implements `ratatui::widgets::StatefulWidget` or `Widget`.
//! - **State struct** (`FooState`): holds mutable state (scroll, cursor, user input).
//!   Implements `Default`.
//!
//! # Theme integration
//!
//! All widgets accept a `&Theme` reference and convert it to ratatui `Style`
//! via the `Theme::to_styles()` method.

pub mod chat;
pub mod command_palette;
pub mod footer;
pub mod input;