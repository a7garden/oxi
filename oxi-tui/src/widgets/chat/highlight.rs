//! Code syntax highlighting — theme-aware token-based highlighter.
//!
//! Accepts a `&ThemeStyles` reference so colors adapt to the active theme
//! (dark, light, or custom). Falls back gracefully when a style is `Style::default()`.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::ThemeStyles;

/// Code token types for styling.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TokenType {
    Normal,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Punctuation,
}

/// Map token type to a ratatui Style using the active theme's semantic colors.
fn token_style(token: TokenType, styles: &ThemeStyles) -> Style {
    match token {
        TokenType::Normal => styles.normal,
        TokenType::Keyword => styles.accent.add_modifier(Modifier::BOLD),
        TokenType::String => styles.secondary,
        TokenType::Comment => styles.muted,
        TokenType::Number => styles.warning,
        TokenType::Type => styles.primary,
        TokenType::Function => styles.success,
        TokenType::Punctuation => styles.muted,
    }
}

/// Get keywords for a given language.
fn lang_keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" | "rs" => &[
            "fn", "let", "mut", "if", "else", "match", "loop", "while", "for", "in", "return",
            "break", "continue", "struct", "enum", "impl", "trait", "type", "pub", "mod", "use",
            "crate", "self", "super", "where", "async", "await", "move", "ref", "static", "const",
            "unsafe", "extern", "dyn", "as",
        ],
        "python" | "py" => &[
            "def", "class", "if", "elif", "else", "for", "while", "return", "import", "from", "as",
            "try", "except", "finally", "with", "yield", "lambda", "pass", "break", "continue",
            "and", "or", "not", "in", "is", "True", "False", "None", "self", "async", "await",
            "raise",
        ],
        "javascript" | "js" | "typescript" | "ts" | "tsx" | "jsx" => &[
            "function",
            "const",
            "let",
            "var",
            "if",
            "else",
            "for",
            "while",
            "do",
            "return",
            "class",
            "new",
            "this",
            "super",
            "import",
            "export",
            "from",
            "default",
            "async",
            "await",
            "try",
            "catch",
            "finally",
            "throw",
            "typeof",
            "instanceof",
            "void",
            "null",
            "undefined",
            "true",
            "false",
            "switch",
            "case",
            "break",
            "continue",
            "yield",
            "of",
            "in",
        ],
        "go" => &[
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "if",
            "else",
            "for",
            "range",
            "return",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "go",
            "defer",
            "select",
            "package",
            "import",
            "nil",
            "true",
            "false",
        ],
        "bash" | "sh" | "shell" | "zsh" => &[
            "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac",
            "function", "return", "local", "export", "source", "echo", "cd", "exit", "set",
            "unset", "readonly", "shift",
        ],
        "toml" | "yaml" | "yml" | "json" => &["true", "false", "null", "yes", "no"],
        _ => &[],
    }
}

/// Get line comment prefix for a language.
fn line_comment_prefix(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" | "rs" | "javascript" | "js" | "typescript" | "ts" | "tsx" | "jsx" | "go" | "c"
        | "cpp" | "java" | "swift" | "kotlin" => Some("//"),
        "python" | "py" | "bash" | "sh" | "shell" | "zsh" | "toml" => Some("#"),
        "sql" => Some("--"),
        _ => None,
    }
}

/// Check if a string is PascalCase (potential type name).
fn is_pascal_case(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
        && s.chars().any(|c| c.is_lowercase())
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Check if the text before a position ends with `.` (method call).
fn preceded_by_dot(text_before: &str) -> bool {
    text_before.trim_end().ends_with('.')
}

/// Highlight a single code line into styled Spans.
fn highlight_line(line: &str, lang: &str, styles: &ThemeStyles) -> Line<'static> {
    let keywords = lang_keywords(lang);
    let comment_prefix = line_comment_prefix(lang);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // ── Line comment ──
        if let Some(prefix) = comment_prefix
            && line[i..].starts_with(prefix)
        {
            let rest: String = chars[i..].iter().collect();
            spans.push(Span::styled(rest, token_style(TokenType::Comment, styles)));
            break;
        }

        // ── String literal (double-quoted) ──
        if chars[i] == '"' {
            let mut end = i + 1;
            while end < chars.len() {
                if chars[end] == '\\' && end + 1 < chars.len() {
                    end += 2;
                } else if chars[end] == '"' {
                    end += 1;
                    break;
                } else {
                    end += 1;
                }
            }
            let s: String = chars[i..end].iter().collect();
            spans.push(Span::styled(s, token_style(TokenType::String, styles)));
            i = end;
            continue;
        }

        // ── String literal (single-quoted) ──
        if chars[i] == '\'' {
            let mut end = i + 1;
            while end < chars.len() {
                if chars[end] == '\\' && end + 1 < chars.len() {
                    end += 2;
                } else if chars[end] == '\'' {
                    end += 1;
                    break;
                } else {
                    end += 1;
                }
            }
            let s: String = chars[i..end].iter().collect();
            spans.push(Span::styled(s, token_style(TokenType::String, styles)));
            i = end;
            continue;
        }

        // ── Number ──
        if chars[i].is_ascii_digit()
            || (chars[i] == '0'
                && i + 1 < chars.len()
                && (chars[i + 1] == 'x' || chars[i + 1] == 'b'))
        {
            let mut end = i;
            if chars[i] == '0' && end + 1 < chars.len() {
                let next = chars[end + 1];
                if next == 'x' || next == 'b' || next == 'o' {
                    end += 2;
                }
            }
            while end < chars.len()
                && (chars[end].is_ascii_hexdigit() || chars[end] == '.' || chars[end] == '_')
            {
                end += 1;
            }
            while end < chars.len() && chars[end].is_ascii_alphabetic() {
                end += 1;
            }
            let s: String = chars[i..end].iter().collect();
            spans.push(Span::styled(s, token_style(TokenType::Number, styles)));
            i = end;
            continue;
        }

        // ── Identifier / keyword ──
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut end = i;
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let word: String = chars[i..end].iter().collect();
            let before: String = chars[..i].iter().collect();

            let token_type = if keywords.contains(&word.as_str()) {
                TokenType::Keyword
            } else if is_pascal_case(&word) {
                TokenType::Type
            } else if preceded_by_dot(&before) {
                TokenType::Function
            } else {
                TokenType::Normal
            };

            spans.push(Span::styled(word, token_style(token_type, styles)));
            i = end;
            continue;
        }

        // ── Punctuation / operators ──
        let c = chars[i];
        let tok = if c == '('
            || c == ')'
            || c == '{'
            || c == '}'
            || c == '['
            || c == ']'
            || c == ':'
            || c == ';'
            || c == ','
            || c == '.'
        {
            TokenType::Punctuation
        } else {
            TokenType::Normal
        };
        spans.push(Span::styled(c.to_string(), token_style(tok, styles)));
        i += 1;
    }

    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    Line::from(spans)
}

/// Highlight a code block with language-aware syntax coloring.
///
/// Now accepts `&ThemeStyles` so that syntax colors adapt to the active
/// theme (dark, light, or custom) instead of always using dark defaults.
pub(crate) fn highlight_code(
    content: &str,
    lang: &str,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for line in content.lines() {
        lines.push(highlight_line(line, lang, styles));
    }
    lines
}
