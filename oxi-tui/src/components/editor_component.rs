//! Trait for custom editor components.
//!
//! Provides the [`EditorComponent`] trait which defines the interface for
//! pluggable editor implementations. Extensions can implement this trait
//! to provide custom editing modes (vim, emacs, etc.) while remaining
//! compatible with the core application.

use crate::autocomplete::FuzzyMatcher;
use crate::{Component, Event};

/// Trait for custom editor components.
///
/// This allows extensions to provide their own editor implementation
/// (e.g., vim mode, emacs mode, custom keybindings) while maintaining
/// compatibility with the core application.
///
/// # Required Methods
///
/// - [`getText`](EditorComponent::get_text) / [`setText`](EditorComponent::set_text) — core text access
/// - [`handleEvent`](EditorComponent::handle_input) — raw input handling
///
/// # Optional Methods
///
/// All optional methods have default no-op implementations.
///
/// # Example
///
/// ```ignore
/// struct VimEditor { /* ... */ }
///
/// impl EditorComponent for VimEditor {
///     fn get_text(&self) -> String { self.content.clone() }
///     fn set_text(&mut self, text: &str) { self.content = text.to_string(); }
///     fn handle_input(&mut self, event: &Event) { /* vim key handling */ }
///     // ...
/// }
/// ```
pub trait EditorComponent: Component + Send {
    // ── Core text access (required) ──────────────────────────────────

    /// Get the current text content.
    fn get_text(&self) -> String;

    /// Set the text content.
    fn set_text(&mut self, text: &str);

    /// Handle raw terminal input event.
    fn handle_input(&mut self, event: &Event);

    // ── Callbacks (optional) ─────────────────────────────────────────

    /// Called when user submits (e.g., Enter key).
    fn on_submit(&mut self, _text: &str) {}

    /// Called when text changes.
    fn on_change(&mut self, _text: &str) {}

    // ── History support (optional) ───────────────────────────────────

    /// Add text to history for up/down navigation.
    fn add_to_history(&mut self, _text: &str) {}

    // ── Advanced text manipulation (optional) ────────────────────────

    /// Insert text at current cursor position.
    fn insert_text_at_cursor(&mut self, _text: &str) {}

    /// Get text with any markers expanded (e.g., paste markers).
    /// Falls back to `get_text()` if not implemented.
    fn get_expanded_text(&self) -> String {
        self.get_text()
    }

    // ── Autocomplete support (optional) ──────────────────────────────

    /// Set the autocomplete provider.
    fn set_autocomplete_provider(&mut self, _matcher: FuzzyMatcher) {}

    // ── Appearance (optional) ────────────────────────────────────────

    /// Set horizontal padding.
    fn set_padding_x(&mut self, _padding: usize) {}

    /// Set max visible items in autocomplete dropdown.
    fn set_autocomplete_max_visible(&mut self, _max_visible: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rect, Size, Surface};

    /// A minimal editor component for testing the trait.
    struct TestEditor {
        content: String,
        dirty: bool,
    }

    impl TestEditor {
        fn new() -> Self {
            Self {
                content: String::new(),
                dirty: true,
            }
        }
    }

    impl Component for TestEditor {
        fn request_render(&mut self) {
            self.dirty = true;
        }

        fn is_dirty(&self) -> bool {
            self.dirty
        }

        fn clear_dirty(&mut self) {
            self.dirty = false;
        }

        fn handle_event(&mut self, _event: &Event) -> bool {
            false
        }

        fn render(&mut self, _surface: &mut Surface, _area: Rect) {}

        fn min_size(&self) -> Size {
            Size { width: 10, height: 1 }
        }
    }

    impl EditorComponent for TestEditor {
        fn get_text(&self) -> String {
            self.content.clone()
        }

        fn set_text(&mut self, text: &str) {
            self.content = text.to_string();
        }

        fn handle_input(&mut self, _event: &Event) {}
    }

    #[test]
    fn test_editor_component_trait() {
        let mut editor = TestEditor::new();
        assert_eq!(editor.get_text(), "");

        editor.set_text("hello");
        assert_eq!(editor.get_text(), "hello");
    }

    #[test]
    fn test_expanded_text_fallback() {
        let editor = TestEditor::new();
        assert_eq!(editor.get_expanded_text(), "");
    }

    #[test]
    fn test_boxed_editor_component() {
        let editor = TestEditor::new();
        let boxed: Box<dyn EditorComponent> = Box::new(editor);
        // Can't call methods on Box<dyn EditorComponent> without Component impl,
        // so just verify construction works.
        drop(boxed);
    }
}
