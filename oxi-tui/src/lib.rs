//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides terminal UI primitives and built-in components.

pub mod cell;
pub mod component;
pub mod components;
pub mod event;
pub mod overlay;
pub mod renderer;
pub mod surface;
pub mod terminal;
pub mod tui;

pub use cell::{Attributes, Cell, CellBuilder, Color};
pub use component::Component;
pub use components::{Input, Text};
pub use event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, ResizeEvent};
pub use overlay::{OverlayBox, OverlayContent, OverlayHandle, OverlayOptions};
pub use renderer::Renderer;
pub use surface::{Rect, Surface};
pub use terminal::{CrosstermTerminal, Position, Size, Terminal};
pub use tui::TUI;