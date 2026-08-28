//! Final-stage width invariant for transcript rows.
//!
//! `clamp_segments_to_width` is the safety net applied at every write exit
//! that funnels rendered `InlineSegment` rows into the terminal: no matter
//! which markdown block produced the row, every output row must fit inside
//! the physical terminal width (omp `tui-core-renderer.md` §4). It cuts at
//! display-width boundaries using `unicode-width`, preserving the original
//! segment styles, and never orphans a zero-width character at the line
//! end — the simplest correct rule is "stop before the char that would
//! overflow".
use std::sync::Arc;

use oxicode_vtui_compat::ui_protocol::{InlineSegment, InlineTextStyle};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Cut `segs` so its total display width never exceeds `width`.
///
/// - `width == 0`: returns an empty `Vec` (no row can hold even one cell).
/// - `width >= total content width`: identity — every segment preserved.
/// - Otherwise: walks the concatenated text char-by-char (display-width
///   aware), keeps whole segments that fit, truncates the first segment
///   that crosses the boundary, and drops the rest.
///
/// Returns owned `InlineSegment`s so callers can re-render without
/// lifetime entanglement with the input slice.
#[must_use]
pub fn clamp_segments_to_width(segs: &[InlineSegment], width: u16) -> Vec<InlineSegment> {
    if width == 0 {
        return Vec::new();
    }
    let max = width as usize;
    let mut out: Vec<InlineSegment> = Vec::with_capacity(segs.len());
    let mut used: usize = 0;
    for seg in segs {
        let seg_w = seg.text.width();
        if used + seg_w <= max {
            used += seg_w;
            out.push(seg.clone());
            continue;
        }
        // This segment (or part of it) crosses the boundary. Slice it
        // by char so display-width never overshoots, then drop.
        let remaining = max.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let mut kept = String::new();
        let mut kept_w = 0usize;
        for ch in seg.text.chars() {
            let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if kept_w + ch_w > remaining {
                break;
            }
            kept.push(ch);
            kept_w += ch_w;
        }
        if !kept.is_empty() {
            out.push(InlineSegment {
                text: kept,
                style: Arc::clone(&seg.style),
            });
        }
        // Even if the segment fit exactly into remaining, we are full
        // and any later segment would overflow — stop.
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> InlineSegment {
        InlineSegment {
            text: text.to_string(),
            style: Arc::new(InlineTextStyle::default()),
        }
    }

    fn styled(text: &str, fg: anstyle::Color) -> InlineSegment {
        InlineSegment {
            text: text.to_string(),
            style: Arc::new(InlineTextStyle::default().with_color(Some(fg))),
        }
    }

    #[test]
    fn clamp_cuts_cjk_at_boundary() {
        // "한글" each is display-width 2. Width 5 holds "한글" (4) + one
        // ASCII char; the second CJK char would push to 6 and must be
        // dropped at the boundary — never orphan a half-width char.
        let segs = vec![plain("한글abc")];
        let cut = clamp_segments_to_width(&segs, 5);
        let combined: String = cut.iter().map(|s| s.text.as_str()).collect();
        assert!(
            combined.width() <= 5,
            "clamped CJK row exceeds width: {combined:?} (width {})",
            combined.width()
        );
        assert_eq!(combined, "한글a", "wrong cut at CJK boundary: {combined:?}");
    }

    #[test]
    fn clamp_preserves_styles_of_kept_segments() {
        let fg = anstyle::Color::Rgb(anstyle::RgbColor(0xff, 0x00, 0x00));
        let segs = vec![plain("aaa"), styled("bbb", fg), plain("ccc")];
        let cut = clamp_segments_to_width(&segs, 5);
        assert_eq!(cut.len(), 2, "expected 2 kept segments, got {cut:?}");
        // First segment kept whole — default style.
        assert_eq!(cut[0].style.as_ref(), &InlineTextStyle::default());
        assert_eq!(cut[0].text, "aaa");
        // Second segment preserved with its custom color.
        assert_eq!(cut[1].style.as_ref().color, Some(fg));
        assert_eq!(cut[1].text, "bb");
    }

    #[test]
    fn clamp_zero_width_returns_empty() {
        let segs = vec![plain("anything")];
        let cut = clamp_segments_to_width(&segs, 0);
        assert!(
            cut.is_empty(),
            "width 0 must yield empty output, got {cut:?}"
        );
    }

    #[test]
    fn clamp_wider_than_content_is_identity() {
        let fg = anstyle::Color::Rgb(anstyle::RgbColor(0x12, 0x34, 0x56));
        let segs = vec![plain("hello"), styled("world", fg)];
        let cut = clamp_segments_to_width(&segs, 80);
        assert_eq!(cut.len(), 2);
        assert_eq!(cut[0].text, "hello");
        assert_eq!(cut[0].style.as_ref(), &InlineTextStyle::default());
        assert_eq!(cut[1].text, "world");
        assert_eq!(cut[1].style.as_ref().color, Some(fg));
    }
}
