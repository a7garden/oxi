//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides terminal UI primitives and built-in components.

pub mod autocomplete;
pub mod cell;
pub mod component;
pub mod components;
pub mod event;
pub mod keybindings;
pub mod overlay;
pub mod renderer;
pub mod surface;
pub mod terminal;
pub mod theme;
pub mod tui;

pub use autocomplete::FuzzyMatcher;
pub use cell::{Attributes, Cell, CellBuilder, Color};
pub use component::Component;
pub use components::{AutocompleteProvider, ChatMessageDisplay, ChatView, Completion, ContentBlockDisplay, Editor, EditorOptions, FileCompleter, Image, ImageProtocol, Input, InputOptions, Markdown, MarkdownTheme, Mention, MessageRole, StreamingState, Text};
pub use event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, ResizeEvent};
pub use keybindings::{ActionId, KeybindingError, KeybindingRegistry, KeyName, KeySequence, actions};{OverlayBox, OverlayContent, OverlayHandle, OverlayOptions};
pub use renderer::Renderer;
pub use surface::{Rect, Surface};
pub use terminal::{CrosstermTerminal, Position, Size, Terminal};
pub use theme::{ColorScheme, FontScheme, Spacing, Theme, ThemeFile, ThemeManager};
pub use tui::TUI;