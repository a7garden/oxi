//! Overlay anchor layout resolver for the CLI layer.
//!
//! Bridges the shared `OverlayLayout` / `OverlayAnchor` types from `oxi-tui`
//! with the CLI overlay rendering system.

#[allow(unused_imports)]
pub use oxi_tui::overlay_anchor::{
    resolve_overlay_layout, OverlayAnchor, OverlayLayout, SizeValue,
};

/// Composite an overlay line onto a base line at a given column.
///
/// This is the line-level compositing function that blends overlay content
/// onto the terminal buffer. Characters from `overlay` replace characters
/// in `base` starting at `col`, up to `width` columns.
///
/// Port of pi's `compositeLineAt`.
#[allow(dead_code)]
pub fn composite_line_at(base: &str, overlay: &str, col: usize, width: u16) -> String {
    use unicode_width::UnicodeWidthStr;

    let base_width = base.width();
    let mut result = String::with_capacity(base.len() + overlay.len());

    // Copy base characters up to the overlay start column
    let mut byte_pos = 0;
    let mut visual_col = 0;
    for ch in base.chars() {
        if visual_col >= col {
            break;
        }
        result.push(ch);
        visual_col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        byte_pos += ch.len_utf8();
    }

    // Pad if base is shorter than col
    while visual_col < col {
        result.push(' ');
        visual_col += 1;
    }

    // Overlay characters (up to width)
    let mut _overlay_col = 0;
    let max_col = col + width as usize;
    for ch in overlay.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if visual_col + ch_width > max_col {
            break;
        }
        result.push(ch);
        visual_col += ch_width;
        _overlay_col += ch_width;
    }

    // Pad remaining overlay space with spaces
    while visual_col < max_col && visual_col < col + width as usize {
        result.push(' ');
        visual_col += 1;
    }

    // Append remaining base after the overlay region
    let mut remaining_col = visual_col;
    let skip_remaining = false;
    for ch in base.chars().skip_by(byte_pos) {
        // skip bytes we've already consumed
        if skip_remaining {
            break;
        }
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if remaining_col + ch_width > base_width {
            break;
        }
        result.push(ch);
        remaining_col += ch_width;
    }

    result
}

/// Trait extension: skip chars by byte position.
trait SkipBytes: Iterator<Item = char> {
    fn skip_by(self, byte_pos: usize) -> SkipBytesIter<Self>
    where
        Self: Sized,
    {
        SkipBytesIter {
            inner: self.peekable(),
            bytes_consumed: 0,
            target: byte_pos,
        }
    }
}

impl<I: Iterator<Item = char>> SkipBytes for I {}

struct SkipBytesIter<I: Iterator<Item = char>> {
    inner: std::iter::Peekable<I>,
    bytes_consumed: usize,
    target: usize,
}

impl<I: Iterator<Item = char>> Iterator for SkipBytesIter<I> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        while self.bytes_consumed < self.target {
            let ch = self.inner.next()?;
            self.bytes_consumed += ch.len_utf8();
        }
        self.inner.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composite_basic() {
        let result = composite_line_at("Hello World", "XXXX", 0, 4);
        assert!(result.starts_with("XXXX"));
    }

    #[test]
    fn test_composite_middle() {
        let result = composite_line_at("Hello World", "XY", 3, 2);
        // Should have "Hel" + "XY" + remaining
        assert!(result.starts_with("HelXY"));
    }

    #[test]
    fn test_composite_short_base() {
        let result = composite_line_at("Hi", "World", 2, 5);
        // Base "Hi" is only 2 chars wide, so overlay starts after
        assert!(result.contains("World"));
    }

    #[test]
    fn test_resolve_layout_center() {
        let layout = OverlayLayout {
            anchor: OverlayAnchor::Center,
            width: SizeValue::Fixed(40),
            max_height: Some(10),
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&layout, 80, 24);
        assert_eq!(rect.width, 40);
        assert_eq!(rect.height, 10);
    }
}
