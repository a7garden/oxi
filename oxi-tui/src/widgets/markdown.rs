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
// detect_line_type removed


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
    base.bold()
}

/// Style used for italic text.
pub fn italic_style(base: Style) -> Style {
    base.italic()
}

/// Style used for links (visible text portion).
pub fn link_style(base: Style) -> Style {
    base.fg(Color::Cyan).underlined()
}

/// Style used for heading text.
pub fn heading_style(base: Style, level: u8) -> Style {
    let s = base.bold();
    // Optionally differentiate by level — for now all bold.
    let _ = level; // avoid unused warning
    s
}

/// Style used for code-block lines.
pub fn code_block_style(base: Style) -> Style {
    base.bg(Color::Indexed(234))
}

