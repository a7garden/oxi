// SPDX-License-Identifier: Apache-2.0
//
// Derived from OpenAI Codex's `codex-rs/tui/src/render/renderable.rs`
// (commit 9ded177ce7c1c0bd2047f902936c177612ab3434, 2026-08-16).
//
// The implementation is intentionally narrowed to oxicode's presentation
// boundary: a measured view can be composed vertically without inheriting
// Codex protocol, configuration, or application dependencies.

//! Measured ratatui cells used to compose the transcript and bottom pane.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Widget},
};

/// A view that can report its height before it is allocated a rectangle.
///
/// This is the essential boundary used by Codex's chat surface: transcript
/// cells, the active streaming cell, and the bottom pane are independently
/// measurable. Keeping it here prevents the next renderer from becoming a
/// second agent state machine.
pub trait Renderable {
    /// Paint this view into its already-allocated rectangle.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Return the number of rows needed at `width` columns.
    fn desired_height(&self, width: u16) -> u16;
}

/// A vertical composition of independently measured views.
#[derive(Default)]
pub struct Column<'a> {
    children: Vec<Box<dyn Renderable + 'a>>,
}

impl<'a> Column<'a> {
    /// Create an empty column.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a child in display order.
    pub fn push(&mut self, child: impl Renderable + 'a) {
        self.children.push(Box::new(child));
    }

    /// Return whether the column has no children.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

impl Renderable for Column<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut y = area.y;
        for child in &self.children {
            if y >= area.bottom() {
                break;
            }
            let height = child.desired_height(area.width).min(area.bottom() - y);
            if height == 0 {
                continue;
            }
            let child_area = Rect::new(area.x, y, area.width, height);
            child.render(child_area, buf);
            y = y.saturating_add(height);
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.children.iter().fold(0_u16, |height, child| {
            height.saturating_add(child.desired_height(width))
        })
    }
}

/// A plain, wrapping text cell. Higher-level transcript cells can build
/// styled `Line`s and delegate wrapping/measurement here.
#[derive(Clone, Debug, Default)]
pub struct TextCell {
    lines: Vec<Line<'static>>,
}

impl TextCell {
    /// Construct a cell from display lines.
    pub fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines }
    }
}

impl Renderable for TextCell {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if !area.is_empty() {
            Paragraph::new(self.lines.clone()).render(area, buf);
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        if width == 0 {
            return 0;
        }
        self.lines.iter().fold(0_u16, |height, line| {
            let rows = (line.width() as u16).max(1).div_ceil(width);
            height.saturating_add(rows)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{style::Style, text::Span};

    #[test]
    fn column_measures_and_clips_children_to_its_area() {
        let mut column = Column::new();
        column.push(TextCell::new(vec![Line::from("one")]));
        column.push(TextCell::new(vec![Line::from(Span::styled(
            "two",
            Style::default(),
        ))]));

        assert_eq!(column.desired_height(10), 2);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 1));
        column.render(Rect::new(0, 0, 10, 1), &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "o");
    }

    #[test]
    fn text_cell_measures_wrapped_lines() {
        let cell = TextCell::new(vec![Line::from("123456")]);
        assert_eq!(cell.desired_height(4), 2);
    }
}
