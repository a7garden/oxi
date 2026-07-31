//! Status bar — 1-row top bar with left/center/right segments.
//!
//! Ported from grok-build's `views/status_bar.rs` + `views/agent_status.rs`.
//!
//! Two APIs:
//! - [`StatusBar`] — simple left/center/right widget (ratatui `Widget`).
//! - [`StatusBarBuilder`] — composable right-aligned items with separators
//!   and hit-test area lookup.
//!
//! Styling is decoupled via the [`StatusBarStyling`] trait, mirroring the
//! existing `PanelStyleProvider` pattern.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

// ───────────────────────────────────────────────────────────────────────────
// StatusBarStyling trait
// ───────────────────────────────────────────────────────────────────────────

/// Trait for providing status bar styles.  Decouples the widget from any
/// specific theme type (mirrors `PanelStyleProvider`).
///
/// Implementors provide the styles needed to render the status bar:
/// - `background_style`: the base background fill
/// - `separator_style`: the `│` between right-aligned items
pub trait StatusBarStyling {
    /// Base background style for the status bar row.
    fn background_style(&self) -> Style;

    /// Style for the separator between right-aligned items.
    fn separator_style(&self) -> Style;
}

// ───────────────────────────────────────────────────────────────────────────
// StatusBar (simple left/center/right widget)
// ───────────────────────────────────────────────────────────────────────────

/// 1-row status bar displaying left, center, and right content.
pub struct StatusBar<'a> {
    /// Left-aligned content.
    pub left: Line<'a>,
    /// Center content.
    pub center: Option<Line<'a>>,
    /// Right-aligned content.
    pub right: Option<Line<'a>>,
    /// Base style for the whole row.
    pub style: Style,
}

impl<'a> StatusBar<'a> {
    /// Create a status bar with left content.
    #[must_use]
    pub fn new(left: Line<'a>) -> Self {
        Self {
            left,
            center: None,
            right: None,
            style: Style::default(),
        }
    }

    /// Set center content.
    #[must_use]
    pub fn center(mut self, line: Line<'a>) -> Self {
        self.center = Some(line);
        self
    }

    /// Set right content.
    #[must_use]
    pub fn right(mut self, line: Line<'a>) -> Self {
        self.right = Some(line);
        self
    }

    /// Set the base style.
    #[must_use]
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        buf.set_style(area, self.style);

        let left_w = self.left.width() as u16;
        buf.set_line(area.x, area.y, &self.left, left_w);

        if let Some(center) = self.center {
            let center_w = center.width() as u16;
            let center_x = area.x + (area.width.saturating_sub(center_w)) / 2;
            if center_x > area.x + left_w + 1 {
                buf.set_line(center_x, area.y, &center, center_w);
            }
        }

        if let Some(right) = self.right {
            let right_w = right.width() as u16;
            let right_x = area.x + area.width.saturating_sub(right_w);
            buf.set_line(right_x, area.y, &right, right_w);
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// StatusBarBuilder (composable right-aligned items)
// ───────────────────────────────────────────────────────────────────────────

/// A named status bar item.
struct StatusEntry {
    id: &'static str,
    line: Line<'static>,
    width: u16,
}

/// Builder for a composable status bar with right-aligned items.
///
/// Collect items with [`push`](Self::push), then call [`render`](Self::render)
/// to lay them out with separators and get back hit-test areas.
///
/// # Example
///
/// ```ignore
/// let mut status = StatusBarBuilder::new(&styles);
/// status.push("context", context_line);
/// status.push("badge", badge_line);
/// let areas = status.render(buf, status_bar_rect);
/// ```
pub struct StatusBarBuilder<'a, S: StatusBarStyling> {
    items: Vec<StatusEntry>,
    styles: &'a S,
    right_pad: u16,
}

impl<'a, S: StatusBarStyling> StatusBarBuilder<'a, S> {
    /// Create a new empty status bar builder.
    #[must_use]
    pub fn new(styles: &'a S) -> Self {
        Self {
            items: Vec::new(),
            styles,
            right_pad: 0,
        }
    }

    /// Set right-edge padding.
    #[must_use]
    pub fn right_pad(mut self, pad: u16) -> Self {
        self.right_pad = pad;
        self
    }

    /// Add a right-aligned item.
    pub fn push(&mut self, id: &'static str, line: Line<'static>) {
        let width = line.width() as u16;
        self.items.push(StatusEntry { id, line, width });
    }

    /// Separator string: ` │ `.
    const SEPARATOR: &'static str = " \u{2502} ";

    /// Render all items right-aligned into the given area.
    ///
    /// Returns a map of item ID → screen [`Rect`] for hit-testing.
    pub fn render(self, buf: &mut Buffer, area: Rect) -> HashMap<&'static str, Rect> {
        if area.height == 0 || area.width == 0 || self.items.is_empty() {
            return HashMap::new();
        }

        buf.set_style(area, self.styles.background_style());

        let sep_style = self.styles.separator_style();
        let sep_w = 3u16; // " │ " = 3 columns

        let items_width: u16 = self.items.iter().map(|e| e.width).sum();
        let num_seps = u16::try_from(self.items.len())
            .unwrap_or(0)
            .saturating_sub(1);
        let total_width = items_width + num_seps * sep_w;

        let start_x = area
            .x
            .saturating_add(area.width.saturating_sub(self.right_pad + total_width));
        let mut x = start_x;
        let mut areas = HashMap::new();

        for (i, entry) in self.items.iter().enumerate() {
            if i > 0 {
                let sep = Span::styled(Self::SEPARATOR, sep_style);
                buf.set_span(x, area.y, &sep, sep_w);
                x += sep_w;
            }
            buf.set_line(x, area.y, &entry.line, entry.width);
            areas.insert(
                entry.id,
                Rect {
                    x,
                    y: area.y,
                    width: entry.width,
                    height: 1,
                },
            );
            x += entry.width;
        }
        areas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStyles;
    impl StatusBarStyling for TestStyles {
        fn background_style(&self) -> Style {
            Style::default()
        }
        fn separator_style(&self) -> Style {
            Style::default()
        }
    }

    #[test]
    fn status_bar_renders_all_segments() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        StatusBar::new(Line::from("left"))
            .center(Line::from("center"))
            .right(Line::from("right"))
            .render(Rect::new(0, 0, 80, 1), &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "l");
        assert_eq!(buf[(75, 0)].symbol(), "r");
    }

    #[test]
    fn builder_right_aligns_items() {
        let styles = TestStyles;
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        let mut sb = StatusBarBuilder::new(&styles);
        sb.push("ctx", Line::from("5.2k"));
        sb.push("badge", Line::from("3/5"));
        let areas = sb.render(&mut buf, Rect::new(0, 0, 80, 1));
        assert!(areas.contains_key("ctx"));
        assert!(areas.contains_key("badge"));
    }

    #[test]
    fn builder_empty_returns_empty_map() {
        let styles = TestStyles;
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        let areas = StatusBarBuilder::new(&styles).render(&mut buf, Rect::new(0, 0, 80, 1));
        assert!(areas.is_empty());
    }
}
