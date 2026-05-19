//! Text rendering utilities for TUI widgets.

use unicode_width::UnicodeWidthChar;

/// Truncate a string to fit within `max_width` terminal columns.
///
/// Appends `…` (U+2026) if the string was truncated.
/// Returns an empty string if `max_width` is 0.
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    // First pass: check if everything fits
    let total_width: usize = s
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum();
    if total_width <= max_width {
        return s.to_string();
    }
    // Second pass: truncate to max_width - 1 (reserve 1 col for ellipsis)
    let target = max_width.saturating_sub(1);
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > target {
            break;
        }
        width += cw;
        end = i + ch.len_utf8();
    }
    format!("{}\u{2026}", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_to_width() {
        let text = "hello world";
        assert_eq!(truncate_to_width(text, 100), "hello world");
        assert_eq!(truncate_to_width(text, 5), "hell…");
        assert_eq!(truncate_to_width(text, 0), "");
        // Exact fit — no truncation
        assert_eq!(truncate_to_width("hello", 5), "hello");
        assert_eq!(truncate_to_width("abc", 3), "abc");
        // CJK exact fit (ABC=2 chars → total width 4)
        assert_eq!(truncate_to_width("ABCD", 4), "ABCD");
        // CJK truncation
        assert_eq!(truncate_to_width("ABCDEFGH", 5), "ABCD…");
    }
}
