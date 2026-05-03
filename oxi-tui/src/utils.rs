//! Text utilities for terminal UI rendering.
//!
//! Provides ANSI-aware text manipulation: wrapping, truncation, stripping,
//! width measurement, grapheme segmentation, word boundary detection, and
//! search highlighting. All width calculations understand ANSI escape codes
//! (they are invisible and contribute zero width) and wide Unicode characters
//! (CJK, emoji, etc. contribute two columns).

use std::fmt::Write;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

/// Result of extracting an ANSI escape sequence from a string.
struct AnsiSequence {
    /// The full escape code string.
    code: String,
    /// Byte length of the code in the source string.
    len: usize,
}

/// Try to extract an ANSI escape sequence starting at byte position `pos`.
///
/// Recognises:
/// - **CSI**: `ESC [` … final byte in `[mGKHJ]`
/// - **OSC**: `ESC ]` … `BEL` or `ESC \` (string terminator)
/// - **APC**: `ESC _` … `BEL` or `ESC \`
fn extract_ansi_code(s: &str, pos: usize) -> Option<AnsiSequence> {
    let bytes = s.as_bytes();
    if pos >= bytes.len() || bytes[pos] != 0x1b {
        return None;
    }
    if pos + 1 >= bytes.len() {
        return None;
    }
    let next = bytes[pos + 1];

    // CSI sequence: ESC [ ... <final>
    if next == b'[' {
        let mut j = pos + 2;
        while j < bytes.len() {
            let b = bytes[j];
            if matches!(b, b'm' | b'G' | b'K' | b'H' | b'J') {
                let end = j + 1;
                return Some(AnsiSequence {
                    code: s[pos..end].to_string(),
                    len: end - pos,
                });
            }
            j += 1;
        }
        return None;
    }

    // OSC sequence: ESC ] ... BEL or ESC \
    if next == b']' {
        let mut j = pos + 2;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                let end = j + 1;
                return Some(AnsiSequence {
                    code: s[pos..end].to_string(),
                    len: end - pos,
                });
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                let end = j + 2;
                return Some(AnsiSequence {
                    code: s[pos..end].to_string(),
                    len: end - pos,
                });
            }
            j += 1;
        }
        return None;
    }

    // APC sequence: ESC _ ... BEL or ESC \
    if next == b'_' {
        let mut j = pos + 2;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                let end = j + 1;
                return Some(AnsiSequence {
                    code: s[pos..end].to_string(),
                    len: end - pos,
                });
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                let end = j + 2;
                return Some(AnsiSequence {
                    code: s[pos..end].to_string(),
                    len: end - pos,
                });
            }
            j += 1;
        }
        return None;
    }

    None
}

/// Strip all ANSI escape codes from `text`, returning plain visible content.
///
/// ```
/// use oxi_tui::utils::strip_ansi;
/// assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
/// ```
pub fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let text_for_ansi = text; // borrow for ANSI extraction
    let mut byte_i = 0;
    let mut char_i = 0;
    while byte_i < text.len() {
        if let Some(seq) = extract_ansi_code(text_for_ansi, byte_i) {
            byte_i += seq.len;
            // Skip corresponding chars
            let seq_str = &text_for_ansi[byte_i - seq.len..byte_i];
            char_i += seq_str.chars().count();
            continue;
        }
        result.push(chars[char_i]);
        byte_i += chars[char_i].len_utf8();
        char_i += 1;
    }
    result
}

/// Calculate the visible terminal width of `text`, accounting for:
/// - ANSI escape codes (zero width)
/// - Wide characters (CJK, emoji – 2 columns)
/// - Tab stops (counted as 3 columns)
/// - Combining marks and control characters (zero width)
///
/// ```
/// use oxi_tui::utils::visible_width;
/// assert_eq!(visible_width("hello"), 5);
/// assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
/// ```
pub fn visible_width(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    // Fast path: pure ASCII printable (no ANSI, no tabs)
    if is_printable_ascii(text) {
        return text.len();
    }

    // Strip ANSI, expand tabs
    let clean = if text.contains('\t') || text.contains('\x1b') {
        let stripped = strip_ansi(text);
        stripped.replace('\t', "   ")
    } else {
        text.to_string()
    };

    // Sum grapheme widths via unicode-width
    clean
        .graphemes(true)
        .map(|g| grapheme_visible_width(g))
        .sum()
}

