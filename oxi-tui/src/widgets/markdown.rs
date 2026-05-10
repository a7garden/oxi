//! Lightweight inline markdown parser for terminal rendering.
//!
//! Detects common markdown patterns (bold, inline code, headings, code fences,
//! lists, horizontal rules, links) and produces typed segments that the chat
//! renderer can style differently without changing its data structures.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Inline segments
// ---------------------------------------------------------------------------

/// A parsed inline segment within a single line.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// Plain text with no special formatting.
    Normal(String),
    /// Bold text (`**text**`).
    Bold(String),
    /// Italic text — kept for completeness but rarely triggered.
    Italic(String),
    /// Inline code (`code`).
    Code(String),
    /// Hyperlink with visible text and URL.
    Link { text: String, url: String },
}

/// Parse inline markdown patterns in a single line.
///
/// Handles: `` `code` ``, `**bold**`, `[text](url)`.
/// Everything else is returned as `Segment::Normal`.
pub fn parse_inline(line: &str) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut chars = line.chars().peekable();
    let mut normal = String::new();

    while let Some(c) = chars.next() {
        match c {
            // ── inline code ────────────────────────────────────────
            '`' => {
                if !normal.is_empty() {
                    segments.push(Segment::Normal(normal.clone()));
                    normal.clear();
                }
                let mut code = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '`' {
                        chars.next(); // consume closing backtick
                        break;
                    }
                    code.push(chars.next().unwrap());
                }
                segments.push(Segment::Code(code));
            }

            // ── bold **…** ─────────────────────────────────────────
            '*' if chars.peek() == Some(&'*') => {
                chars.next(); // consume second '*'
                if !normal.is_empty() {
                    segments.push(Segment::Normal(normal.clone()));
                    normal.clear();
                }
                let mut bold = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '*' {
                        chars.next(); // first closing '*'
                        if chars.peek() == Some(&'*') {
                            chars.next(); // second closing '*'
                            break;
                        } else {
                            // Was a single '*' inside bold — keep it.
                            bold.push('*');
                            continue;
                        }
                    }
                    bold.push(chars.next().unwrap());
                }
                segments.push(Segment::Bold(bold));
            }

            // ── link [text](url) ───────────────────────────────────
            '[' => {
                if !normal.is_empty() {
                    segments.push(Segment::Normal(normal.clone()));
                    normal.clear();
                }
                let mut text = String::new();
                while let Some(&next) = chars.peek() {
                    if next == ']' {
                        chars.next();
                        break;
                    }
                    text.push(chars.next().unwrap());
                }
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume '('
                    let mut url = String::new();
                    while let Some(&next) = chars.peek() {
                        if next == ')' {
                            chars.next();
                            break;
                        }
                        url.push(chars.next().unwrap());
                    }
                    segments.push(Segment::Link { text, url });
                } else {
                    // Not a proper link — put back as normal text.
                    segments.push(Segment::Normal(format!("[{}", text)));
                }
            }

            // ── italic _…_ ─────────────────────────────────────────
            '_' => {
                if !normal.is_empty() {
                    segments.push(Segment::Normal(normal.clone()));
                    normal.clear();
                }
                let mut italic = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '_' {
                        chars.next(); // consume closing '_'
                        break;
                    }
                    italic.push(chars.next().unwrap());
                }
                // Only add if we actually captured content
                if !italic.is_empty() {
                    segments.push(Segment::Italic(italic));
                } else {
                    // No closing '_' found — push as normal
                    normal.push('_');
                }
            }

            // ── normal char ────────────────────────────────────────
            _ => normal.push(c),
        }
    }

    // Un-closed trailing italic — fall back to normal text by
    // checking whether the open '_' consumed any chars.
    // (Already handled above; nothing extra needed here.)

    if !normal.is_empty() {
        segments.push(Segment::Normal(normal));
    }
    segments
}

// ---------------------------------------------------------------------------
// Line-type detection
// ---------------------------------------------------------------------------

/// The structural type of a markdown line.
#[derive(Debug, Clone, PartialEq)]
pub enum LineType {
    /// Ordinary text line (may contain inline markdown).
    Normal,
    /// ATX heading with level 1–6.
    Heading(u8),
    /// Opening or closing of a fenced code block.
    CodeFence { lang: String },
    /// Unordered or ordered list item.
    ListItem,
    /// Horizontal rule (`---`, `***`, `___`).
    HorizontalRule,
    /// Table separator line (e.g. `| --- | --- |`).
    TableSeparator { widths: Vec<usize> },
    /// Table data row (e.g. `| cell1 | cell2 |`).
    TableRow { cells: Vec<String> },
}

