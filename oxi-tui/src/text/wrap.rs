//! Word-aware line wrapping with CJK support.
//!
//! Plain text and styled variants share the same algorithm: split the
//! input on `\n` for hard breaks, then greedily fill each paragraph
//! with words that fit within `width` terminal columns. CJK characters
//! (width 2 in `unicode-width`) become individual breakable tokens so
//! that unspaced Korean / Chinese / Japanese text wraps correctly.
//!
//! ## Algorithm
//!
//! For each hard-separated paragraph:
//!
//! 1. Segment the paragraph into `Whitespace` runs, `Cjk` singletons,
//!    and `Word` runs (consecutive non-whitespace, non-CJK chars).
//! 2. Walk tokens left-to-right:
//!    - If pending whitespace + this token fits the current line, append.
//!    - If the token alone is wider than `width`, break it at character
//!      boundaries until it fits, then continue.
//!    - Otherwise, flush the current line and start a new one with the
//!      token (pending whitespace is dropped on line wrap).
//! 3. Flush the final line.
//!
//! Empty input yields exactly one empty line so callers always get at
//! least one renderable row.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

/// Wrap a plain `&str` so each output `Line` fits within `width` columns.
///
/// Hard line breaks (`\n`) are preserved as paragraph boundaries. CJK
/// characters break between any two adjacent glyphs; ASCII words break
/// on whitespace. Words wider than `width` are split at character
/// boundaries.
///
/// An input of `""` returns a single empty `Line`. A `width` of `0`
/// returns the input split on `\n` without any column fitting — this
/// preserves degenerate configurations without panicking.
#[must_use]
pub fn wrap_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    wrap_paragraphs(text.split('\n'), width, Style::default(), false)
        .into_iter()
        .map(styled_to_unstyled)
        .collect()
}

/// Wrap a plain `&str` while applying `style` to every emitted span.
///
/// Each output `Line` contains a single `Span::styled(content, style)`.
/// Use this when feeding the wrap layer before per-token styling
/// (e.g. inline markdown decoration) has been resolved.
#[must_use]
pub fn wrap_lines_styled(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let styled = wrap_paragraphs(text.split('\n'), width, style, true);
    styled.into_iter().map(|p| Line::from(p.spans)).collect()
}

/// Strip the wrapped-line styles back to defaults, keeping the text.
fn styled_to_unstyled(p: StyledParagraph) -> Line<'static> {
    if p.is_styled {
        let spans: Vec<Span<'static>> = p
            .spans
            .into_iter()
            .map(|s| Span::raw(s.content.into_owned()))
            .collect();
        Line::from(spans)
    } else {
        Line::from(p.spans)
    }
}

/// Internal carrier that tracks whether a paragraph's spans should be
/// emitted as raw strings or styled spans.
struct StyledParagraph {
    spans: Vec<Span<'static>>,
    is_styled: bool,
}

/// Wrap each paragraph independently and stitch the per-paragraph
/// lines together. Centralizes the split-on-newline / greedy-fill logic
/// so `wrap_lines` and `wrap_lines_styled` only differ in how they
/// project spans.
fn wrap_paragraphs<'a, I>(
    paragraphs: I,
    width: usize,
    style: Style,
    is_styled: bool,
) -> Vec<StyledParagraph>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: Vec<StyledParagraph> = Vec::new();

    for paragraph in paragraphs {
        if width == 0 {
            // Degenerate: no column budget. Emit each paragraph verbatim
            // as a single line. Preserve empty paragraphs so the caller
            // still observes the hard-break count.
            let span = if is_styled {
                Span::styled(paragraph.to_owned(), style)
            } else {
                Span::raw(paragraph.to_owned())
            };
            out.push(StyledParagraph {
                spans: vec![span],
                is_styled,
            });
            continue;
        }

        let wrapped = wrap_single_paragraph(paragraph, width, style, is_styled);
        out.extend(wrapped);
    }

    if out.is_empty() {
        // Empty input → one empty line so renderers always have a row.
        out.push(empty_paragraph_row(style, is_styled));
    }

    out
}

