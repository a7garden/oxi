//! Markdown component – renders markdown text with syntax highlighting.
//!
//! Parses a subset of CommonMark sufficient for AI chat responses:
//! headings, code blocks (fenced + inline), bold, italic, strikethrough,
//! links, unordered/ordered lists, blockquotes, and thematic breaks.

use crate::cell::{Attributes, Cell, Color};
use crate::component::Component;
use crate::event::Event;
use crate::surface::{Rect, Surface};
use crate::Size;

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// Styling theme for rendered markdown.
#[derive(Debug, Clone)]
pub struct MarkdownTheme {
    // Heading colours (h1–h6, index 0–5).
    pub heading_colors: [Color; 6],
    pub heading_bold: [bool; 6],

    // Inline styles
    pub code_fg: Color,
    pub code_bg: Color,
    pub link_fg: Color,
    pub link_underline: bool,
    pub bold_color: Option<Color>,
    pub italic_color: Option<Color>,
    pub strikethrough_color: Option<Color>,

    // Block styles
    pub blockquote_fg: Color,
    pub blockquote_marker: char,
    pub list_marker_unordered: char,
    pub hr_char: char,
    pub hr_color: Color,

    // Syntax-highlighting colours for code blocks.
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_comment: Color,
    pub syntax_number: Color,
    pub syntax_type: Color,
    pub syntax_function: Color,
    pub syntax_punctuation: Color,
    pub syntax_default: Color,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        Self {
            heading_colors: [
                Color::Indexed(12), // bright cyan   – h1
                Color::Indexed(14), // bright yellow – h2
                Color::Indexed(10), // bright green  – h3
                Color::Indexed(13), // bright magenta – h4
                Color::Indexed(11), // bright blue   – h5
                Color::Indexed(9),  // bright red    – h6
            ],
            heading_bold: [true; 6],

            code_fg: Color::Indexed(15),
            code_bg: Color::Indexed(236),
            link_fg: Color::Indexed(14),
            link_underline: true,
            bold_color: Some(Color::Indexed(15)),
            italic_color: None,
            strikethrough_color: None,

            blockquote_fg: Color::Indexed(242),
            blockquote_marker: '│',
            list_marker_unordered: '•',
            hr_char: '─',
            hr_color: Color::Indexed(242),

            syntax_keyword: Color::Indexed(13),
            syntax_string: Color::Indexed(10),
            syntax_comment: Color::Indexed(242),
            syntax_number: Color::Indexed(11),
            syntax_type: Color::Indexed(14),
            syntax_function: Color::Indexed(12),
            syntax_punctuation: Color::Indexed(246),
            syntax_default: Color::Indexed(252),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsed nodes
// ---------------------------------------------------------------------------

/// A single rendered cell with its style.
#[derive(Debug, Clone)]
struct StyledChar {
    ch: char,
    fg: Color,
    bg: Color,
    attrs: Attributes,
}

impl Default for StyledChar {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attributes::default(),
        }
    }
}

/// A horizontal run of styled characters occupying exactly one display row.
#[derive(Debug, Clone)]
struct StyledLine {
    chars: Vec<StyledChar>,
}

impl StyledLine {
    fn new() -> Self {
        Self { chars: Vec::new() }
    }

    fn push(&mut self, ch: char, fg: Color, bg: Color, attrs: Attributes) {
        self.chars.push(StyledChar { ch, fg, bg, attrs });
    }

    fn push_str(&mut self, s: &str, fg: Color, bg: Color, attrs: Attributes) {
        for c in s.chars() {
            self.push(c, fg, bg, attrs);
        }
    }

