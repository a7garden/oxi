//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides terminal UI primitives and built-in components.

pub mod components;

pub use components::text::Text;
pub use components::input::Input;
pub use components::select_list::{SelectList, SelectItem};
