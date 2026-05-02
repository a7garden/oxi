//! Built-in components for oxi-tui.

pub mod completion;
pub mod editor;
pub mod input;
pub mod text;

pub use completion::{Completion, FileCompleter};
pub use editor::{Editor, EditorOptions, Mention};
pub use input::{AutocompleteProvider, Input, InputOptions};
pub use text::Text;