    fn len(&self) -> usize {
        self.chars.len()
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Inline scanner – returns a list of styled runs for a single text segment.
// ---------------------------------------------------------------------------

/// A contiguous run of identically-styled characters.
#[derive(Debug, Clone)]
struct InlineRun {
    text: String,
    fg: Color,
    bg: Color,
    attrs: Attributes,
}

/// Token produced by the block-level scanner.
enum Block {
    Heading {
        level: usize, // 1–6
        text: String,
    },
    Paragraph {
        text: String,
    },
    CodeBlock {
        language: String,
        code: String,
    },
    Blockquote {
        text: String,
    },
    UnorderedList {
        items: Vec<String>,
    },
    OrderedList {
        start: usize,
        items: Vec<String>,
    },
    HorizontalRule,
}

// ---------------------------------------------------------------------------
// Block-level parser
// ---------------------------------------------------------------------------

fn parse_blocks(input: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut lines = input.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();

        // ── Blank line ────────────────────────────────────────────
        if trimmed.is_empty() {
            continue;
        }

        // ── Thematic break ────────────────────────────────────────
        if is_thematic_break(trimmed) {
            blocks.push(Block::HorizontalRule);
            continue;
        }

        // ── ATX heading (# …) ────────────────────────────────────
        if let Some((level, text)) = parse_atx_heading(trimmed) {
            blocks.push(Block::Heading { level, text });
            continue;
        }

        // ── Fenced code block ─────────────────────────────────────
        if trimmed.starts_with("```") {
            let lang = trimmed.trim_start_matches('`').trim().to_string();
            let mut code_lines: Vec<String> = Vec::new();
            while let Some(cl) = lines.next() {
                if cl.trim().starts_with("```") {
                    break;
                }
                code_lines.push(cl.to_string());
            }
            blocks.push(Block::CodeBlock {
                language: lang,
                code: code_lines.join("\n"),
            });
            continue;
        }

        // ── Blockquote ────────────────────────────────────────────
        if trimmed.starts_with('>') {
            let mut quote_lines: Vec<String> = Vec::new();
            let text = trimmed.trim_start_matches('>').trim_start();
            quote_lines.push(text.to_string());
            while let Some(next) = lines.peek() {
                let nt = next.trim_start();
                if nt.starts_with('>') {
                    let t = nt.trim_start_matches('>').trim_start();
                    quote_lines.push(t.to_string());
                    lines.next();
                } else if nt.is_empty() {
                    lines.next();
                    break;
                } else {
                    break;
                }
            }
            blocks.push(Block::Blockquote {
                text: quote_lines.join("\n"),
            });
            continue;
        }

        // ── Unordered list ────────────────────────────────────────
        if is_unordered_list_item(trimmed) {
            let mut items: Vec<String> = Vec::new();
            items.push(strip_list_marker(trimmed).to_string());
            while let Some(next) = lines.peek() {
                let nt = next.trim_start();
                if nt.is_empty() {
                    break;
                }
                if is_unordered_list_item(nt) {
                    items.push(strip_list_marker(nt).to_string());
                    lines.next();
                } else if !items.is_empty() && (nt.starts_with(' ') || nt.starts_with('\t')) {
                    // continuation line
                    if let Some(last) = items.last_mut() {
                        last.push(' ');
                        last.push_str(nt.trim());
                    }
                    lines.next();
                } else {
                    break;
                }
            }
            blocks.push(Block::UnorderedList { items });
            continue;
        }

        // ── Ordered list ──────────────────────────────────────────
        if is_ordered_list_item(trimmed) {
            let start = parse_ordered_start(trimmed).unwrap_or(1);
            let mut items: Vec<String> = Vec::new();
            items.push(strip_ordered_marker(trimmed).to_string());
            while let Some(next) = lines.peek() {
                let nt = next.trim_start();
                if nt.is_empty() {
                    break;
                }
                if is_ordered_list_item(nt) {
                    items.push(strip_ordered_marker(nt).to_string());
                    lines.next();
                } else if !items.is_empty() && (nt.starts_with(' ') || nt.starts_with('\t')) {
                    if let Some(last) = items.last_mut() {
                        last.push(' ');
                        last.push_str(nt.trim());
                    }
                    lines.next();
                } else {
                    break;
                }
            }
            blocks.push(Block::OrderedList { start, items });
            continue;
        }

        // ── Paragraph (accumulate consecutive non-special lines) ──
        let mut para_lines: Vec<String> = vec![trimmed.to_string()];
        while let Some(next) = lines.peek() {
            let nt = next.trim_start();
            if nt.is_empty()
                || nt.starts_with('#')
                || nt.starts_with("```")
                || nt.starts_with('>')
                || is_thematic_break(nt)
                || is_unordered_list_item(nt)
                || is_ordered_list_item(nt)
            {
                break;
            }
            para_lines.push(nt.to_string());
            lines.next();
        }
        blocks.push(Block::Paragraph {
            text: para_lines.join(" "),
        });
    }

    blocks
}

// ---------------------------------------------------------------------------
// Block-level helpers
// ---------------------------------------------------------------------------

fn is_thematic_break(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 3 {
        return false;
    }
    let first = s.chars().next().unwrap();
    if first != '-' && first != '*' && first != '_' {
        return false;
    }
    s.chars().all(|c| c == first || c == ' ')
}

fn parse_atx_heading(s: &str) -> Option<(usize, String)> {
    let level = s.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &s[level..];
    // Remove optional trailing ###
    let text = rest.trim();
    let text = text.trim_end_matches('#').trim();
    Some((level, text.to_string()))
}

fn is_unordered_list_item(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    if first == '-' || first == '*' || first == '+' {
        let rest = &s[1..];
        rest.starts_with(' ') || rest.is_empty()
    } else {
        false
    }
}

fn strip_list_marker(s: &str) -> &str {
    if s.is_empty() {
        return s;
    }
    let first = s.chars().next().unwrap();
    if first == '-' || first == '*' || first == '+' {
        let rest = &s[first.len_utf8()..];
        rest.trim_start()
    } else {
        s
    }
}

fn is_ordered_list_item(s: &str) -> bool {
    let digits_end = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_end == 0 {
        return false;
    }
    let rest = &s[digits_end..];
    rest.starts_with('.') || rest.starts_with(')')
}

fn parse_ordered_start(s: &str) -> Option<usize> {
    let digits_end = s.chars().take_while(|c| c.is_ascii_digit()).count();
    s[..digits_end].parse().ok()
}

fn strip_ordered_marker(s: &str) -> &str {
    let digits_end = s.chars().take_while(|c| c.is_ascii_digit()).count();
    let rest = &s[digits_end..];
    // skip . or ) and following spaces
    rest.trim_start_matches(|c: char| c == '.' || c == ')').trim_start()
}

// ---------------------------------------------------------------------------
// Inline parser – handles **bold**, *italic*, ~~strike~~, `code`, [links](url)
// ---------------------------------------------------------------------------

fn parse_inline_runs(text: &str, base_fg: Color, base_bg: Color, base_attrs: Attributes, theme: &MarkdownTheme) -> Vec<InlineRun> {
    let mut runs: Vec<InlineRun> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    let push_run = |runs: &mut Vec<InlineRun>, content: &str, fg: Color, bg: Color, attrs: Attributes| {
        if !content.is_empty() {
            runs.push(InlineRun { text: content.to_string(), fg, bg, attrs });
        }
    };

    while i < len {
        // ── Inline code ───────────────────────────────────────────
        if chars[i] == '`' {
            let start = i;
            i += 1;
            // Count backticks
            let fence_len: usize = chars[i..].iter().take_while(|c| **c == '`').count();
            let total_fence = fence_len + 1;
            i += fence_len;
            // Find closing fence
            let code_start = i;
            let mut found = false;
            while i + total_fence <= len {
                if chars[i..].iter().take(total_fence).all(|c| *c == '`') {
                    found = true;
                    break;
                }
                i += 1;
            }
            if found {
                let code_text: String = chars[code_start..i].iter().collect();
                push_run(&mut runs, &code_text, theme.code_fg, theme.code_bg, Attributes::default());
                i += total_fence;
            } else {
                // No closing backtick – treat as literal
                i = code_start;
                push_run(&mut runs, &text[start..start+1], base_fg, base_bg, base_attrs);
            }
            continue;
        }

        // ── Bold (**text** or __text__) ───────────────────────────
        if i + 1 < len && ((chars[i] == '*' && chars[i + 1] == '*') || (chars[i] == '_' && chars[i + 1] == '_')) {
            let marker = chars[i];
            let close: String = std::iter::repeat(marker).take(2).collect();
            let search_from = i + 2;
            if let Some(end) = find_closing_marker(&chars, search_from, &close) {
                let bold_text: String = chars[i + 2..end].iter().collect();
                let mut attrs = base_attrs;
                attrs.bold = true;
                let fg = theme.bold_color.unwrap_or(base_fg);
                push_run(&mut runs, &bold_text, fg, base_bg, attrs);
                i = end + 2;
                continue;
            }
        }

        // ── Strikethrough (~~text~~) ──────────────────────────────
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            let search_from = i + 2;
            if let Some(end) = find_closing_marker(&chars, search_from, "~~") {
                let text: String = chars[i + 2..end].iter().collect();
                let mut attrs = base_attrs;
                attrs.strikethrough = true;
                let fg = theme.strikethrough_color.unwrap_or(base_fg);
                push_run(&mut runs, &text, fg, base_bg, attrs);
                i = end + 2;
                continue;
            }
        }

        // ── Italic (*text* or _text_) ─────────────────────────────
        if (chars[i] == '*' || chars[i] == '_')
            && (i + 1 < len && chars[i + 1] != chars[i])
        {
            let marker = chars[i];
            let search_from = i + 1;
            if let Some(end) = find_single_closing(&chars, search_from, marker) {
                let text: String = chars[i + 1..end].iter().collect();
                let mut attrs = base_attrs;
                attrs.italic = true;
                let fg = theme.italic_color.unwrap_or(base_fg);
                push_run(&mut runs, &text, fg, base_bg, attrs);
                i = end + 1;
                continue;
            }
        }

        // ── Link [text](url) ──────────────────────────────────────
        if chars[i] == '[' {
            if let Some((link_text, url, end_pos)) = parse_link(&chars, i) {
                push_run(&mut runs, &link_text, theme.link_fg, base_bg, {
                    let mut a = base_attrs;
                    a.underline = theme.link_underline;
                    a
                });
                // Don't show the URL in terminal rendering – the underlined text is enough.
                // If we wanted to show it, we'd push a second run here.
                let _ = url; // url available for future use
                i = end_pos;
                continue;
            }
        }

        // ── Regular character ─────────────────────────────────────
        let mut buf = String::new();
        buf.push(chars[i]);
        i += 1;
        while i < len {
            let c = chars[i];
            if c == '`' || c == '[' || c == '*' || c == '_' || c == '~' {
                break;
            }
            buf.push(c);
            i += 1;
        }
        push_run(&mut runs, &buf, base_fg, base_bg, base_attrs);
    }

