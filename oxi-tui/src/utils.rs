use unicode_width::UnicodeWidthStr;

/// Marker string used to identify cursor position in rendered output
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Calculate the visible width of a string (excluding ANSI escape codes)
pub fn visible_width(s: &str) -> usize {
    // Strip ANSI escape sequences for width calculation
    let stripped = strip_ansi(s);
    UnicodeWidthStr::width(&*stripped)
}

/// Truncate a string to fit within the specified visible width
pub fn truncate_to_width(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let stripped = strip_ansi(s);
    let visible = &*stripped;

    if UnicodeWidthStr::width(visible) <= width {
        return s.to_string();
    }

    // Find the point where we exceed the target width
    let mut current_width = 0;
    let mut end_pos = 0;

    for (i, c) in visible.char_indices() {
        let char_width = UnicodeWidthChar::width(c).unwrap_or(1);
        if current_width + char_width > width {
            break;
        }
        current_width += char_width;
        end_pos = i + c.len_utf8();
    }

    // Return the original string up to that point
    s[..end_pos].to_string()
}

/// Simple ANSI escape sequence stripper
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Start of ANSI escape sequence
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Skip until we hit a letter (the sequence terminator)
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphabetic() {
                        chars.next();
                        break;
                    }
                    chars.next();
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

trait UnicodeWidthChar {
    fn width(c: char) -> Option<usize>;
}

impl UnicodeWidthChar for char {
    fn width(c: char) -> Option<usize> {
        Some(UnicodeWidthStr::width(&c.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visible_width_simple() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn test_visible_width_unicode() {
        // Wide characters (East Asian)
        assert_eq!(visible_width("日本語"), 6); // 3 characters * 2 width each
    }

    #[test]
    fn test_truncate_basic() {
        assert_eq!(truncate_to_width("hello world", 5), "hello");
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_unicode() {
        let s = "日本語テキスト";
        let truncated = truncate_to_width(s, 4);
        // Should truncate to fit within 4 cell width
        assert!(visible_width(&truncated) <= 4);
    }

    #[test]
    fn test_strip_ansi() {
        let with_ansi = "\x1b[31mred\x1b[0m";
        assert_eq!(strip_ansi(with_ansi), "red");
    }
}