/// Greedy fill for one paragraph (no `\n` inside). Returns the wrapped
/// lines for that paragraph — possibly zero if the paragraph is empty
/// (callers handle the empty-input case at the outer layer).
fn wrap_single_paragraph(
    paragraph: &str,
    width: usize,
    style: Style,
    is_styled: bool,
) -> Vec<StyledParagraph> {
    if paragraph.is_empty() {
        return vec![empty_paragraph_row(style, is_styled)];
    }

    let mut state = WrapState::new(width, style, is_styled);
    for token in tokenize(paragraph) {
        state.feed_token(&token);
    }
    state.finish()
}

/// One empty paragraph row — used for blank hard-break lines and the
/// outer empty-input fallback.
fn empty_paragraph_row(style: Style, is_styled: bool) -> StyledParagraph {
    let span = if is_styled {
        Span::styled(String::new(), style)
    } else {
        Span::raw(String::new())
    };
    StyledParagraph {
        spans: vec![span],
        is_styled,
    }
}

/// Per-paragraph wrap state. Owns the in-progress line plus the
/// pending separator, and exposes one method per token variant.
struct WrapState {
    width: usize,
    style: Style,
    is_styled: bool,
    lines: Vec<StyledParagraph>,
    current: String,
    current_width: usize,
    pending_space_width: usize,
    pending_space_active: bool,
}

impl WrapState {
    fn new(width: usize, style: Style, is_styled: bool) -> Self {
        Self {
            width,
            style,
            is_styled,
            lines: Vec::new(),
            current: String::new(),
            current_width: 0,
            pending_space_width: 0,
            pending_space_active: false,
        }
    }

    fn feed_token(&mut self, token: &Token) {
        match token {
            Token::Whitespace(w) => self.feed_whitespace(w),
            Token::Cjk(c) => self.feed_cjk(*c),
            Token::Word(word) => self.feed_word(word),
        }
    }

    fn feed_whitespace(&mut self, w: &str) {
        self.pending_space_width = w.width();
        self.pending_space_active = true;
    }

    fn feed_cjk(&mut self, c: char) {
        let cw = char_width(c);
        let needed = self.pending_space_width_if_active(cw);

        if self.fits(needed) {
            self.commit_inline(cw, |s| s.current.push(c));
        } else if cw > self.width {
            // Single CJK char wider than the entire row (cannot happen
            // for normal CJK, defensive for edge inputs). Flush and
            // start a fresh line with the glyph.
            self.flush_current();
            self.discard_pending_space();
            self.current.push(c);
            self.current_width += cw;
        } else {
            self.flush_current();
            self.discard_pending_space();
            self.current.push(c);
            self.current_width += cw;
        }
    }

    fn feed_word(&mut self, word: &str) {
        let word_width = word.width();
        let needed = self.pending_space_width_if_active(word_width);

        if self.fits(needed) {
            self.commit_inline(word_width, |s| s.current.push_str(word));
        } else if word_width > self.width {
            // Word wider than the row — break it at char boundaries,
            // leaving the final fragment as `current`.
            self.flush_current();
            self.discard_pending_space();
            let mut chunk = String::new();
            let mut chunk_width: usize = 0;
            for ch in word.chars() {
                let cw = char_width(ch);
                if chunk_width + cw > self.width && !chunk.is_empty() {
                    let line = std::mem::take(&mut chunk);
                    self.push_line(line);
                    chunk_width = 0;
                }
                chunk.push(ch);
                chunk_width += cw;
            }
            if !chunk.is_empty() {
                self.current = chunk;
                self.current_width = chunk_width;
            }
        } else {
            self.flush_current();
            self.discard_pending_space();
            self.current.push_str(word);
            self.current_width += word_width;
        }
    }

    /// Flush `current` as a complete output line, leaving `current`
    /// empty. No-op when `current` is empty.
    fn flush_current(&mut self) {
        if !self.current.is_empty() {
            let line = std::mem::take(&mut self.current);
            self.push_line(line);
            self.current_width = 0;
        }
    }

