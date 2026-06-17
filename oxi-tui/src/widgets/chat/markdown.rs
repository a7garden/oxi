//! Markdown rendering, CJK-aware wrapping, and code block extraction.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::table_renderer::render_markdown_table;
use crate::theme::ThemeStyles;
use crate::widgets::chat::highlight::highlight_code;

// ── Code block extraction ────────────────────────────────────────────

pub(crate) fn extract_last_code_block(text: &str) -> Option<String> {
    let mut result: Option<String> = None;
    let mut in_block = false;
    let mut block_content = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_block {
                let c = block_content.trim().to_string();
                if !c.is_empty() {
                    result = Some(c);
                }
                block_content.clear();
                in_block = false;
            } else {
                block_content.clear();
                in_block = true;
            }
        } else if in_block {
            if !block_content.is_empty() {
                block_content.push('\n');
            }
            block_content.push_str(line);
        }
    }
    result
}

/// Fix bare code fences (``` without a language) to ```text.
/// Tracks open/close state so closing fences are left as ```.
pub(crate) fn fix_bare_code_fences(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_code = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                // Closing fence — emit as-is
                result.push_str("```");
                in_code = false;
            } else {
                // Opening fence
                let lang = trimmed.strip_prefix("```").unwrap_or(trimmed).trim();
                if lang.is_empty() {
                    result.push_str("```text");
                } else {
                    result.push_str(trimmed);
                }
                in_code = true;
            }
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    // Remove trailing newline if original didn't have one
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Parse markdown, extract tables, and render to styled Lines.
/// Tables are rendered using pulldown-cmark with width-aware column sizing.
/// `width` limits table width to prevent overflow.
pub(crate) fn md_lines(content: &str, width: u16, styles: &ThemeStyles) -> Vec<Line<'static>> {
    // Try table rendering first (pulldown-cmark based)
    let table_lines = render_markdown_table(content, width);
    if !table_lines.is_empty() {
        // render_markdown_table handles table cell wrapping internally,
        // but non-table text (before/after the table) comes from
        // flush_text → tui_markdown and may exceed the width.
        // Apply wrapping to the entire result to catch those cases.
        return wrap_lines_styled(&table_lines, width);
    }

    // No table found, use regular markdown rendering.
    // Apply CJK-aware wrapping before returning so that the layout
    // engine gets correctly-sized lines. Without this, ratatui's
    // Paragraph::wrap would be used at render time, but it only
    // breaks at whitespace — CJK characters that lack spaces between
    // them would be treated as a single giant "word" and overflow.
    let raw_lines = render_markdown(content, styles);
    wrap_lines_styled(&raw_lines, width)
}