    runs
}

fn find_closing_marker(chars: &[char], from: usize, marker: &str) -> Option<usize> {
    let marker_chars: Vec<char> = marker.chars().collect();
    let mlen = marker_chars.len();
    let mut i = from;
    while i + mlen <= chars.len() {
        if chars[i..i + mlen] == marker_chars[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_single_closing(chars: &[char], from: usize, marker: char) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == marker {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    // [text](url)
    let mut i = start + 1; // skip '['
    let mut text = String::new();
    // Find closing ']'
    while i < chars.len() && chars[i] != ']' {
        text.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ']' {
        return None;
    }
    i += 1; // skip ']'
    if i >= chars.len() || chars[i] != '(' {
        return None;
    }
    i += 1; // skip '('
    let mut url = String::new();
    while i < chars.len() && chars[i] != ')' {
        url.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ')' {
        return None;
    }
    i += 1; // skip ')'
    Some((text, url, i))
}

// ---------------------------------------------------------------------------
// Syntax highlighting (simple keyword-based)
// ---------------------------------------------------------------------------

/// Token types for syntax highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxToken {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Punctuation,
    Ident,
    Whitespace,
}

/// A span of source code tagged with a syntax category.
#[derive(Debug, Clone)]
struct SyntaxSpan {
    text: String,
    kind: SyntaxToken,
}

fn highlight_line(source: &str, lang: &str, theme: &MarkdownTheme) -> Vec<(char, Color)> {
    let spans = tokenize_syntax(source, lang);
    let mut result: Vec<(char, Color)> = Vec::with_capacity(source.len());
    for span in spans {
        let color = match span.kind {
            SyntaxToken::Keyword => theme.syntax_keyword,
            SyntaxToken::String => theme.syntax_string,
            SyntaxToken::Comment => theme.syntax_comment,
            SyntaxToken::Number => theme.syntax_number,
            SyntaxToken::Type => theme.syntax_type,
            SyntaxToken::Function => theme.syntax_function,
            SyntaxToken::Punctuation => theme.syntax_punctuation,
            SyntaxToken::Ident => theme.syntax_default,
            SyntaxToken::Whitespace => theme.syntax_default,
        };
        for c in span.text.chars() {
            result.push((c, color));
        }
    }
    result
}

/// Get the keyword set for a language.
fn keywords_for_lang(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" | "rs" => &[
            "fn", "let", "mut", "if", "else", "match", "return", "pub", "struct",
            "enum", "impl", "trait", "use", "mod", "crate", "self", "Self", "super",
            "where", "async", "await", "move", "dyn", "const", "static", "type",
            "for", "while", "loop", "break", "continue", "in", "ref", "true",
            "false", "as", "extern", "unsafe", "macro_rules",
        ],
        "python" | "py" => &[
            "def", "class", "if", "elif", "else", "return", "import", "from",
            "as", "with", "for", "while", "break", "continue", "pass", "raise",
            "try", "except", "finally", "yield", "lambda", "and", "or", "not",
            "is", "in", "True", "False", "None", "async", "await", "global",
            "nonlocal", "assert", "del",
        ],
        "javascript" | "js" | "ts" | "typescript" => &[
            "function", "const", "let", "var", "if", "else", "return", "for",
            "while", "do", "switch", "case", "break", "continue", "new", "this",
            "class", "extends", "import", "export", "from", "default", "async",
            "await", "try", "catch", "finally", "throw", "typeof", "instanceof",
            "in", "of", "true", "false", "null", "undefined", "void", "delete",
            "yield", "static", "get", "set", "implements", "interface", "type",
            "enum", "declare", "abstract", "as", "keyof", "readonly",
        ],
        "go" => &[
            "func", "var", "const", "type", "struct", "interface", "map", "chan",
            "if", "else", "for", "range", "switch", "case", "default", "return",
            "break", "continue", "go", "select", "defer", "fallthrough", "goto",
            "package", "import", "true", "false", "nil", "append", "make", "len",
            "cap", "new",
        ],
        "java" | "kotlin" | "kt" => &[
            "class", "interface", "enum", "object", "fun", "val", "var", "if",
            "else", "when", "for", "while", "do", "return", "break", "continue",
            "try", "catch", "finally", "throw", "import", "package", "public",
            "private", "protected", "internal", "abstract", "open", "sealed",
            "data", "object", "companion", "override", "suspend", "inline",
            "true", "false", "null", "this", "super", "new", "void", "static",
            "final", "extends", "implements", "throws",
        ],
        "ruby" | "rb" => &[
            "def", "end", "class", "module", "if", "else", "elsif", "unless",
            "while", "until", "for", "do", "begin", "rescue", "ensure", "raise",
            "return", "yield", "block_given?", "require", "include", "attr_reader",
            "attr_writer", "attr_accessor", "self", "true", "false", "nil",
            "and", "or", "not", "puts", "print", "new",
        ],
        "c" | "cpp" | "c++" | "h" => &[
            "int", "char", "float", "double", "void", "long", "short", "unsigned",
            "signed", "const", "static", "extern", "struct", "enum", "union",
            "typedef", "class", "namespace", "template", "typename", "public",
            "private", "protected", "virtual", "override", "if", "else", "for",
            "while", "do", "switch", "case", "break", "continue", "return",
            "new", "delete", "try", "catch", "throw", "true", "false", "nullptr",
            "auto", "sizeof", "include", "define",
        ],
        "sh" | "bash" | "zsh" | "shell" => &[
            "if", "then", "else", "elif", "fi", "for", "while", "do", "done",
            "case", "esac", "function", "return", "exit", "echo", "export",
            "local", "readonly", "set", "unset", "shift", "source", "alias",
            "true", "false", "in", "select", "until", "trap", "wait",
        ],
        _ => &[],
    }
}

/// Check if a character is a type-like identifier start (uppercase).
fn is_type_like(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_uppercase() => chars.all(|c| c.is_alphanumeric() || c == '_'),
        _ => false,
    }
}

/// Simple syntax tokenizer. Not a full parser – good enough for colouring.
fn tokenize_syntax(source: &str, lang: &str) -> Vec<SyntaxSpan> {
    let keywords = keywords_for_lang(lang);
    let mut spans: Vec<SyntaxSpan> = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // ── Line comment ──────────────────────────────────────────
        if (chars[i] == '/' && i + 1 < len && chars[i + 1] == '/')
            || (lang == "python" || lang == "py" || lang == "ruby" || lang == "rb" || lang == "sh" || lang == "bash")
                && chars[i] == '#'
        {
            let comment: String = chars[i..].iter().collect();
            spans.push(SyntaxSpan { text: comment, kind: SyntaxToken::Comment });
            break;
        }
        // Rust line comments
        if chars[i] == '/' && i + 1 < len && chars[i + 1] == '/' {
            let comment: String = chars[i..].iter().collect();
            spans.push(SyntaxSpan { text: comment, kind: SyntaxToken::Comment });
            break;
        }

        // ── String literal (double-quoted) ────────────────────────
        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            let mut s = String::new();
            s.push(quote);
            i += 1;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    s.push(chars[i]);
                    s.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                s.push(chars[i]);
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(SyntaxSpan { text: s, kind: SyntaxToken::String });
            continue;
        }

        // ── Number ────────────────────────────────────────────────
        if chars[i].is_ascii_digit() {
            let mut s = String::new();
            while i < len
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == 'x'
                    || ('a'..='f').contains(&chars[i])
                    || ('A'..='F').contains(&chars[i]))
            {
                s.push(chars[i]);
                i += 1;
            }
            spans.push(SyntaxSpan { text: s, kind: SyntaxToken::Number });
            continue;
        }

        // ── Identifier / keyword ──────────────────────────────────
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut s = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                s.push(chars[i]);
                i += 1;
            }

            // Check if next non-space char is '(' → function call
            let mut peek = i;
            while peek < len && chars[peek] == ' ' {
                peek += 1;
            }
            let is_call = peek < len && chars[peek] == '(';

            let kind = if keywords.contains(&s.as_str()) {
                SyntaxToken::Keyword
            } else if is_type_like(&s) && !is_call {
                SyntaxToken::Type
            } else if is_call {
                SyntaxToken::Function
            } else {
                SyntaxToken::Ident
            };

            spans.push(SyntaxSpan { text: s, kind });
            continue;
        }

        // ── Punctuation ───────────────────────────────────────────
        if "!@#$%^&*()-=+[]{}|;:',.<>?/\\~`".contains(chars[i]) {
            spans.push(SyntaxSpan {
                text: chars[i].to_string(),
                kind: SyntaxToken::Punctuation,
            });
            i += 1;
            continue;
        }

        // ── Whitespace / other ────────────────────────────────────
        spans.push(SyntaxSpan {
            text: chars[i].to_string(),
            kind: SyntaxToken::Whitespace,
        });
        i += 1;
    }

    spans
}

