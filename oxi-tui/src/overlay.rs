//! Overlay system for modal dialogs and popups.

use crate::event::Event;
use crate::{Component, Rect, Surface};

/// Configuration for overlay behavior.
#[derive(Debug, Clone)]
pub struct OverlayOptions {
    /// Whether clicking outside the overlay should close it.
    pub click_outside_close: bool,
    /// Whether pressing Escape should close it.
    pub escape_close: bool,
    /// Whether to capture all keyboard input.
    pub capture_keys: bool,
    /// Whether this is a modal overlay (blocks interaction with content below).
    pub modal: bool,
    /// Optional backdrop opacity (0.0 to 1.0).
    pub backdrop_opacity: Option<f32>,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            click_outside_close: true,
            escape_close: true,
            capture_keys: true,
            modal: true,
            backdrop_opacity: Some(0.5),
        }
    }
}

/// Handle for managing an active overlay.
///
/// This trait combines overlay lifecycle management with Component rendering,
/// so callers only need one boxed pointer instead of two.
pub trait OverlayHandle: Send {
    /// Hide the overlay (but keep it in the stack).
    fn hide(&mut self);

    /// Set the hidden state.
    fn set_hidden(&mut self, hidden: bool);

    /// Check if overlay is hidden.
    fn is_hidden(&self) -> bool;

    /// Bring this overlay to front.
    fn bring_to_front(&mut self);

    /// Get a mutable reference to the overlay content.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;

    // -- Component passthrough methods --

    /// Render the overlay onto the surface.
    fn render(&mut self, surface: &mut Surface, area: Rect);

    /// Handle an input event. Returns `true` if the event was consumed.
    fn handle_event(&mut self, event: &Event) -> bool;

    /// Check if the overlay needs re-rendering.
    fn is_dirty(&self) -> bool;

    /// Clear the dirty flag.
    fn clear_dirty(&mut self);

    /// Request a re-render.
    fn request_render(&mut self);
}

/// Trait for objects that can be rendered as overlay content.
pub trait OverlayContent: Component {
    /// Called when overlay is closed.
    fn on_close(&mut self) {}

    /// Check if this overlay should be closed.
    fn should_close(&self, _event: &Event) -> bool {
        false
    }
}

/// Simple overlay wrapper containing content and options.
pub struct OverlayBox<T: OverlayContent> {
    content: T,
    options: OverlayOptions,
    hidden: bool,
    id: usize,
}

impl<T: OverlayContent> OverlayBox<T> {
    pub fn new(content: T, options: OverlayOptions) -> Self {
        Self {
            content,
            options,
            hidden: false,
            id: 0, // Will be set by TUI
        }
    }

    pub fn content(&self) -> &T {
        &self.content
    }

    pub fn content_mut(&mut self) -> &mut T {
        &mut self.content
    }

    pub fn options(&self) -> &OverlayOptions {
        &self.options
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn set_id(&mut self, id: usize) {
        self.id = id;
    }
}

impl<T: OverlayContent + 'static> OverlayHandle for OverlayBox<T> {
    fn hide(&mut self) {
        self.hidden = true;
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }

    fn is_hidden(&self) -> bool {
        self.hidden
    }

    fn bring_to_front(&mut self) {
        // This is handled by the overlay manager
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    // -- Component passthrough delegations --

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        <Self as Component>::render(self, surface, area);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        <Self as Component>::handle_event(self, event)
    }

    fn is_dirty(&self) -> bool {
        <Self as Component>::is_dirty(self)
    }

    fn clear_dirty(&mut self) {
        <Self as Component>::clear_dirty(self);
    }

    fn request_render(&mut self) {
        <Self as Component>::request_render(self);
    }
}

// Ensure OverlayBox is also a Component
impl<T: OverlayContent> Component for OverlayBox<T> {
    fn name(&self) -> &str {
        self.content.name()
    }

    fn request_render(&mut self) {
        self.content.request_render();
    }

    fn is_dirty(&self) -> bool {
        self.content.is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.content.clear_dirty();
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if self.hidden {
            return false;
        }
        // Handle escape key if configured
        if self.options.escape_close {
            if let Event::Key(key) = event {
                if key.code == crate::event::KeyCode::Escape {
                    self.content.on_close();
                    return true;
                }
            }
        }
        self.content.handle_event(event)
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        if self.hidden {
            return;
        }
        // NOTE: Backdrop rendering is intentionally skipped here.
        // A full-surface fill would destroy content rendered by components
        // underneath the overlay. Proper backdrop rendering requires area
        // management (rendering only within the overlay's designated rect).
        // That will be added when overlay positioning is implemented.
        self.content.render(surface, area);
    }

    fn min_size(&self) -> crate::terminal::Size {
        self.content.min_size()
    }

    fn desired_size(&self) -> Option<crate::terminal::Size> {
        self.content.desired_size()
    }
}
