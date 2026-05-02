//! Built-in components for oxi-tui.

pub mod chat_view;
pub mod completion;
pub mod editor;
pub mod image;
pub mod input;
pub mod markdown;
pub mod text;

pub use chat_view::{ChatMessageDisplay, ChatView, ContentBlockDisplay, MessageRole, StreamingState};
pub use completion::{Completion, FileCompleter};
pub use editor::{Editor, EditorOptions, Mention};
pub use image::{Image, ImageProtocol};
pub use input::{AutocompleteProvider, Input, InputOptions};
pub use markdown::{Markdown, MarkdownTheme};
pub use text::Text;