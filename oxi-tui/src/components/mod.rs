//! Built-in TUI components.
//!
//! This module provides common UI components for terminal applications.

pub mod text;
pub mod input;
pub mod select_list;

// Re-export component types
pub use text::{Text, Render as TextRender};
pub use input::{Input, KeyEvent};
pub use select_list::{SelectList, SelectItem};
