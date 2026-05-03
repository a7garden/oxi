//! Built-in components for oxi-tui.

pub mod box_comp;
pub mod chat_view;
pub mod command_palette;
pub mod completion;
pub mod editor;
pub mod footer;
pub mod image;
pub mod input;
pub mod loader;
pub mod markdown;
pub mod select_list;
pub mod settings_list;
pub mod settings_overlay;
pub mod spacer;
pub mod text;
pub mod theme_selector;
pub mod truncated_text;

pub use box_comp::{BorderStyle, Box};
pub use chat_view::{
    ChatMessageDisplay, ChatView, ContentBlockDisplay, MessageRole, StreamingState,
};
pub use command_palette::{Command, CommandPalette};
pub use completion::{Completion, FileCompleter};
pub use editor::{Editor, EditorOptions, Mention};
pub use footer::{Footer, FooterData, FooterTheme};
pub use image::{Image, ImageProtocol};
pub use input::{AutocompleteProvider, Input, InputOptions};
pub use loader::Loader;
pub use markdown::{Markdown, MarkdownTheme};
pub use select_list::{SelectItem, SelectList};
pub use settings_list::{SettingEntry, SettingValue, SettingsList};
pub use settings_overlay::SettingsOverlay;
pub use spacer::Spacer;
pub use text::Text;
pub use theme_selector::{ThemeInfo, ThemeSelector};
pub use truncated_text::TruncatedText;
