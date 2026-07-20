//! Line and string utility functions for ratatui text manipulation.

use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub use super::tool_paths::{path_basename, path_for_tool_header, shorten_path};

/// Clone a borrowed ratatui `Line` into an owned `'static` line.
pub fn line_to_static(line: &Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|s| Span {
                style: s.style,
                content: std::borrow::Cow::Owned(s.content.to_string()),
            })
            .collect(),
    }
}

/// Append owned copies of borrowed lines to `out`.
pub fn push_owned_lines(src: &[Line<'_>], out: &mut Vec<Line<'static>>) {
    for l in src {
        out.push(line_to_static(l));
    }
}

/// True for a character unsafe to render from untrusted/server text:
/// C0/C1 controls (the terminal-escape-injection vector) plus the Unicode
/// bidi-control and zero-width/format set (Trojan-Source spoofing) — U+061C,
/// U+200B–200F, U+202A–202E, U+2060–206F, U+FEFF.
///
/// Shared by every untrusted-text strip/scrub site (chip labels, toast error
/// scrub, settings editor input) so the set never drifts between them.
pub fn is_unsafe_display_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
        )
}

/// Polyfill for nightly-only [`str::floor_char_boundary`].
/// Snaps a byte index down to the nearest char boundary.
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    let index = index.min(s.len());
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte offset at which cumulative display width exceeds `max_width`.
/// Returns `s.len()` when the entire string fits.
pub fn byte_offset_at_width(s: &str, max_width: usize) -> usize {
    let mut width = 0;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            return i;
        }
        width += cw;
    }
    s.len()
}

/// Truncate a string to fit within `max_width` display columns.
///
/// Uses Unicode-aware width measurement (handles CJK wide chars,
/// multi-byte UTF-8 like em-dash, etc.). If truncated, the last character
/// is replaced with `…` so the result fits within `max_width`.
///
/// Returns the original string (owned) if it already fits.
pub fn truncate_str(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let end = byte_offset_at_width(s, max_width);
    let needs_ellipsis = end < s.len();

    if needs_ellipsis && max_width > 1 {
        // Back up one char to make room for '…' (1 display column).
        let truncated_end = s[..end]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("{}…", &s[..truncated_end])
    } else if needs_ellipsis {
        "…".to_string()
    } else {
        s[..end].to_string()
    }
}

/// Truncate a styled `Line` (multiple spans) to fit within `max_width` display columns.
///
/// Walks spans left-to-right, consuming width budget. When the budget is
/// exhausted mid-span, that span is truncated and `…` is appended. Spans
/// beyond the budget are dropped. All styles are preserved.
///
/// Returns the line unchanged if it already fits.
pub fn truncate_line(line: Line<'static>, max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from(vec![]);
    }

    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total <= max_width {
        return line;
    }

    // Need room for the ellipsis (1 column).
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out: Vec<Span<'static>> = Vec::new();

    for span in line.spans {
        let sw = span.content.width();
        if used + sw <= budget {
            // Entire span fits.
            used += sw;
            out.push(span);
        } else {
            // Partial fit — truncate this span.
            let remaining = budget - used;
            if remaining > 0 {
                let truncated = take_width(&span.content, remaining);
                out.push(Span::styled(truncated, span.style));
            }
            // Append ellipsis with the same style as the last span.
            let ellipsis_style = out.last().map(|s| s.style).unwrap_or_default();
            out.push(Span::styled("\u{2026}", ellipsis_style));
            return Line::from(out);
        }
    }

    // Shouldn't reach here (total > max_width checked above), but be safe.
    Line::from(out)
}

/// Clip or pad a styled `Line` to exactly `width` display columns.
///
/// Wider lines are clipped on grapheme boundaries (a multi-`char` grapheme like
/// `⚠\u{FE0F}` is never split) with no ellipsis; narrower lines are padded with
/// trailing spaces. This keeps a rendered row "self-owning" — the app writes a
/// real cell in every column, so a terminal drawing a glyph wider than the app
/// measured cannot strand a stale cell past the row (the markdown-table ghost
/// glyph bug). Width uses [`UnicodeWidthStr`], matching the table layout.
///
/// `width` must be a bounded display width: the pad branch allocates
/// `width - total` spaces.
pub fn fit_line_to_width<'a>(line: Line<'a>, width: usize) -> Line<'a> {
    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total == width {
        return line;
    }

    let Line {
        style,
        alignment,
        mut spans,
    } = line;

    if total < width {
        spans.push(Span::raw(" ".repeat(width - total)));
        return Line {
            style,
            alignment,
            spans,
        };
    }

    // Wider than width: clip on grapheme boundaries, no ellipsis.
    let mut out: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let sw = span.content.width();
        if used + sw <= width {
            used += sw;
            out.push(span);
            if used == width {
                break;
            }
            continue;
        }
        // This span straddles the boundary — take whole graphemes that fit.
        let remaining = width - used;
        let mut taken = String::new();
        let mut taken_width = 0usize;
        for g in span.content.graphemes(true) {
            let gw = g.width();
            if taken_width + gw > remaining {
                break;
            }
            taken_width += gw;
            taken.push_str(g);
        }
        if !taken.is_empty() {
            out.push(Span::styled(taken, span.style));
            used += taken_width;
        }
        // A straddling wide grapheme leaves a 1-column gap; pad it.
        if used < width {
            out.push(Span::raw(" ".repeat(width - used)));
        }
        break;
    }

    Line {
        style,
        alignment,
        spans: out,
    }
}

/// Take the first `n` display columns from a string.
fn take_width(s: &str, n: usize) -> String {
    let mut width = 0;
    let mut end = s.len();
    for (i, ch) in s.char_indices() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > n {
            end = i;
            break;
        }
        width += cw;
    }
    s[..end].to_string()
}

/// Cascade-truncate multiple text elements to fit within `avail` display columns.
///
/// Returns `(type, description, activity, meta)` truncated to fit.
/// Priority (highest first): type, activity, meta. Description is truncated
/// first. If overhead (type + activity + meta) >= avail, description is dropped
/// and the remaining elements are cascaded: meta is dropped first, then
/// activity is truncated, then type.
pub fn cascade_truncate(
    avail: usize,
    type_text: &str,
    description: &str,
    activity_text: &str,
    meta_text: &str,
) -> (String, String, String, String) {
    let overhead = type_text.width() + activity_text.width() + meta_text.width();
    if overhead <= avail {
        let desc_max = avail - overhead;
        (
            type_text.to_string(),
            truncate_str(description, desc_max),
            activity_text.to_string(),
            meta_text.to_string(),
        )
    } else {
        let mut budget = avail;
        let td = if type_text.width() <= budget {
            budget -= type_text.width();
            type_text.to_string()
        } else {
            let s = truncate_str(type_text, budget);
            budget = 0;
            s
        };
        let ad = if budget == 0 {
            String::new()
        } else if activity_text.width() <= budget {
            budget -= activity_text.width();
            activity_text.to_string()
        } else {
            let s = truncate_str(activity_text, budget);
            budget = 0;
            s
        };
        let md = if budget == 0 {
            String::new()
        } else if meta_text.width() <= budget {
            meta_text.to_string()
        } else {
            truncate_str(meta_text, budget)
        };
        (td, String::new(), ad, md)
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