/// Detect the structural type of a line.
pub fn detect_line_type(line: &str) -> LineType {
    let trimmed = line.trim();

    // Code fence
    if trimmed.starts_with("```") {
        let lang = trimmed.trim_start_matches('`').trim().to_string();
        return LineType::CodeFence { lang };
    }

    // Horizontal rule
    if (trimmed.starts_with("---") || trimmed.starts_with("***") || trimmed.starts_with("___"))
        && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_' || c == ' ')
        && trimmed.len() >= 3
    {
        return LineType::HorizontalRule;
    }

    // ATX heading
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes > 0 && hashes <= 6 {
        // Must be followed by a space, end-of-line, or non-# char (e.g. emoji)
        // e.g. "# Title" → heading, "##🔧 기술" → heading, "####" → not heading
        let after = trimmed.chars().nth(hashes);
        if after.is_none() || after == Some(' ') || after != Some('#') {
            return LineType::Heading(hashes as u8);
        }
    }

    // Unordered list
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        return LineType::ListItem;
    }

    // Ordered list (e.g. "1. item")
    if let Some(dot_pos) = trimmed.find(". ") {
        let prefix = &trimmed[..dot_pos];
        if prefix.chars().all(|c| c.is_ascii_digit()) && !prefix.is_empty() {
            return LineType::ListItem;
        }
    }

    // Markdown table: must start and end with '|', have at least one internal '|'
    if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() >= 3 {
        let cells: Vec<&str> = trimmed.split('|').filter(|s| !s.is_empty()).collect();
        if cells.len() >= 2 {
            // Check if this is a separator line (all dashes/colons/spaces)
            let is_sep = cells.iter().all(|c| {
                let c = c.trim();
                c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
                    && c.contains('-')
            });
            if is_sep {
                let widths: Vec<usize> = cells.iter().map(|c| c.trim().len().max(3)).collect();
                return LineType::TableSeparator { widths };
            }
            // Regular table row
            let cell_strings: Vec<String> = cells.iter().map(|c| c.trim().to_string()).collect();
            return LineType::TableRow { cells: cell_strings };
        }
    }

    LineType::Normal
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

/// Style used for table borders.
pub fn table_border_style(base: Style) -> Style {
    base.fg(Color::Indexed(242))
}

/// Style used for table header cells.
pub fn table_header_style(base: Style) -> Style {
    base.add_modifier(Modifier::BOLD)
}

/// Style used for inline code spans.
pub fn code_style(base: Style) -> Style {
    base.bg(Color::Indexed(236))
}

/// Style used for bold text.
pub fn bold_style(base: Style) -> Style {
    base.add_modifier(Modifier::BOLD)
}

/// Style used for italic text.
pub fn italic_style(base: Style) -> Style {
    base.add_modifier(Modifier::ITALIC)
}

/// Style used for links (visible text portion).
pub fn link_style(base: Style) -> Style {
    base.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED)
}

/// Style used for heading text.
pub fn heading_style(base: Style, level: u8) -> Style {
    let s = base.add_modifier(Modifier::BOLD);
    // Optionally differentiate by level — for now all bold.
    let _ = level; // avoid unused warning
    s
}

/// Style used for code-block lines.
pub fn code_block_style(base: Style) -> Style {
    base.bg(Color::Indexed(234))
}

/// Compute left-indent for a heading based on its level (smaller level → no indent).
pub fn heading_text(line: &str, _level: u8) -> String {
    line.trim_start_matches('#').trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inline_normal() {
        let segs = parse_inline("hello world");
        assert_eq!(segs, vec![Segment::Normal("hello world".into())]);
    }

    #[test]
    fn parse_inline_code() {
        let segs = parse_inline("use `foo` here");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment::Normal("use ".into()));
        assert_eq!(segs[1], Segment::Code("foo".into()));
        assert_eq!(segs[2], Segment::Normal(" here".into()));
    }

    #[test]
    fn parse_inline_italic() {
        let segs = parse_inline("this is _italic_ text");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment::Normal("this is ".into()));
        assert_eq!(segs[1], Segment::Italic("italic".into()));
        assert_eq!(segs[2], Segment::Normal(" text".into()));
    }

    #[test]
    fn parse_inline_bold_and_italic() {
        let segs = parse_inline("**bold** and _italic_");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment::Bold("bold".into()));
        assert_eq!(segs[1], Segment::Normal(" and ".into()));
        assert_eq!(segs[2], Segment::Italic("italic".into()));
    }

    #[test]
    fn parse_inline_bold() {
        let segs = parse_inline("this is **bold** text");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[1], Segment::Bold("bold".into()));
    }

    #[test]
    fn parse_inline_link() {
        let segs = parse_inline("click [here](https://example.com) now");
        assert_eq!(segs.len(), 3);
        assert_eq!(
            segs[1],
            Segment::Link {
                text: "here".into(),
                url: "https://example.com".into(),
            }
        );
    }

    #[test]
    fn detect_heading() {
        assert!(matches!(detect_line_type("# Title"), LineType::Heading(1)));
        assert!(matches!(detect_line_type("## Sub"), LineType::Heading(2)));
        assert!(matches!(detect_line_type("### H3"), LineType::Heading(3)));
    }

    #[test]
    fn detect_code_fence() {
        if let LineType::CodeFence { lang } = detect_line_type("```rust") {
            assert_eq!(lang, "rust");
        } else {
            panic!("expected CodeFence");
        }
    }

    #[test]
    fn detect_list_item() {
        assert!(matches!(detect_line_type("- item"), LineType::ListItem));
        assert!(matches!(detect_line_type("1. item"), LineType::ListItem));
    }

    #[test]
    fn detect_horizontal_rule() {
        assert!(matches!(detect_line_type("---"), LineType::HorizontalRule));
        assert!(matches!(detect_line_type("***"), LineType::HorizontalRule));
    }

    #[test]
    fn detect_normal() {
        assert!(matches!(detect_line_type("just text"), LineType::Normal));
    }

    #[test]
    fn heading_text_extraction() {
        assert_eq!(heading_text("### Hello", 3), "Hello");
        assert_eq!(heading_text("# Title", 1), "Title");
    }
}
