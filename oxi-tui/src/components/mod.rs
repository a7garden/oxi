//! Built-in components for oxi-tui.

pub mod box_comp;
pub mod cancellable_loader;
pub mod chat_view;
pub mod command_palette;
pub mod completion;
pub mod editor;
pub mod editor_component;
pub mod footer;
pub mod image;
pub mod input;
pub mod loader;
pub mod markdown;
pub mod model_selector_overlay;
pub mod select_list;
pub mod settings_list;
pub mod settings_overlay;
pub mod spacer;
pub mod text;
pub mod theme_selector;
pub mod truncated_text;

pub use box_comp::{BorderStyle, Box};
pub use cancellable_loader::CancellableLoader;
pub use chat_view::{
    ChatMessageDisplay, ChatView, ContentBlockDisplay, MessageRole, StreamingState,
};
pub use command_palette::{Command, CommandPalette};
pub use completion::{Completion, FileCompleter};
pub use editor::{Editor, EditorOptions, Mention};
pub use editor_component::EditorComponent;
pub use footer::{Footer, FooterData, FooterTheme};
pub use image::{Image, ImageProtocol};
pub use input::{AutocompleteProvider, Input, InputOptions};
pub use loader::Loader;
pub use markdown::{Markdown, MarkdownTheme};
pub use model_selector_overlay::{ModelItem, ModelSelectorOverlay, ModelSelectorTheme};
pub use select_list::{SelectItem, SelectList};
pub use settings_list::{SettingEntry, SettingValue, SettingsList};
pub use settings_overlay::SettingsOverlay;
pub use spacer::Spacer;
pub use text::Text;
pub use theme_selector::{OnThemePreviewFn, ThemeInfo, ThemeSelector};
pub use truncated_text::TruncatedText;