// ---------------------------------------------------------------------------
// Block → StyledLines rendering
// ---------------------------------------------------------------------------

/// Lay out parsed blocks into a list of `StyledLine`s suitable for display
/// at a given width. Word-wrapping is applied to paragraphs and list items.
fn render_blocks_to_lines(blocks: &[Block], max_width: u16, theme: &MarkdownTheme) -> Vec<StyledLine> {
    let w = max_width as usize;
    let mut lines: Vec<StyledLine> = Vec::new();

    for block in blocks {
        match block {
            Block::HorizontalRule => {
                let mut line = StyledLine::new();
                for _ in 0..w {
                    line.push(theme.hr_char, theme.hr_color, Color::Default, Attributes::default());
                }
                lines.push(line);
                lines.push(StyledLine::new()); // blank after
            }

            Block::Heading { level, text } => {
                let idx = (level - 1).min(5);
                let fg = theme.heading_colors[idx];
                let mut attrs = Attributes::default();
                attrs.bold = theme.heading_bold[idx];
                // Render heading with underline for level 1–2
                let wrapped = word_wrap(text, w);
                for row in &wrapped {
                    let mut line = StyledLine::new();
                    for c in row.chars() {
                        line.push(c, fg, Color::Default, attrs);
                    }
                    lines.push(line);
                }
                // Underline for h1 and h2
                if *level <= 2 {
                    let ch = if *level == 1 { '=' } else { '-' };
                    let mut line = StyledLine::new();
                    let heading_len = wrapped.iter().map(|r| r.len()).max().unwrap_or(0);
                    for _ in 0..heading_len.min(w) {
                        line.push(ch, fg, Color::Default, Attributes::default());
                    }
                    lines.push(line);
                }
                lines.push(StyledLine::new()); // blank after
            }

            Block::Paragraph { text } => {
                let runs = parse_inline_runs(text, Color::Default, Color::Default, Attributes::default(), theme);
                let row_lines = render_inline_runs_wrapped(&runs, w);
                for rl in row_lines {
                    lines.push(rl);
                }
                lines.push(StyledLine::new()); // blank after
            }

            Block::CodeBlock { language, code } => {
                let lang = language.to_lowercase();
                let mut header = StyledLine::new();
                // Render language label
                if !language.is_empty() {
                    let label = format!(" {} ", language);
                    for c in label.chars() {
                        header.push(c, theme.syntax_comment, theme.code_bg, Attributes::default());
                    }
                    // Fill remainder
                    while header.len() < w {
                        header.push(' ', theme.syntax_default, theme.code_bg, Attributes::default());
                    }
                    lines.push(header);
                }

                // Render code lines
                for code_line in code.lines() {
                    let mut line = StyledLine::new();
                    let highlighted = highlight_line(code_line, &lang, theme);
                    for (ch, color) in highlighted {
                        if line.len() < w {
                            line.push(ch, color, theme.code_bg, Attributes::default());
                        }
                    }
                    // Fill background
                    while line.len() < w {
                        line.push(' ', theme.syntax_default, theme.code_bg, Attributes::default());
                    }
                    lines.push(line);
                }

                // Closing line (empty, with bg)
                let mut close = StyledLine::new();
                while close.len() < w {
                    close.push(' ', theme.syntax_default, theme.code_bg, Attributes::default());
                }
                lines.push(close);
                lines.push(StyledLine::new()); // blank after
            }

            Block::Blockquote { text } => {
                for q_line in text.lines() {
                    let runs = parse_inline_runs(
                        q_line,
                        theme.blockquote_fg,
                        Color::Default,
                        Attributes::default(),
                        theme,
                    );
                    let mut line = StyledLine::new();
                    line.push(theme.blockquote_marker, theme.blockquote_fg, Color::Default, Attributes::default());
                    line.push(' ', theme.blockquote_fg, Color::Default, Attributes::default());
                    for run in &runs {
                        for c in run.text.chars() {
                            line.push(c, run.fg, run.bg, run.attrs);
                        }
                    }
                    lines.push(line);
                }
                lines.push(StyledLine::new()); // blank after
            }

            Block::UnorderedList { items } => {
                for item in items {
                    let marker = format!("{} ", theme.list_marker_unordered);
                    let runs = parse_inline_runs(
                        item,
                        Color::Default,
                        Color::Default,
                        Attributes::default(),
                        theme,
                    );
                    let row_lines = render_inline_runs_wrapped(&runs, w.saturating_sub(2));
                    let mut first = true;
                    for rl in row_lines {
                        let mut line = StyledLine::new();
                        if first {
                            line.push_str(&marker, theme.blockquote_fg, Color::Default, Attributes::default());
                            first = false;
                        } else {
                            line.push_str("  ", Color::Default, Color::Default, Attributes::default());
                        }
                        // Append the wrapped run line
                        line.chars.extend(rl.chars);
                        lines.push(line);
                    }
                }
                lines.push(StyledLine::new()); // blank after
            }

            Block::OrderedList { start, items } => {
                for (idx, item) in items.iter().enumerate() {
                    let num = start + idx;
                    let marker = format!("{}. ", num);
                    let runs = parse_inline_runs(
                        item,
                        Color::Default,
                        Color::Default,
                        Attributes::default(),
                        theme,
                    );
                    let marker_width = marker.len();
                    let row_lines = render_inline_runs_wrapped(&runs, w.saturating_sub(marker_width));
                    let mut first = true;
                    for rl in row_lines {
                        let mut line = StyledLine::new();
                        if first {
                            line.push_str(&marker, theme.blockquote_fg, Color::Default, Attributes::default());
                            first = false;
                        } else {
                            let pad = marker_width;
                            for _ in 0..pad {
                                line.push(' ', Color::Default, Color::Default, Attributes::default());
                            }
                        }
                        line.chars.extend(rl.chars);
                        lines.push(line);
                    }
                }
                lines.push(StyledLine::new()); // blank after
            }
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Inline-run word-wrap helper
// ---------------------------------------------------------------------------

/// Take a list of `InlineRun`s and produce one `StyledLine` per display row.
fn render_inline_runs_wrapped(runs: &[InlineRun], max_width: usize) -> Vec<StyledLine> {
    if max_width == 0 {
        return Vec::new();
    }

    // First, flatten all runs into (char, fg, bg, attrs) with explicit space splitting.
    let mut words: Vec<Vec<StyledChar>> = Vec::new();
    let mut current_word: Vec<StyledChar> = Vec::new();

    for run in runs {
        for c in run.text.chars() {
            if c == ' ' {
                if !current_word.is_empty() {
                    words.push(std::mem::take(&mut current_word));
                }
                // Represent spaces as their own "word" to preserve them
                current_word.push(StyledChar {
                    ch: ' ',
                    fg: run.fg,
                    bg: run.bg,
                    attrs: run.attrs,
                });
                words.push(std::mem::take(&mut current_word));
            } else {
                current_word.push(StyledChar {
                    ch: c,
                    fg: run.fg,
                    bg: run.bg,
                    attrs: run.attrs,
                });
            }
        }
    }
    if !current_word.is_empty() {
        words.push(current_word);
    }

    let mut result_lines: Vec<StyledLine> = Vec::new();
    let mut current_line = StyledLine::new();

    for word in &words {
        // Is this a space word?
        let is_space = word.len() == 1 && word[0].ch == ' ';

        if is_space {
            // Add space if there's room and we're not at line start
            if !current_line.is_empty() && current_line.len() + 1 <= max_width {
                current_line.chars.push(word[0].clone());
            }
            continue;
        }

        // Does the word fit on the current line?
        if current_line.len() + word.len() <= max_width {
            current_line.chars.extend(word.iter().cloned());
        } else {
            // Start a new line
            if !current_line.is_empty() {
                result_lines.push(current_line);
            }
            current_line = StyledLine::new();
            // If the word itself is longer than max_width, just put it anyway
            current_line.chars.extend(word.iter().cloned());
        }
    }

    if !current_line.is_empty() {
        result_lines.push(current_line);
    }

    // If there were no runs at all, return a single empty line
    if result_lines.is_empty() {
        result_lines.push(StyledLine::new());
    }

    result_lines
}

/// Simple word-wrap for a plain text string (used for headings).
fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut result: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.len() + 1 + word.len() <= max_width {
            current.push(' ');
            current.push_str(word);
        } else {
            result.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

// ---------------------------------------------------------------------------
// Markdown component
// ---------------------------------------------------------------------------

/// A Markdown rendering component.
///
/// Parses markdown text and renders it with colours and styles appropriate
/// for a terminal.
pub struct Markdown {
    content: String,
    theme: MarkdownTheme,
    /// Cached rendered lines (recomputed when content or width changes).
    styled_lines: Vec<StyledLine>,
    /// Width used for the cached layout.
    cached_width: u16,
    /// Scroll offset (number of rows scrolled up from the top).
    scroll_offset: u16,
    dirty: bool,
}

impl Markdown {
    /// Create a new Markdown component with default theme.
    pub fn new() -> Self {
        Self {
            content: String::new(),
            theme: MarkdownTheme::default(),
            styled_lines: Vec::new(),
            cached_width: 0,
            scroll_offset: 0,
            dirty: true,
        }
    }

    /// Create with a custom theme.
    pub fn with_theme(theme: MarkdownTheme) -> Self {
        Self {
            content: String::new(),
            theme,
            styled_lines: Vec::new(),
            cached_width: 0,
            scroll_offset: 0,
            dirty: true,
        }
    }

    /// Set the markdown content.
    pub fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.cached_width = 0; // invalidate cache
            self.dirty = true;
        }
    }

    /// Get the current content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the number of rendered lines.
    pub fn line_count(&self) -> usize {
        self.styled_lines.len()
    }

    /// Scroll up by `n` rows.
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.dirty = true;
    }

    /// Scroll down by `n` rows.
    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.dirty = true;
    }

    /// Scroll to the bottom of the content.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// Ensure the layout is up to date for the given width.
    fn ensure_layout(&mut self, width: u16) {
        if width != self.cached_width {
            let blocks = parse_blocks(&self.content);
            self.styled_lines = render_blocks_to_lines(&blocks, width, &self.theme);
            self.cached_width = width;
        }
    }
}