    fn push_line(&mut self, content: String) {
        let is_styled = self.is_styled;
        let style = self.style;
        let span = if is_styled {
            Span::styled(content, style)
        } else {
            Span::raw(content)
        };
        self.lines.push(StyledParagraph {
            spans: vec![span],
            is_styled,
        });
    }

    fn discard_pending_space(&mut self) {
        self.pending_space_active = false;
        self.pending_space_width = 0;
    }

    /// Inline append with optional pending separator.
    fn commit_inline<F>(&mut self, token_width: usize, append: F)
    where
        F: FnOnce(&mut Self),
    {
        if self.pending_space_active {
            self.current.push(' ');
            self.current_width += self.pending_space_width;
            self.pending_space_active = false;
        }
        append(self);
        self.current_width += token_width;
    }

    fn fits(&self, needed: usize) -> bool {
        self.current_width + needed <= self.width
    }

    fn pending_space_width_if_active(&self, base: usize) -> usize {
        if self.pending_space_active {
            self.pending_space_width + base
        } else {
            base
        }
    }
    /// Emit any trailing accumulated text and return all wrapped rows.
    fn finish(mut self) -> Vec<StyledParagraph> {
        if !self.current.is_empty() {
            let line = std::mem::take(&mut self.current);
            self.push_line(line);
        }
        self.lines
    }
}

/// Wrap-content tokens. We segment once per paragraph; the wrap loop
/// only needs width and content for each token.
enum Token {
    /// Whitespace run. Acts as a separator; the wrap loop tracks one
    /// pending whitespace at a time and drops it on line wrap.
    Whitespace(String),
    /// A single CJK code point. Each is its own breakable token.
    Cjk(char),
    /// Consecutive non-whitespace, non-CJK chars — an ASCII / Latin word.
    Word(String),
}

/// Compute the unicode display width of a single character, treating
/// `None` (control / combining) as 0.
fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Total unicode display width of a string slice.
trait StrWidth {
    fn width(&self) -> usize;
}

impl StrWidth for str {
    fn width(&self) -> usize {
        self.chars().map(char_width).sum()
    }
}

impl StrWidth for String {
    fn width(&self) -> usize {
        self.as_str().width()
    }
}

/// Segment a paragraph into `Whitespace`, `Cjk`, and `Word` tokens.
fn tokenize(paragraph: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut chars = paragraph.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            let mut buf = String::new();
            buf.push(c);
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    buf.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Whitespace(buf));
        } else if is_cjk_breakable(c) {
            tokens.push(Token::Cjk(c));
        } else {
            let mut buf = String::new();
            buf.push(c);
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() || is_cjk_breakable(next) {
                    break;
                }
                buf.push(next);
                chars.next();
            }
            tokens.push(Token::Word(buf));
        }
    }

    tokens
}

