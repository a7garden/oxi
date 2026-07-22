//! Capability-aware theme system. See spec §7.

pub mod capability;
mod constructors;
pub mod palette;

pub use capability::{ColorLevel, ImageProtocol, TerminalCaps, adapt_color};
pub use palette::{ColorScheme, Theme, ThemeStyles};