impl Default for Markdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Markdown {
    fn name(&self) -> &str {
        "Markdown"
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if let Event::Key(key) = event {
            match key.code {
                crate::KeyCode::Up => {
                    self.scroll_up(1);
                    true
                }
                crate::KeyCode::Down => {
                    self.scroll_down(1);
                    true
                }
                crate::KeyCode::PageUp => {
                    self.scroll_up(10);
                    true
                }
                crate::KeyCode::PageDown => {
                    self.scroll_down(10);
                    true
                }
                _ => false,
            }
        } else if let Event::Mouse(mouse) = event {
            match mouse.kind {
                crate::event::MouseEventKind::ScrollUp => {
                    self.scroll_up(3);
                    true
                }
                crate::event::MouseEventKind::ScrollDown => {
                    self.scroll_down(3);
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        let width = area.width;
        self.ensure_layout(width);

        let total_lines = self.styled_lines.len();
        let visible_height = area.height as usize;

        // scroll_offset is from the bottom; compute the start row
        let max_scroll = total_lines.saturating_sub(visible_height);
        let start_row = max_scroll.saturating_sub(self.scroll_offset as usize);

        for row in 0..visible_height {
            let line_idx = start_row + row;
            let y = area.y + row as u16;

            for col in 0..width as usize {
                let x = area.x + col as u16;
                let cell = if line_idx < total_lines {
                    let line = &self.styled_lines[line_idx];
                    if col < line.chars.len() {
                        let sc = &line.chars[col];
                        Cell {
                            char: sc.ch,
                            fg: sc.fg,
                            bg: sc.bg,
                            attrs: sc.attrs,
                        }
                    } else {
                        Cell::default()
                    }
                } else {
                    Cell::default()
                };
                surface.set(y, x, cell);
            }
        }
    }

    fn min_size(&self) -> Size {
        Size {
            width: 10,
            height: 1,
        }
    }

    fn desired_size(&self) -> Option<Size> {
        Some(Size {
            width: 80,
            height: 24,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_parsing() {
        let blocks = parse_blocks("# Hello\n## World\n### Foo");
        assert_eq!(blocks.len(), 3);
        match &blocks[0] {
            Block::Heading { level, text } => {
                assert_eq!(*level, 1);
                assert_eq!(text, "Hello");
            }
            _ => panic!("Expected heading"),
        }
        match &blocks[1] {
            Block::Heading { level, text } => {
                assert_eq!(*level, 2);
                assert_eq!(text, "World");
            }
            _ => panic!("Expected heading"),
        }
    }

    #[test]
    fn test_code_block_parsing() {
        let input = "```rust\nfn main() {}\n```";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::CodeBlock { language, code } => {
                assert_eq!(language, "rust");
                assert_eq!(code, "fn main() {}");
            }
            _ => panic!("Expected code block"),
        }
    }

    #[test]
    fn test_unordered_list() {
        let input = "- item 1\n- item 2\n- item 3";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::UnorderedList { items } => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], "item 1");
            }
            _ => panic!("Expected unordered list"),
        }
    }

    #[test]
    fn test_ordered_list() {
        let input = "1. first\n2. second\n3. third";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::OrderedList { start, items } => {
                assert_eq!(*start, 1);
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], "first");
            }
            _ => panic!("Expected ordered list"),
        }
    }

    #[test]
    fn test_blockquote() {
        let input = "> line 1\n> line 2";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Blockquote { text } => {
                assert!(text.contains("line 1"));
                assert!(text.contains("line 2"));
            }
            _ => panic!("Expected blockquote"),
        }
    }

    #[test]
    fn test_horizontal_rule() {
        let input = "---\n***\n___";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 3);
        for block in &blocks {
            assert!(matches!(block, Block::HorizontalRule));
        }
    }

    #[test]
    fn test_thematic_break_not_dash_in_word() {
        // "---" alone is a rule, but "hello---world" in a paragraph is not
        let input = "hello---world";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], Block::Paragraph { .. }));
    }

    #[test]
    fn test_inline_bold() {
        let theme = MarkdownTheme::default();
        let runs = parse_inline_runs("**bold text**", Color::Default, Color::Default, Attributes::default(), &theme);
        // Should have one bold run
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "bold text");
        assert!(runs[0].attrs.bold);
    }

    #[test]
    fn test_inline_italic() {
        let theme = MarkdownTheme::default();
        let runs = parse_inline_runs("*italic text*", Color::Default, Color::Default, Attributes::default(), &theme);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "italic text");
        assert!(runs[0].attrs.italic);
    }

    #[test]
    fn test_inline_code() {
        let theme = MarkdownTheme::default();
        let runs = parse_inline_runs("`code here`", Color::Default, Color::Default, Attributes::default(), &theme);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "code here");
        assert_eq!(runs[0].fg, theme.code_fg);
        assert_eq!(runs[0].bg, theme.code_bg);
    }

    #[test]
    fn test_inline_link() {
        let theme = MarkdownTheme::default();
        let runs = parse_inline_runs("[click me](http://example.com)", Color::Default, Color::Default, Attributes::default(), &theme);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "click me");
        assert!(runs[0].attrs.underline);
        assert_eq!(runs[0].fg, theme.link_fg);
    }

    #[test]
    fn test_inline_mixed() {
        let theme = MarkdownTheme::default();
        let runs = parse_inline_runs("hello **bold** and `code` world", Color::Default, Color::Default, Attributes::default(), &theme);
        // "hello ", "**bold**", " and ", "`code`", " world"
        assert!(runs.len() >= 4);
    }

    #[test]
    fn test_syntax_highlighting_keywords() {
        let spans = tokenize_syntax("fn main() { let x = 1; }", "rust");
        let keyword_spans: Vec<_> = spans.iter().filter(|s| s.kind == SyntaxToken::Keyword).collect();
        assert!(!keyword_spans.is_empty());
        let texts: Vec<&str> = keyword_spans.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"fn"));
        assert!(texts.contains(&"let"));
    }

    #[test]
    fn test_syntax_highlighting_strings() {
        let spans = tokenize_syntax("let s = \"hello\";", "rust");
        let string_spans: Vec<_> = spans.iter().filter(|s| s.kind == SyntaxToken::String).collect();
        assert!(!string_spans.is_empty());
    }

    #[test]
    fn test_syntax_highlighting_comments() {
        let spans = tokenize_syntax("let x = 1; // comment", "rust");
        let comment_spans: Vec<_> = spans.iter().filter(|s| s.kind == SyntaxToken::Comment).collect();
        assert!(!comment_spans.is_empty());
    }

    #[test]
    fn test_render_paragraphs() {
        let mut md = Markdown::new();
        md.set_content("Hello **world**!\n\nThis is a test.");
        // Force layout
        md.ensure_layout(80);
        assert!(!md.styled_lines.is_empty());
    }

    #[test]
    fn test_render_code_block_with_highlighting() {
        let mut md = Markdown::new();
        md.set_content("```rust\nfn main() {\n    println!(\"hello\");\n}\n```");
        md.ensure_layout(80);
        assert!(md.styled_lines.len() > 3); // header + 3 code lines + close + blank
    }

    #[test]
    fn test_word_wrap() {
        let wrapped = word_wrap("hello world foo bar baz", 12);
        for line in &wrapped {
            assert!(line.len() <= 12);
        }
    }

    #[test]
    fn test_strikethrough() {
        let theme = MarkdownTheme::default();
        let runs = parse_inline_runs("~~deleted~~", Color::Default, Color::Default, Attributes::default(), &theme);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].attrs.strikethrough);
    }

    #[test]
    fn test_paragraph_continuation() {
        let input = "line one\nline two\nline three";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Paragraph { text } => {
                assert_eq!(text, "line one line two line three");
            }
            _ => panic!("Expected single paragraph"),
        }
    }

    #[test]
    fn test_multiline_blockquote() {
        let input = "> first line\n> second line\n> third line";
        let blocks = parse_blocks(input);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Blockquote { text } => {
                let lines: Vec<&str> = text.lines().collect();
                assert_eq!(lines.len(), 3);
            }
            _ => panic!("Expected blockquote"),
        }
    }
}