/// Returns true when `ch` is a CJK character that allows a line break
/// between any two adjacent CJK glyphs.
///
/// The ranges cover CJK Unified Ideographs, Hangul syllables /
/// Jamo, Compatibility Ideographs, Compatibility Forms, halfwidth
/// and fullwidth forms, and the four CJK Extension blocks (B–D)
/// plus the Compatibility Supplement.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    /// Per-line joined text (without joining lines together).
    fn per_line(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    /// Concatenate the spans of every line into a single string for
    /// easy assertion on the visual output.
    fn joined(lines: &[Line<'_>]) -> String {
        per_line(lines).join("\n")
    }

    #[test]
    fn wraps_long_line_at_word_boundary() {
        // 25 ASCII chars; width 10 forces three wraps.
        let lines = wrap_lines("the quick brown fox jumps", 10);
        assert_eq!(
            per_line(&lines),
            vec![
                "the quick".to_owned(),
                "brown fox".to_owned(),
                "jumps".to_owned(),
            ],
        );
    }

    #[test]
    fn preserves_hard_breaks() {
        // Newlines must produce separate output lines.
        let lines = wrap_lines("alpha\nbeta\ngamma", 80);
        assert_eq!(
            per_line(&lines),
            vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()],
        );
    }

    #[test]
    fn handles_cjk_double_width() {
        // "你好世界" — 4 chars × width 2 = 8 columns.
        // Width 4 packs exactly 2 CJK glyphs per row (verifying the
        // width-2 accounting, not 1).
        let lines = wrap_lines("你好世界", 4);
        assert_eq!(per_line(&lines), vec!["你好".to_owned(), "世界".to_owned()],);

        // Width 2 forces exactly one CJK glyph per row.
        let lines = wrap_lines("你好世界", 2);
        assert_eq!(
            per_line(&lines),
            vec![
                "你".to_owned(),
                "好".to_owned(),
                "世".to_owned(),
                "界".to_owned()
            ],
        );
    }
    #[test]
    fn empty_string_returns_one_empty_line() {
        let lines = wrap_lines("", 80);
        assert_eq!(per_line(&lines), vec![String::new()]);
    }

    #[test]
    fn wrap_lines_returns_no_styling() {
        let lines = wrap_lines("hello world", 80);
        for line in &lines {
            for span in &line.spans {
                assert_eq!(span.style, Style::default());
            }
        }
    }

    #[test]
    fn wrap_lines_styled_applies_style_to_every_span() {
        let style = Style::default().fg(Color::Red);
        let lines = wrap_lines_styled("hello world", 80, style);
        assert!(!lines.is_empty());
        for span in &lines[0].spans {
            assert_eq!(span.style, style);
        }
    }

    #[test]
    fn wrap_lines_styled_splits_cjk_with_style() {
        let style = Style::default().fg(Color::Blue);
        let lines = wrap_lines_styled("你好世界", 2, style);
        assert_eq!(lines.len(), 4);
        for line in &lines {
            assert_eq!(line.spans.len(), 1);
            assert_eq!(line.spans[0].style, style);
        }
        assert_eq!(joined(&lines), "你\n好\n世\n界");
    }

    #[test]
    fn hard_break_then_blank_paragraph_yields_three_lines() {
        // "a\n\nb" → "a", "", "b".
        let lines = wrap_lines("a\n\nb", 80);
        assert_eq!(
            per_line(&lines),
            vec!["a".to_owned(), String::new(), "b".to_owned()],
        );
    }

    #[test]
    fn pending_whitespace_dropped_on_line_wrap() {
        // "ab cd" with width 2: "ab" then "cd" — no leading space on
        // the second line.
        let lines = wrap_lines("ab cd", 2);
        assert_eq!(per_line(&lines), vec!["ab".to_owned(), "cd".to_owned()]);
    }

    #[test]
    fn long_word_broken_at_char_boundary() {
        // A single 12-char word at width 5 must split mid-word.
        let lines = wrap_lines("abcdefghijkl", 5);
        assert_eq!(
            per_line(&lines),
            vec!["abcde".to_owned(), "fghij".to_owned(), "kl".to_owned(),],
        );
    }

    #[test]
    fn zero_width_returns_paragraphs_unchanged() {
        let lines = wrap_lines("alpha\nbeta", 0);
        assert_eq!(
            per_line(&lines),
            vec!["alpha".to_owned(), "beta".to_owned()],
        );
    }

    #[test]
    fn trailing_newline_preserves_trailing_empty_line() {
        // split('\n') is total: "a\n" → ["a", ""] → two lines.
        let lines = wrap_lines("a\n", 80);
        assert_eq!(per_line(&lines), vec!["a".to_owned(), String::new()]);
    }

    #[test]
    fn is_cjk_breakable_covers_common_ranges() {
        assert!(is_cjk_breakable('中')); // CJK Unified
        assert!(is_cjk_breakable('한')); // Hangul Syllables
        assert!(is_cjk_breakable('字')); // CJK Compatibility
        assert!(!is_cjk_breakable('a')); // ASCII
        assert!(!is_cjk_breakable(' '));
        assert!(!is_cjk_breakable('1'));
    }
}
