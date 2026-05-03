//! Component trait for UI building blocks.

use crate::{Event, Rect, Size, Surface};

/// Component trait - all UI elements implement this.
pub trait Component: Send {
    /// Get the component's name for debugging.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// Request a render on the next frame.
    fn request_render(&mut self);

    /// Check if this component has pending render requests.
    fn is_dirty(&self) -> bool;

    /// Clear the dirty flag.
    fn clear_dirty(&mut self);

    /// Handle an input event.
    /// Returns true if the event was consumed.
    fn handle_event(&mut self, event: &Event) -> bool;

    /// Render this component into the given surface area.
    fn render(&mut self, surface: &mut Surface, area: Rect);

    /// Get the component's minimum size.
    fn min_size(&self) -> Size;

    /// Get desired size, may be larger than min_size.
    fn desired_size(&self) -> Option<Size> {
        None
    }

    /// Called when this component gains focus.
    fn on_focus(&mut self) {}

    /// Called when this component loses focus.
    fn on_unfocus(&mut self) {}

    /// Check if this component is currently focused.
    fn is_focused(&self) -> bool {
        false
    }

    /// Request focus for this component.
    fn focus(&mut self) {
        self.on_focus();
    }

    /// Remove focus from this component.
    fn unfocus(&mut self) {
        self.on_unfocus();
    }
}

/// Blanket implementation for Box<dyn Component>.
impl Component for Box<dyn Component> {
    fn name(&self) -> &str {
        self.as_ref().name()
    }

    fn request_render(&mut self) {
        self.as_mut().request_render()
    }

    fn is_dirty(&self) -> bool {
        self.as_ref().is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.as_mut().clear_dirty()
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        self.as_mut().handle_event(event)
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        self.as_mut().render(surface, area)
    }

    fn min_size(&self) -> Size {
        self.as_ref().min_size()
    }

    fn desired_size(&self) -> Option<Size> {
        self.as_ref().desired_size()
    }

    fn on_focus(&mut self) {
        self.as_mut().on_focus()
    }

    fn on_unfocus(&mut self) {
        self.as_mut().on_unfocus()
    }

    fn is_focused(&self) -> bool {
        self.as_ref().is_focused()
    }
}
