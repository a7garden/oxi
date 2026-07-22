//! Capability-aware theme system. See spec §7.

pub mod capability;
mod constructors;
pub mod palette;
pub mod serializer;

pub use capability::{ColorLevel, ImageProtocol, TerminalCaps, adapt_color};
pub use palette::{ColorScheme, Theme, ThemeStyles};
pub use serializer::{load_theme, save_theme};
