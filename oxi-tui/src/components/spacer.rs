//! Spacer component - configurable empty space.

use crate::{Cell, Color, Component, Event, Rect, Size, Surface};

/// A spacer component that occupies configurable space.
pub struct Spacer {
    width: u16,
    height: u16,
    /// If true, the spacer expands to fill available space.
    flex: bool,
    bg_color: Option<Color>,
    dirty: bool,
}

impl Spacer {
    /// Create a spacer with fixed dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            flex: false,
            bg_color: None,
            dirty: true,
        }
    }

    /// Create a horizontal spacer (1 row tall).
    pub fn horizontal(width: u16) -> Self {
        Self::new(width, 1)
    }

    /// Create a vertical spacer (1 column wide).
    pub fn vertical(height: u16) -> Self {
        Self::new(1, height)
    }

    /// Create a flexible spacer that fills available space.
    pub fn flex() -> Self {
        Self {
            width: 0,
            height: 0,
            flex: true,
            bg_color: None,
            dirty: true,
        }
    }

    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg_color = Some(color);
        self
    }

    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.dirty = true;
    }
}

impl Component for Spacer {
    fn name(&self) -> &str {
        "Spacer"
    }

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

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        if let Some(bg) = self.bg_color {
            for r in area.y..area.y + area.height {
                for c in area.x..area.x + area.width {
                    surface.set(r, c, Cell::new(' ').with_bg(bg));
                }
            }
        }
        // If no bg color, we simply leave the area untouched (transparent)
    }

    fn min_size(&self) -> Size {
        if self.flex {
            Size {
                width: 0,
                height: 0,
            }
        } else {
            Size {
                width: self.width,
                height: self.height,
            }
        }
    }

    fn desired_size(&self) -> Option<Size> {
        if self.flex {
            // Request a large size to fill available space
            Some(Size {
                width: 1000,
                height: 1000,
            })
        } else {
            Some(Size {
                width: self.width,
                height: self.height,
            })
        }
    }
}
