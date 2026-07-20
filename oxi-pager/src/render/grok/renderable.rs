//! The [`Renderable`] trait for self-rendering content.
//!
//! This is the core rendering abstraction for virtualized scrolling.
//! Types implementing `Renderable` know:
//! - How tall they are at a given width (`desired_height`)
//! - How to render themselves into a buffer area (`render`)

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::WidgetRef;
use std::sync::Arc;

/// Trait for content that can render itself.
///
/// Implementors must be able to:
/// - Report their desired height at a given width
/// - Render into a provided rectangular area
///
/// The trait is object-safe to allow heterogeneous collections.
pub trait Renderable {
    /// Render content into the given area.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Height needed at this width in lines.
    ///
    /// This should be efficient (ideally O(1)) as it may be called
    /// frequently during scroll position calculations.
    fn desired_height(&self, width: u16) -> u16;
}

/// Owned or borrowed renderable item for composition.
pub enum RenderableItem<'a> {
    Owned(Box<dyn Renderable + 'a>),
    Borrowed(&'a dyn Renderable),
}

impl<'a> Renderable for RenderableItem<'a> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            RenderableItem::Owned(child) => child.render(area, buf),
            RenderableItem::Borrowed(child) => child.render(area, buf),
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self {
            RenderableItem::Owned(child) => child.desired_height(width),
            RenderableItem::Borrowed(child) => child.desired_height(width),
        }
    }
}

impl<'a> From<Box<dyn Renderable + 'a>> for RenderableItem<'a> {
    fn from(value: Box<dyn Renderable + 'a>) -> Self {
        RenderableItem::Owned(value)
    }
}

// ============================================================================
// Standard Implementations
// ============================================================================

/// Unit type renders as nothing (0 height).
impl Renderable for () {
    fn render(&self, _area: Rect, _buf: &mut Buffer) {}
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
}

/// String slices render as a single line.
impl Renderable for &str {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Owned strings render as a single line.
impl Renderable for String {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.as_str().render_ref(area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Spans render as a single line.
impl<'a> Renderable for Span<'a> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Lines render as a single line (no wrapping).
impl<'a> Renderable for Line<'a> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        WidgetRef::render_ref(&self, area, buf);
    }
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

// Note: Paragraph::line_count is unstable in ratatui, so we don't implement
// Renderable for Paragraph directly. Users should wrap text in custom types
// that handle their own height calculation.

/// Option<R> renders the inner value or nothing.
impl<R: Renderable> Renderable for Option<R> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(renderable) = self {
            renderable.render(area, buf);
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        if let Some(renderable) = self {
            renderable.desired_height(width)
        } else {
            0
        }
    }
}

/// Arc<R> delegates to inner.
impl<R: Renderable> Renderable for Arc<R> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.as_ref().render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        self.as_ref().desired_height(width)
    }
}

/// Box<R> delegates to inner.
impl<R: Renderable + ?Sized> Renderable for Box<R> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.as_ref().render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        self.as_ref().desired_height(width)
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
