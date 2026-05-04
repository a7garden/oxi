//! oxi-tui: Terminal UI library for oxi
//!
//! This crate provides terminal UI primitives and built-in components.

pub mod autocomplete;
pub mod cell;
pub mod component;
pub mod components;
pub mod event;
pub mod fuzzy;
pub mod keybindings;
pub mod kill_ring;
pub mod overlay;
pub mod renderer;
pub mod stdin_buffer;
pub mod surface;
pub mod terminal;
pub mod terminal_image;
pub mod theme;
pub mod tui;
pub mod undo_stack;
pub mod utils;

pub use autocomplete::FuzzyMatcher;
pub use cell::{Attributes, Cell, CellBuilder, Color};
pub use component::Component;
pub use components::{
    AutocompleteProvider, BorderStyle, Box, CancellableLoader, ChatMessageDisplay, ChatView,
    Command, CommandPalette, Completion, ContentBlockDisplay, Editor, EditorComponent,
    EditorOptions, FileCompleter, Footer, FooterData, FooterTheme, Image, ImageProtocol, Input,
    InputOptions, Loader, Markdown, MarkdownTheme, Mention, MessageRole, ModelItem,
    ModelSelectorOverlay, ModelSelectorTheme, OnThemePreviewFn, SelectItem, SelectList,
    SettingEntry, SettingValue, SettingsList, SettingsOverlay, Spacer, StreamingState, Text,
    ThemeInfo, ThemeSelector, TruncatedText,
};
pub use event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, ResizeEvent,
};
pub use fuzzy::{fuzzy_match, fuzzy_rank, FuzzyResult};
pub use kill_ring::KillRing;
pub use overlay::{OverlayBox, OverlayContent, OverlayHandle, OverlayOptions};
pub use renderer::Renderer;
pub use stdin_buffer::{BufferingMode, StdinBuffer, StdinBufferEvent, StdinBufferOptions};
pub use surface::{Rect, Surface};
pub use terminal::{CrosstermTerminal, Position, Size, Terminal};
pub use terminal_image::{
    calculate_image_rows, delete_all_kitty_images, delete_kitty_image, detect_capabilities,
    detect_format, encode_iterm2, encode_kitty, format_from_extension, get_image_dimensions,
    image_fallback, is_image_line, prepare_image, render_terminal_image, CellDimensions,
    ImageCache, ImageDimensions, ImageFormat, ImagePlaceholder,
    ImageProtocol as TerminalImageProtocol, RenderOptions, RenderedImage, TerminalCapabilities,
};
pub use theme::{ColorScheme, FontScheme, Spacing, Theme, ThemeFile, ThemeManager};
pub use tui::TUI;
pub use undo_stack::UndoStack;