/// Wrap styled `Line`s to fit within `width` terminal columns,
/// breaking at word boundaries for Latin text and at character
/// boundaries for CJK text. Preserves per-Span styling.
///
/// This replaces `Paragraph::wrap` for markdown content because
/// ratatui's `WordWrapper` does not handle CJK line-breaking —
/// Korean/Chinese/Japanese characters that lack spaces between
/// them are treated as a single "word" and never get wrapped.
pub(crate) fn wrap_lines_styled(lines: &[Line<'static>], width: u16) -> Vec<Line<'static>> {
    let max_w = width as usize;
    if max_w == 0 {
        return lines.to_vec();
    }

    let mut result = Vec::new();
    for line in lines {
        // Collect all (char, Style) pairs from all Spans
        let mut chars: Vec<(char, Style)> = Vec::new();
        for span in &line.spans {
            for ch in span.content.chars() {
                chars.push((ch, span.style));
            }
        }

        // Measure total width
        let total_w: usize = chars
            .iter()
            .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
            .sum();
        if total_w <= max_w {
            result.push(line.clone());
            continue;
        }

        // Break into wrapped lines
        let wrapped = wrap_styled_chars(&chars, max_w);
        result.extend(wrapped);
    }
    result
}

/// Wrap a flat list of (char, Style) into `Line`s that fit `max_width`.
///
/// Uses a word-boundary approach: groups consecutive chars into "words"
/// (separated by whitespace) and fits as many words as possible per line.
/// For CJK characters (which can break between any two characters),
/// each character is its own "word".
fn wrap_styled_chars(chars: &[(char, Style)], max_width: usize) -> Vec<Line<'static>> {
    // Segment chars into "tokens": each is either a whitespace run,
    // a CJK character, or a non-CJK word (consecutive non-ws non-CJK).
    #[derive(Debug)]
    enum Token<'a> {
        Word(&'a [(char, Style)]),
        Space(&'a [(char, Style)]),
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (ch, _) = chars[i];
        if ch.is_whitespace() {
            let start = i;
            while i < chars.len() && chars[i].0.is_whitespace() {
                i += 1;
            }
            tokens.push(Token::Space(&chars[start..i]));
        } else if is_cjk_breakable(ch) {
            // Each CJK character is its own token
            tokens.push(Token::Word(&chars[i..i + 1]));
            i += 1;
        } else {
            // Non-CJK word: collect until whitespace or CJK
            let start = i;
            while i < chars.len() {
                let (c, _) = chars[i];
                if c.is_whitespace() || is_cjk_breakable(c) {
                    break;
                }
                i += 1;
            }
            tokens.push(Token::Word(&chars[start..i]));
        }
    }

    // Build lines from tokens
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;
    let mut pending_space: Option<&[(char, Style)]> = None;
    let mut pending_space_width: usize = 0;

    for token in &tokens {
        match token {
            Token::Space(space_chars) => {
                let w: usize = space_chars
                    .iter()
                    .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
                    .sum();
                pending_space = Some(space_chars);
                pending_space_width = w;
            }
            Token::Word(word_chars) => {
                let word_width: usize = word_chars
                    .iter()
                    .map(|(ch, _)| UnicodeWidthChar::width(*ch).unwrap_or(0))
                    .sum();

                // Can we fit pending_space + this word?
                let needed = pending_space_width + word_width;

                if current_width + needed <= max_width {
                    // Fits on current line
                    if let Some(space_chars) = pending_space.take() {
                        append_chars_to_spans(space_chars, &mut current_spans);
                        current_width += pending_space_width;
                    }
                    append_chars_to_spans(word_chars, &mut current_spans);
                    current_width += word_width;
                } else if word_width > max_width {
                    // Word is wider than max_width — break at char boundaries
                    // First, flush current line
                    if !current_spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    }
                    // Break the oversized word into fragments that each fit max_width.
                    // All non-last fragments become complete output lines;
                    // the last fragment stays in current_spans so the next token
                    // can potentially join it.
                    let broken = break_styled_word(word_chars, max_width);
                    let broken_len = broken.len();
                    for (idx, broken_spans) in broken.into_iter().enumerate() {
                        if idx < broken_len - 1 {
                            lines.push(Line::from(broken_spans));
                        } else {
                            current_spans = broken_spans;
                            current_width = spans_width(&current_spans);
                        }
                    }
                } else {
                    // Doesn't fit — start new line
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                    // Drop leading space
                    append_chars_to_spans(word_chars, &mut current_spans);
                    current_width = word_width;
                }
                pending_space = None;
                pending_space_width = 0;
            }
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    if lines.is_empty() {
        lines.push(Line::raw(""));
    }

    lines
}

/// Check if a character is CJK — these characters allow line breaks
/// between any two adjacent CJK characters.
fn is_cjk_breakable(ch: char) -> bool {
    matches!(ch,
        '\u{2E80}'..='\u{9FFF}'   | // CJK Unified, Kangxi, etc.
        '\u{A960}'..='\u{A97F}'   | // Hangul Jamo Extended-A
        '\u{AC00}'..='\u{D7AF}'   | // Hangul Syllables (Korean)
        '\u{D7B0}'..='\u{D7FF}'   | // Hangul Jamo Extended-B
        '\u{F900}'..='\u{FAFF}'   | // CJK Compatibility Ideographs
        '\u{FE30}'..='\u{FE4F}'   | // CJK Compatibility Forms
        '\u{FF65}'..='\u{FFDC}'   | // Halfwidth and Fullwidth Forms
        '\u{20000}'..='\u{2A6DF}' | // CJK Extension B
        '\u{2A700}'..='\u{2B73F}' | // CJK Extension C
        '\u{2B740}'..='\u{2B81F}' | // CJK Extension D
        '\u{2F800}'..='\u{2FA1F}'   // CJK Compat Supplement
    )
}

/// Append styled chars to spans, merging adjacent chars with the same style.
fn append_chars_to_spans(chars: &[(char, Style)], spans: &mut Vec<Span<'static>>) {
    for (ch, style) in chars {
        if let Some(last) = spans.last_mut()
            && last.style == *style
        {
            last.content.to_mut().push(*ch);
            continue;
        }
        spans.push(Span::styled(ch.to_string(), *style));
    }
}

/// Break an oversized styled word at character boundaries to fit `max_width`.
fn break_styled_word(chars: &[(char, Style)], max_width: usize) -> Vec<Vec<Span<'static>>> {
    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_w: usize = 0;

    for (ch, style) in chars {
        let cw = UnicodeWidthChar::width(*ch).unwrap_or(0);
        if current_w + cw > max_width && !current.is_empty() {
            result.push(std::mem::take(&mut current));
            current_w = 0;
        }
        if let Some(last) = current.last_mut() {
            if last.style == *style {
                last.content.to_mut().push(*ch);
            } else {
                current.push(Span::styled(ch.to_string(), *style));
            }
        } else {
            current.push(Span::styled(ch.to_string(), *style));
        }
        current_w += cw;
    }

    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Measure the total unicode display width of a set of Spans.
pub(crate) fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Render regular markdown (non-table content).
/// Detects fenced code blocks and applies syntax highlighting.
///
/// **Public** so callers outside the chat widget (e.g. the issues panel's
/// detail body) can reuse the same styling. Pass `Theme::to_styles()` to
/// match the active theme. Output is suitable for a `Paragraph::new(lines)`
/// with `Wrap { trim: false }`.
pub fn render_markdown(content: &str, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let preprocessed = fix_bare_code_fences(content);

    // Split into segments: code blocks vs inline markdown
    let mut segments: Vec<MarkdownSegment> = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut md_buf = String::new();

    for line in preprocessed.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                segments.push(MarkdownSegment::Code {
                    lang: std::mem::take(&mut code_lang),
                    content: std::mem::take(&mut code_buf),
                });
                in_code = false;
            } else {
                if !md_buf.is_empty() {
                    segments.push(MarkdownSegment::Markdown(std::mem::take(&mut md_buf)));
                }
                code_lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();
                in_code = true;
            }
        } else if in_code {
            if !code_buf.is_empty() {
                code_buf.push('\n');
            }
            code_buf.push_str(line);
        } else {
            if !md_buf.is_empty() {
                md_buf.push('\n');
            }
            md_buf.push_str(line);
        }
    }

    if in_code {
        segments.push(MarkdownSegment::Code {
            lang: code_lang,
            content: code_buf,
        });
    } else if !md_buf.is_empty() {
        segments.push(MarkdownSegment::Markdown(md_buf));
    }

    let mut lines = Vec::new();
    for seg in &segments {
        match seg {
            MarkdownSegment::Markdown(md) => {
                let text: ratatui::text::Text<'_> = tui_markdown::from_str_with_options(
                    md,
                    &tui_markdown::Options::new(crate::markdown_styles::OxiStyleSheet),
                );
                for l in text.lines {
                    let line_style = l.style;
                    let spans: Vec<Span<'static>> = l
                        .spans
                        .into_iter()
                        .map(|s| Span::styled(s.content.into_owned(), line_style.patch(s.style)))
                        .collect();
                    lines.push(Line::from(spans));
                }
            }
            MarkdownSegment::Code { lang, content } => {
                lines.extend(highlight_code(content, lang, styles));
            }
        }
    }
    lines
}

/// A segment of markdown — regular text or a fenced code block.
enum MarkdownSegment {
    Markdown(String),
    Code { lang: String, content: String },
}

/// Filter JSON tool call arrays from thinking text.
/// GLM-5.1 writes tool call plans as `[{\"function\":...}]` inside
/// reasoning_content. We detect standalone JSON array lines (starting
/// with `[{\"` and ending with `]`) and remove them. Line-by-line
/// filtering avoids false positives from inline `[{\"` patterns in
/// normal reasoning text.
pub(crate) fn filter_tool_json(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            // Only remove lines that are standalone JSON tool call arrays.
            // GLM pattern: [{"function":...}] on its own line.
            // Inline occurrences like "see [{"key": "val"}] here" are preserved.
            !(trimmed.starts_with("[{\"") && trimmed.ends_with(']'))
        })
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