/// Return the visible terminal width of a single grapheme cluster.
fn grapheme_visible_width(g: &str) -> usize {
    if g.is_empty() {
        return 0;
    }
    // unicode-width handles most cases correctly
    let w = UnicodeWidthStr::width(g);
    // Emoji that unicode-width reports 0 for but which are actually 2-wide
    // Regional indicator symbols
    if w == 0 {
        let first_cp = g.chars().next().unwrap() as u32;
        // Regional indicators U+1F1E6..U+1F1FF → 2
        if (0x1F1E6..=0x1F1FF).contains(&first_cp) {
            return 2;
        }
    }
    w
}

/// Fast check: is every byte a printable ASCII character (0x20..=0x7E)?
fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| b >= 0x20 && b <= 0x7E)
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

/// Truncate `text` to fit within `max_width` visible terminal columns.
///
/// If the text is longer than `max_width`, it is truncated and `ellipsis`
/// (default `"..."`) is appended. ANSI escape codes are preserved and do not
/// count toward the width.
///
/// When `pad` is true, the result is padded with spaces to exactly `max_width`.
///
/// ```
/// use oxi_tui::utils::truncate_to_width;
/// assert_eq!(truncate_to_width("hello world", 8, None, false), "hello...");
/// assert_eq!(truncate_to_width("hi", 5, None, true), "hi   ");
/// ```
pub fn truncate_to_width(
    text: &str,
    max_width: usize,
    ellipsis: Option<&str>,
    pad: bool,
) -> String {
    let ellipsis = ellipsis.unwrap_or("...");
    if max_width == 0 {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }
    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let text_w = visible_width(text);
    if text_w <= max_width {
        return if pad {
            let mut s = text.to_string();
            let _ = write!(s, "{:width$}", "", width = max_width - text_w);
            s
        } else {
            text.to_string()
        };
    }

    let ellipsis_w = visible_width(ellipsis);
    if ellipsis_w >= max_width {
        // Ellipsis itself is too long; just clip it
        let clipped = truncate_fragment(ellipsis, max_width);
        return if pad {
            format!("{:width$}", clipped, width = max_width)
        } else {
            clipped
        };
    }

    let target = max_width.saturating_sub(ellipsis_w);

    // Fast path: pure ASCII
    if is_printable_ascii(text) && !text.contains('\x1b') {
        let mut byte_end = text.len();
        let mut idx = 0;
        for (i, _) in text.char_indices() {
            if idx == target {
                byte_end = i;
                break;
            }
            idx += 1;
        }
        let prefix = &text[..byte_end];
        return finalize_truncation(prefix, target, ellipsis, ellipsis_w, max_width, pad);
    }

    // General path: walk graphemes, keep ANSI codes
    let mut result = String::new();
    let mut pending_ansi = String::new();
    let mut kept_width = 0usize;
    let mut i = 0;

    while i < text.len() {
        if let Some(seq) = extract_ansi_code(text, i) {
            pending_ansi.push_str(&seq.code);
            i += seq.len;
            continue;
        }

        // Collect non-ANSI run
        let start = i;
        while i < text.len() && extract_ansi_code(text, i).is_none() {
            i += 1;
        }
        let run = &text[start..i];

        for g in run.graphemes(true) {
            let gw = grapheme_visible_width(g);
            if kept_width + gw > target {
                return finalize_truncation(
                    &result, kept_width, ellipsis, ellipsis_w, max_width, pad,
                );
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push_str(g);
            kept_width += gw;
        }
    }

    // Entire text fits within target — shouldn't happen since text_w > max_width,
    // but handle gracefully.
    finalize_truncation(&result, kept_width, ellipsis, ellipsis_w, max_width, pad)
}

/// Truncate a plain (non-ANSI) fragment to at most `max_width` columns.
fn truncate_fragment(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut w = 0;
    for g in text.graphemes(true) {
        let gw = grapheme_visible_width(g);
        if w + gw > max_width {
            break;
        }
        result.push_str(g);
        w += gw;
    }
    result
}

/// Build the final truncated string: prefix + [reset] + ellipsis + optional padding.
/// Only emits ANSI resets when the prefix contains ANSI codes.
fn finalize_truncation(
    prefix: &str,
    prefix_w: usize,
    ellipsis: &str,
    ellipsis_w: usize,
    max_width: usize,
    pad: bool,
) -> String {
    let has_ansi = prefix.contains('\x1b');
    let total_w = prefix_w + ellipsis_w;
    let mut result = if has_ansi {
        format!("{}\x1b[0m{}", prefix, ellipsis)
    } else {
        format!("{}{}", prefix, ellipsis)
    };
    if pad && total_w < max_width {
        let _ = write!(result, "{:width$}", "", width = max_width - total_w);
    }
    result
}

// ---------------------------------------------------------------------------
// Text wrapping
// ---------------------------------------------------------------------------

/// Wrap `text` to `width` visible columns using word-wrap.
///
/// Handles ANSI escape codes (preserved across line breaks), newlines
/// ( honoured as hard breaks), and wide Unicode characters.
///
/// Returns a vector of lines, each with visible width ≤ `width`.
/// Lines are **not** padded.
///
/// ```
/// use oxi_tui::utils::wrap_text;
/// let lines = wrap_text("hello world", 6);
/// assert_eq!(lines, vec!["hello", "world"]);
/// ```
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if width == 0 {
        return vec![String::new()];
    }

    let mut result: Vec<String> = Vec::new();
    let mut active_codes = String::new(); // ANSI state carried across lines

    for line in text.split('\n') {
        let input = if result.is_empty() && active_codes.is_empty() {
            line.to_string()
        } else {
            format!("{}{}", active_codes, line)
        };

        let wrapped = wrap_single_line(&input, width);
        // Update active ANSI codes from this line
        active_codes = collect_active_ansi_codes(line);
        result.extend(wrapped);
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Word-wrap a single line (no embedded newlines) to `width` columns.
#[allow(unused_assignments)]
fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if visible_width(line) <= width {
        return vec![line.to_string()];
    }

    let tokens = split_into_tokens_with_ansi(line);
    let mut wrapped: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0usize;

    for token in &tokens {
        let token_w = visible_width(token);
        let is_whitespace = token.trim().is_empty();

        // Token itself exceeds width – break character-by-character
        if token_w > width && !is_whitespace {
            if !current_line.is_empty() {
                wrapped.push(current_line);
            }
            current_line = String::new();
            current_width = 0;
            let broken = break_long_word(token, width);
            // All but the last go directly into output
            for bl in &broken[..broken.len() - 1] {
                wrapped.push(bl.clone());
            }
            current_line = broken.last().unwrap().clone();
            current_width = visible_width(&current_line);
            continue;
        }

        if current_width + token_w > width && current_width > 0 {
            wrapped.push(current_line.trim_end().to_string());
            if is_whitespace {
                current_line = String::new();
                current_width = 0;
            } else {
                current_line = token.clone();
                current_width = token_w;
            }
        } else {
            current_line.push_str(token);
            current_width += token_w;
        }
    }

    if !current_line.is_empty() {
        wrapped.push(current_line.trim_end().to_string());
    }

    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
}

/// Split a line into alternating whitespace / non-whitespace tokens while
/// keeping ANSI codes attached to the adjacent visible content.
fn split_into_tokens_with_ansi(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut in_whitespace = false;
    let mut i = 0;

    while i < text.len() {
        if let Some(seq) = extract_ansi_code(text, i) {
            pending_ansi.push_str(&seq.code);
            i += seq.len;
            continue;
        }

        let ch = text.as_bytes()[i] as char;
        let ch_is_space = ch == ' ';

        if ch_is_space != in_whitespace && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }

        if !pending_ansi.is_empty() {
            current.push_str(&pending_ansi);
            pending_ansi.clear();
        }

        in_whitespace = ch_is_space;
        current.push(ch);
        i += ch.len_utf8();
    }

    if !pending_ansi.is_empty() {
        current.push_str(&pending_ansi);
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Break a single long word across lines of at most `width` columns.
fn break_long_word(word: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0usize;
    let mut i = 0;

    while i < word.len() {
        if let Some(seq) = extract_ansi_code(word, i) {
            current_line.push_str(&seq.code);
            i += seq.len;
            continue;
        }

        // Collect non-ANSI run
        let start = i;
        while i < word.len() && extract_ansi_code(word, i).is_none() {
            i += 1;
        }
        let run = &word[start..i];

        for g in run.graphemes(true) {
            let gw = grapheme_visible_width(g);
            if current_width + gw > width {
                if !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                }
                current_width = 0;
            }
            current_line.push_str(g);
            current_width += gw;
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Walk `text` and collect the ANSI SGR / hyperlink codes that are still
/// "open" at the end, so they can be re-emitted at the start of the next line.
fn collect_active_ansi_codes(text: &str) -> String {
    // Minimal implementation: track foreground, background, and basic attributes.
    let mut bold = false;
    let mut dim = false;
    let mut italic = false;
    let mut underline = false;
    let mut fg: Option<String> = None;
    let mut bg: Option<String> = None;

    let mut i = 0;
    while i < text.len() {
        if let Some(seq) = extract_ansi_code(text, i) {
            if seq.code.ends_with('m') {
                // Parse SGR parameters
                let inner = seq.code.trim_start_matches("\x1b[").trim_end_matches('m');
                if inner.is_empty() || inner == "0" {
                    bold = false;
                    dim = false;
                    italic = false;
                    underline = false;
                    fg = None;
                    bg = None;
                } else {
                    let parts: Vec<&str> = inner.split(';').collect();
                    let mut pi = 0;
                    while pi < parts.len() {
                        let code: i32 = parts[pi].parse().unwrap_or(0);
                        match code {
                            0 => {
                                bold = false;
                                dim = false;
                                italic = false;
                                underline = false;
                                fg = None;
                                bg = None;
                            }
                            1 => bold = true,
                            2 => dim = true,
                            3 => italic = true,
                            4 => underline = true,
                            22 => {
                                bold = false;
                                dim = false;
                            }
                            23 => italic = false,
                            24 => underline = false,
                            38 | 48 => {
                                // 256-color or RGB
                                if pi + 1 < parts.len()
                                    && parts[pi + 1] == "5"
                                    && pi + 2 < parts.len()
                                {
                                    let color = format!(
                                        "{};{};{}",
                                        parts[pi],
                                        parts[pi + 1],
                                        parts[pi + 2]
                                    );
                                    if code == 38 {
                                        fg = Some(color);
                                    } else {
                                        bg = Some(color);
                                    }
                                    pi += 3;
                                    continue;
                                } else if pi + 1 < parts.len()
                                    && parts[pi + 1] == "2"
                                    && pi + 4 < parts.len()
                                {
                                    let color = format!(
                                        "{};{};{};{};{}",
                                        parts[pi],
                                        parts[pi + 1],
                                        parts[pi + 2],
                                        parts[pi + 3],
                                        parts[pi + 4]
                                    );
                                    if code == 38 {
                                        fg = Some(color);
                                    } else {
                                        bg = Some(color);
                                    }
                                    pi += 5;
                                    continue;
                                }
                            }
                            39 => fg = None,
                            49 => bg = None,
                            c if (30..=37).contains(&c) || (90..=97).contains(&c) => {
                                fg = Some(c.to_string());
                            }
                            c if (40..=47).contains(&c) || (100..=107).contains(&c) => {
                                bg = Some(c.to_string());
                            }
                            _ => {}
                        }
                        pi += 1;
                    }
                }
            }
            i += seq.len;
        } else {
            i += 1;
        }
    }

    let mut codes: Vec<String> = Vec::new();
    if bold {
        codes.push("1".to_string());
    }
    if dim {
        codes.push("2".to_string());
    }
    if italic {
        codes.push("3".to_string());
    }
    if underline {
        codes.push("4".to_string());
    }
    if let Some(ref c) = fg {
        codes.push(c.clone());
    }
    if let Some(ref c) = bg {
        codes.push(c.clone());
    }

    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

// ---------------------------------------------------------------------------
// String segmentation
// ---------------------------------------------------------------------------

/// Segment text into grapheme clusters, preserving ANSI escape sequences
/// as separate "segments".
///
/// Returns a vector of segments where each segment is either a visible
/// grapheme cluster or an ANSI escape code.
///
/// ```
/// use oxi_tui::utils::segment_text;
/// let segs = segment_text("ab");
/// assert_eq!(segs, vec!["a", "b"]);
/// ```
pub fn segment_text(text: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut i = 0;

    while i < text.len() {
        if let Some(seq) = extract_ansi_code(text, i) {
            segments.push(seq.code);
            i += seq.len;
            continue;
        }

        // Collect non-ANSI run
        let start = i;
        while i < text.len() && extract_ansi_code(text, i).is_none() {
            i += 1;
        }
        let run = &text[start..i];
        for g in run.graphemes(true) {
            segments.push(g.to_string());
        }
    }

    segments
}

// ---------------------------------------------------------------------------
// Word boundary detection
// ---------------------------------------------------------------------------

/// A word boundary in text, given as a byte offset and the kind of boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordBoundary {
    /// Byte offset where the boundary occurs.
    pub offset: usize,
    /// Kind of boundary.
    pub kind: WordBoundaryKind,
}

/// Kind of word boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBoundaryKind {
    /// Transition from non-word to word character.
    Start,
    /// Transition from word to non-word character.
    End,
}

/// Find word boundaries in `text`.
///
/// A "word" character is alphanumeric or underscore. Boundaries are reported
/// at every transition between word and non-word characters.
///
/// ```
/// use oxi_tui::utils::{find_word_boundaries, WordBoundaryKind};
/// let bounds = find_word_boundaries("hello world");
/// assert_eq!(bounds[0].offset, 0);
/// assert_eq!(bounds[0].kind, WordBoundaryKind::Start);
/// ```
pub fn find_word_boundaries(text: &str) -> Vec<WordBoundary> {
    let mut boundaries: Vec<WordBoundary> = Vec::new();
    let mut in_word = false;

    for (i, ch) in text.char_indices() {
        let is_word = ch.is_alphanumeric() || ch == '_';
        if is_word && !in_word {
            boundaries.push(WordBoundary {
                offset: i,
                kind: WordBoundaryKind::Start,
            });
        } else if !is_word && in_word {
            boundaries.push(WordBoundary {
                offset: i,
                kind: WordBoundaryKind::End,
            });
        }
        in_word = is_word;
    }

    // If the text ends in a word, add a final End boundary
    if in_word {
        boundaries.push(WordBoundary {
            offset: text.len(),
            kind: WordBoundaryKind::End,
        });
    }

    boundaries
}

/// Find the word (or whitespace run) that contains byte offset `pos`.
///
/// Returns the byte range `(start, end)` of the word. If `pos` is inside a
/// word, returns that word's span. If `pos` is in whitespace, returns the
/// whitespace span.
///
/// ```
/// use oxi_tui::utils::word_at;
/// assert_eq!(word_at("hello world", 2), Some((0, 5)));
/// assert_eq!(word_at("hello world", 5), Some((5, 6))); // space
/// ```
pub fn word_at(text: &str, pos: usize) -> Option<(usize, usize)> {
    if text.is_empty() || pos > text.len() {
        return None;
    }

    // Clamp pos to the last char boundary ≤ pos
    let pos = if pos >= text.len() {
        text.len()
    } else {
        text.floor_char_boundary(pos)
    };

    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';

    // Determine the class at pos
    let char_at = text[pos..].chars().next();
    let target_is_word = match char_at {
        Some(ch) => is_word(ch),
        None => {
            // Past the end – look backwards
            text.chars().last().map_or(false, |ch| is_word(ch))
        }
    };

    // Find the start of the run by scanning backward from pos
    let mut start = 0;
    for (i, ch) in text[..pos].char_indices().rev() {
        if is_word(ch) == target_is_word {
            continue;
        }
        start = i + ch.len_utf8();
        break;
    }

    // Find the end of the run
    let mut end = pos;
    for (i, ch) in text[pos..].char_indices() {
        if is_word(ch) == target_is_word {
            end = pos + i + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == pos {
        // We may be at the very end or just didn't advance
        if let Some(ch) = text[pos..].chars().next() {
            if is_word(ch) == target_is_word {
                end = pos + ch.len_utf8();
            }
        }
    }

    if start < end {
        Some((start, end))
    } else if start == end && end < text.len() {
        // Edge case: single char
        let ch = text[start..].chars().next()?;
        Some((start, start + ch.len_utf8()))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Search highlighting
// ---------------------------------------------------------------------------

/// Highlight occurrences of `query` in `text` by wrapping them in ANSI
/// reverse-video codes (`\x1b[7m` … `\x1b[27m`).
///
/// Matching is case-insensitive. Returns a new string with highlights applied.
///
/// ```
/// use oxi_tui::utils::highlight_matches;
/// let result = highlight_matches("hello world", "world");
/// assert!(result.contains("\x1b[7mworld\x1b[27m"));
/// ```
pub fn highlight_matches(text: &str, query: &str) -> String {
    if query.is_empty() {
        return text.to_string();
    }

    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    let mut result = String::with_capacity(text.len() + 32);
    let mut last_end = 0;

    // Find all non-overlapping matches
    let mut start = 0;
    while let Some(offset) = text_lower[start..].find(&query_lower) {
        let abs_offset = start + offset;
        // Append text before match
        result.push_str(&text[last_end..abs_offset]);
        // Append highlighted match
        result.push_str("\x1b[7m");
        result.push_str(&text[abs_offset..abs_offset + query.len()]);
        result.push_str("\x1b[27m");
        last_end = abs_offset + query.len();
        start = abs_offset + query.len();
    }

    // Append remaining text
    if last_end < text.len() {
        result.push_str(&text[last_end..]);
    }

    result
}

/// Highlight matches with custom prefix and suffix strings instead of
/// ANSI codes, useful for non-terminal output or custom styling.
///
/// ```
/// use oxi_tui::utils::highlight_matches_with;
/// let result = highlight_matches_with("hello world", "world", "<b>", "</b>");
/// assert_eq!(result, "hello <b>world</b>");
/// ```
pub fn highlight_matches_with(text: &str, query: &str, prefix: &str, suffix: &str) -> String {
    if query.is_empty() {
        return text.to_string();
    }

    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    let mut result = String::with_capacity(text.len() + 32);
    let mut last_end = 0;

    let mut start = 0;
    while let Some(offset) = text_lower[start..].find(&query_lower) {
        let abs_offset = start + offset;
        result.push_str(&text[last_end..abs_offset]);
        result.push_str(prefix);
        result.push_str(&text[abs_offset..abs_offset + query.len()]);
        result.push_str(suffix);
        last_end = abs_offset + query.len();
        start = abs_offset + query.len();
    }

    if last_end < text.len() {
        result.push_str(&text[last_end..]);
    }

    result
}

// ---------------------------------------------------------------------------
// Column-level slicing (for horizontal scrolling)
// ---------------------------------------------------------------------------

/// Slice a line by visible column range. Returns the visible portion of
/// `line` starting at column `start_col` for `length` columns.
///
/// ANSI escape codes are preserved where possible.
///
/// ```
/// use oxi_tui::utils::slice_by_column;
/// assert_eq!(slice_by_column("hello world", 6, 5), "world");
/// ```
pub fn slice_by_column(line: &str, start_col: usize, length: usize) -> String {
    if length == 0 {
        return String::new();
    }
    let end_col = start_col + length;
    let mut result = String::new();
    let mut current_col = 0usize;
    let mut pending_ansi = String::new();
    let mut i = 0;

    while i < line.len() {
        if let Some(seq) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                result.push_str(&seq.code);
            } else if current_col < start_col {
                pending_ansi.push_str(&seq.code);
            }
            i += seq.len;
            continue;
        }

        // Collect non-ANSI run
        let start = i;
        while i < line.len() && extract_ansi_code(line, i).is_none() {
            i += 1;
        }
        let run = &line[start..i];

        for g in run.graphemes(true) {
            let gw = grapheme_visible_width(g);
            if current_col >= start_col && current_col + gw <= end_col {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(g);
            }
            current_col += gw;
            if current_col >= end_col {
                break;
            }
        }
        if current_col >= end_col {
            break;
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Character classification
// ---------------------------------------------------------------------------

/// Returns true if `ch` is a whitespace character.
///
/// ```
/// use oxi_tui::utils::is_whitespace_char;
/// assert!(is_whitespace_char(' '));
/// assert!(!is_whitespace_char('a'));
/// ```
pub fn is_whitespace_char(ch: char) -> bool {
    ch.is_whitespace()
}

/// Returns true if `ch` is a punctuation character commonly used in
/// programming contexts.
///
/// ```
/// use oxi_tui::utils::is_punctuation_char;
/// assert!(is_punctuation_char('.'));
/// assert!(is_punctuation_char('('));
/// assert!(!is_punctuation_char('a'));
/// ```
pub fn is_punctuation_char(ch: char) -> bool {
    matches!(
        ch,
        '(' | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '<'
            | '>'
            | '.'
            | ','
            | ';'
            | ':'
            | '\''
            | '"'
            | '!'
            | '?'
            | '+'
            | '-'
            | '='
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '^'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
    )
}

// ---------------------------------------------------------------------------
// Background application
// ---------------------------------------------------------------------------

/// Apply a background colour to a line by wrapping the entire content
/// (including padding) in ANSI 256-colour or true-colour escapes.
///
/// The line is padded with spaces to exactly `width` columns.
pub fn apply_background_to_line<F>(line: &str, width: usize, bg_fn: F) -> String
where
    F: Fn(&str) -> String,
{
    let vis = visible_width(line);
    let padding = width.saturating_sub(vis);
    let padded = format!("{}{}", line, " ".repeat(padding));
    bg_fn(&padded)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- strip_ansi ---

    #[test]
    fn test_strip_ansi_basic() {
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
    }

    #[test]
    fn test_strip_ansi_multiple_codes() {
        assert_eq!(
            strip_ansi("\x1b[1;31m\x1b[48;5;240mhello world\x1b[0m"),
            "hello world"
        );
    }

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_strip_ansi_osc_sequence() {
        assert_eq!(
            strip_ansi("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"),
            "link"
        );
    }

    // --- visible_width ---

    #[test]
    fn test_visible_width_ascii() {
        assert_eq!(visible_width("hello"), 5);
    }

    #[test]
    fn test_visible_width_ansi_ignored() {
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
    }

    #[test]
    fn test_visible_width_wide_char() {
        // CJK character is 2 columns
        assert_eq!(visible_width("你好"), 4);
    }

    #[test]
    fn test_visible_width_tabs() {
        assert_eq!(visible_width("a\tb"), 5); // a + 3 spaces + b
    }

    #[test]
    fn test_visible_width_empty() {
        assert_eq!(visible_width(""), 0);
    }

    // --- truncate_to_width ---

    #[test]
    fn test_truncate_short_text() {
        assert_eq!(truncate_to_width("hi", 10, None, false), "hi");
    }

    #[test]
    fn test_truncate_exact_fit() {
        assert_eq!(truncate_to_width("hello", 5, None, false), "hello");
    }

    #[test]
    fn test_truncate_with_ellipsis() {
        assert_eq!(truncate_to_width("hello world", 8, None, false), "hello...");
    }

    #[test]
    fn test_truncate_with_padding() {
        let result = truncate_to_width("hi", 5, None, true);
        assert_eq!(result, "hi   ");
    }

    #[test]
    fn test_truncate_custom_ellipsis() {
        // "…" is 1 column; max_width=9, target=8, "hello wo"=8 chars
        let result = truncate_to_width("hello world", 9, Some("…"), false);
        assert_eq!(result, "hello wo…");
    }

    #[test]
    fn test_truncate_with_ansi() {
        let result = truncate_to_width("\x1b[31mhello world\x1b[0m", 8, None, false);
        // Should contain ANSI codes + "hello" + ellipsis
        assert!(result.contains("\x1b[31m"));
        assert!(result.contains("hello"));
        assert!(result.contains("..."));
    }

    #[test]
    fn test_truncate_zero_width() {
        assert_eq!(truncate_to_width("hello", 0, None, false), "");
    }

    // --- wrap_text ---

    #[test]
    fn test_wrap_text_basic() {
        let lines = wrap_text("hello world", 6);
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[test]
    fn test_wrap_text_short_enough() {
        let lines = wrap_text("hi", 10);
        assert_eq!(lines, vec!["hi"]);
    }

    #[test]
    fn test_wrap_text_with_newlines() {
        let lines = wrap_text("hello\nworld", 10);
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[test]
    fn test_wrap_text_long_word() {
        let lines = wrap_text("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn test_wrap_text_empty() {
        let lines = wrap_text("", 10);
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn test_wrap_text_preserves_ansi() {
        let input = "\x1b[31mhello world\x1b[0m";
        let lines = wrap_text(input, 6);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\x1b[31m"));
        assert!(lines[1].contains("\x1b[0m") || lines[0].contains("\x1b[0m"));
    }

    // --- segment_text ---

    #[test]
    fn test_segment_text_basic() {
        assert_eq!(segment_text("ab"), vec!["a", "b"]);
    }

    #[test]
    fn test_segment_text_emoji() {
        let segs = segment_text("a🎉b");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], "a");
        assert_eq!(segs[1], "🎉");
        assert_eq!(segs[2], "b");
    }

    #[test]
    fn test_segment_text_with_ansi() {
        let segs = segment_text("\x1b[31mhi\x1b[0m");
        assert!(segs.contains(&"\x1b[31m".to_string()));
        assert!(segs.contains(&"h".to_string()));
        assert!(segs.contains(&"i".to_string()));
        assert!(segs.contains(&"\x1b[0m".to_string()));
    }

    // --- find_word_boundaries ---

    #[test]
    fn test_word_boundaries_simple() {
        let bounds = find_word_boundaries("hello world");
        assert_eq!(bounds[0].offset, 0);
        assert_eq!(bounds[0].kind, WordBoundaryKind::Start);
        assert_eq!(bounds[1].offset, 5);
        assert_eq!(bounds[1].kind, WordBoundaryKind::End);
        assert_eq!(bounds[2].offset, 6);
        assert_eq!(bounds[2].kind, WordBoundaryKind::Start);
    }

    #[test]
    fn test_word_boundaries_underscores() {
        let bounds = find_word_boundaries("foo_bar baz");
        // foo_bar is one word because underscores are word chars
        assert_eq!(bounds.len(), 4); // start foo_bar, end foo_bar, start baz, end baz
    }

    #[test]
    fn test_word_boundaries_empty() {
        assert!(find_word_boundaries("").is_empty());
    }

    // --- word_at ---

    #[test]
    fn test_word_at_middle() {
        assert_eq!(word_at("hello world", 2), Some((0, 5)));
    }

    #[test]
    fn test_word_at_space() {
        assert_eq!(word_at("hello world", 5), Some((5, 6)));
    }

    #[test]
    fn test_word_at_second_word() {
        assert_eq!(word_at("hello world", 8), Some((6, 11)));
    }

    // --- highlight_matches ---

    #[test]
    fn test_highlight_matches_basic() {
        let result = highlight_matches("hello world", "world");
        assert_eq!(result, "hello \x1b[7mworld\x1b[27m");
    }

    #[test]
    fn test_highlight_matches_case_insensitive() {
        let result = highlight_matches("Hello World", "hello");
        assert!(result.contains("\x1b[7mHello\x1b[27m"));
    }

    #[test]
    fn test_highlight_matches_multiple() {
        let result = highlight_matches("ab ab ab", "ab");
        assert_eq!(
            result,
            "\x1b[7mab\x1b[27m \x1b[7mab\x1b[27m \x1b[7mab\x1b[27m"
        );
    }

    #[test]
    fn test_highlight_matches_empty_query() {
        assert_eq!(highlight_matches("hello", ""), "hello");
    }

    #[test]
    fn test_highlight_matches_no_match() {
        assert_eq!(highlight_matches("hello", "xyz"), "hello");
    }

    // --- highlight_matches_with ---

    #[test]
    fn test_highlight_matches_with_custom() {
        assert_eq!(
            highlight_matches_with("hello world", "world", "<b>", "</b>"),
            "hello <b>world</b>"
        );
    }

    // --- slice_by_column ---

    #[test]
    fn test_slice_by_column_basic() {
        assert_eq!(slice_by_column("hello world", 6, 5), "world");
    }

    #[test]
    fn test_slice_by_column_start() {
        assert_eq!(slice_by_column("hello world", 0, 5), "hello");
    }

    #[test]
    fn test_slice_by_column_with_wide_char() {
        // "你" is 2 columns wide
        assert_eq!(slice_by_column("你good", 2, 4), "good");
    }

    #[test]
    fn test_slice_by_column_zero_length() {
        assert_eq!(slice_by_column("hello", 0, 0), "");
    }

    // --- Character classification ---

    #[test]
    fn test_is_whitespace() {
        assert!(is_whitespace_char(' '));
        assert!(is_whitespace_char('\t'));
        assert!(!is_whitespace_char('a'));
    }

    #[test]
    fn test_is_punctuation() {
        assert!(is_punctuation_char('.'));
        assert!(is_punctuation_char('('));
        assert!(!is_punctuation_char('a'));
        assert!(!is_punctuation_char(' '));
    }

    // --- apply_background_to_line ---

    #[test]
    fn test_apply_background_padding() {
        let result = apply_background_to_line("hi", 5, |s| format!("[{}]", s));
        assert_eq!(result, "[hi   ]");
    }

    // --- truncate_to_width edge cases ---

    #[test]
    fn test_truncate_wide_chars() {
        // 你=2, 好=2, a=1, b=1 → total 6
        assert_eq!(truncate_to_width("你好ab", 6, None, false), "你好ab");
    }

    #[test]
    fn test_truncate_wide_chars_with_ellipsis() {
        let result = truncate_to_width("你好世界", 5, None, false);
        let stripped = strip_ansi(&result);
        let w = visible_width(&stripped);
        // 你 = 2 cols, ... = 3 cols → total 5
        assert_eq!(w, 5, "result={:?} stripped={:?} w={}", result, stripped, w);
    }
}
