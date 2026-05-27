//! Overlay anchor positioning for TUI overlays.
//!
//! Provides 9-direction anchor positioning for overlay placement,
//! replacing the center-only layout used previously.

/// Anchor position for overlay placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAnchor {
    /// Center of the terminal.
    Center,
    /// Top-left corner.
    TopLeft,
    /// Top-center.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Left-center.
    LeftCenter,
    /// Right-center.
    RightCenter,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-center.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

/// Size specification for overlay dimensions.
#[derive(Debug, Clone, Copy)]
pub enum SizeValue {
    /// Fixed number of cells.
    Fixed(u16),
    /// Percentage of available space (0.0–1.0).
    Percent(f32),
}

/// Layout constraints for overlay positioning.
#[derive(Debug, Clone)]
pub struct OverlayLayout {
    /// Anchor position.
    pub anchor: OverlayAnchor,
    /// Width: fixed cells or percentage of terminal width.
    pub width: SizeValue,
    /// Minimum width in cells.
    pub min_width: Option<u16>,
    /// Maximum height in cells.
    pub max_height: Option<u16>,
    /// Horizontal offset from the anchor position.
    pub offset_x: i16,
    /// Vertical offset from the anchor position.
    pub offset_y: i16,
    /// Margin from terminal edges.
    pub margin: u16,
}

impl Default for OverlayLayout {
    fn default() -> Self {
        OverlayLayout {
            anchor: OverlayAnchor::Center,
            width: SizeValue::Percent(0.6),
            min_width: Some(30),
            max_height: Some(20),
            offset_x: 0,
            offset_y: 0,
            margin: 1,
        }
    }
}

/// Resolve an overlay layout into a concrete Rect within the terminal area.
///
/// Port of pi's `resolveOverlayLayout`: computes the overlay's position and
/// size based on the anchor, size constraints, and terminal dimensions.
pub fn resolve_overlay_layout(
    layout: &OverlayLayout,
    term_w: u16,
    term_h: u16,
) -> ratatui::layout::Rect {
    let margin = layout.margin;

    // Available area after margins
    let avail_w = term_w.saturating_sub(margin * 2);
    let avail_h = term_h.saturating_sub(margin * 2);

    // Resolve width
    let width = match layout.width {
        SizeValue::Fixed(w) => w.min(avail_w),
        SizeValue::Percent(pct) => ((avail_w as f32 * pct).ceil() as u16).min(avail_w),
    };
    let width = layout.min_width.map_or(width, |min| width.max(min));
    let width = width.min(avail_w);

    // Resolve height (estimate based on content or use max_height)
    let height = layout
        .max_height
        .map_or(avail_h / 2, |max| max.min(avail_h));

    // Position by anchor
    let (x, y) = match layout.anchor {
        OverlayAnchor::Center => (
            margin + (avail_w.saturating_sub(width)) / 2,
            margin + (avail_h.saturating_sub(height)) / 2,
        ),
        OverlayAnchor::TopLeft => (margin, margin),
        OverlayAnchor::TopCenter => (margin + (avail_w.saturating_sub(width)) / 2, margin),
        OverlayAnchor::TopRight => (margin + avail_w.saturating_sub(width), margin),
        OverlayAnchor::LeftCenter => (margin, margin + (avail_h.saturating_sub(height)) / 2),
        OverlayAnchor::RightCenter => (
            margin + avail_w.saturating_sub(width),
            margin + (avail_h.saturating_sub(height)) / 2,
        ),
        OverlayAnchor::BottomLeft => (margin, margin + avail_h.saturating_sub(height)),
        OverlayAnchor::BottomCenter => (
            margin + (avail_w.saturating_sub(width)) / 2,
            margin + avail_h.saturating_sub(height),
        ),
        OverlayAnchor::BottomRight => (
            margin + avail_w.saturating_sub(width),
            margin + avail_h.saturating_sub(height),
        ),
    };

    // Apply offsets (clamped to terminal bounds)
    let x = ((x as i16) + layout.offset_x)
        .max(0)
        .min(term_w.saturating_sub(width) as i16) as u16;
    let y = ((y as i16) + layout.offset_y)
        .max(0)
        .min(term_h.saturating_sub(height) as i16) as u16;

    ratatui::layout::Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_center_layout() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::Center,
            width: SizeValue::Fixed(40),
            max_height: Some(10),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.width, 40);
        assert_eq!(rect.height, 10);
        // Should be roughly centered
        assert!(rect.x >= 10);
        assert!(rect.y >= 2);
    }

    #[test]
    fn test_top_left_layout() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::TopLeft,
            width: SizeValue::Fixed(30),
            max_height: Some(5),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.x, 1); // margin
        assert_eq!(rect.y, 1); // margin
        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 5);
    }

    #[test]
    fn test_bottom_right_layout() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::BottomRight,
            width: SizeValue::Fixed(20),
            min_width: None,
            max_height: Some(5),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.width, 20);
        assert_eq!(rect.height, 5);
        // Should be near the right edge
        assert!(rect.x >= 50);
        // Should be near the bottom
        assert!(rect.y >= 15);
    }

    #[test]
    fn test_percent_width() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::Center,
            width: SizeValue::Percent(0.5),
            max_height: Some(10),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.width, 39); // (78 * 0.5) ≈ 39
    }

    #[test]
    fn test_min_width_enforced() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::Center,
            width: SizeValue::Fixed(10),
            min_width: Some(30),
            max_height: Some(5),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.width, 30); // min_width wins
    }

    #[test]
    fn test_offset_applied() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::TopLeft,
            width: SizeValue::Fixed(20),
            max_height: Some(5),
            offset_x: 5,
            offset_y: 3,
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.x, 6); // margin(1) + offset(5)
        assert_eq!(rect.y, 4); // margin(1) + offset(3)
    }

    #[test]
    fn test_small_terminal() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::Center,
            width: SizeValue::Percent(0.8),
            max_height: Some(10),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 20, 10);
        assert!(rect.width <= 18); // 20 - 2*margin
        assert!(rect.height <= 8);
    }
}
