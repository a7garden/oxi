//! Stateful, line-oriented classifier for hashline diff text.
//!
//! Turns raw patch lines into typed [`Token`]s (section headers, hunk
//! headers, payload rows). The parser consumes the token stream.
//!
//! Line-ops only (default build): `SWAP`, `DEL`, `INS.PRE|POST|HEAD|TAIL`.
//! Block ops (`SWAP.BLK`, `DEL.BLK`, `INS.BLK.POST`) live behind the
//! `block-ops` feature gate and are not recognized here.
//!
//! Ported from omp `packages/hashline/src/tokenizer.ts`.

use crate::format::{
    HL_DELETE_KEYWORD, HL_FILE_HASH_LENGTH, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX,
    HL_HEADER_COLON, HL_INSERT_AFTER, HL_INSERT_BEFORE, HL_INSERT_HEAD, HL_INSERT_KEYWORD,
    HL_INSERT_TAIL, HL_PAYLOAD_REPLACE, HL_REPLACE_KEYWORD,
};
use crate::messages::{ABORT_MARKER, BEGIN_PATCH_MARKER, END_PATCH_MARKER};
use crate::mismatch::HashlineError;
use crate::types::{Anchor, Cursor, ParsedRange};

// ── Byte-level predicates ────────────────────────────────────────────────

#[inline]
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}
#[inline]
fn is_nonzero_digit(b: u8) -> bool {
    (b'1'..=b'9').contains(&b)
}
#[inline]
fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}
/// omp `isWhitespaceCode`: space, or 0x09–0x0d (tab, LF, VT, FF, CR).
#[inline]
fn is_ws(b: u8) -> bool {
    b == b' ' || (b'\t'..=b'\r').contains(&b)
}

fn skip_ws(bytes: &[u8], mut idx: usize, end: usize) -> usize {
    while idx < end && is_ws(bytes[idx]) {
        idx += 1;
    }
    idx
}

/// Index of the first non-trailing-whitespace byte (`bytes.len()` if all ws).
fn trim_end(bytes: &[u8]) -> usize {
    let mut end = bytes.len();
    while end > 0 && is_ws(bytes[end - 1]) {
        end -= 1;
    }
    end
}

fn marker_line_equals(line: &str, marker: &str) -> bool {
    let bytes = line.as_bytes();
    let end = trim_end(bytes);
    end == marker.len() && bytes[..end] == *marker.as_bytes()
}

// ── Line splitting ───────────────────────────────────────────────────────

/// Split `text` into lines on `\n`, stripping a trailing `\r` from each.
/// An empty input yields a single empty line (omp `splitHashlineLines`).
pub fn split_hashline_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'\n' {
            continue;
        }
        let mut stop = i;
        if stop > start && bytes[stop - 1] == b'\r' {
            stop -= 1;
        }
        lines.push(text[start..stop].to_string());
        start = i + 1;
    }
    if start < bytes.len() {
        let mut stop = bytes.len();
        if stop > start && bytes[stop - 1] == b'\r' {
            stop -= 1;
        }
        lines.push(text[start..stop].to_string());
    }
    lines
}

/// `Cursor` carries only a `Copy` [`Anchor`]; cloning is cheap. Provided for
/// API parity with omp `cloneCursor` — idiomatic Rust callers reach for
/// `Cursor::clone` directly.
pub fn clone_cursor(cursor: &Cursor) -> Cursor {
    cursor.clone()
}

// ── Number / range scanning ──────────────────────────────────────────────

struct NumberScan {
    line: u32,
    next: usize,
}

fn scan_line_number(bytes: &[u8], idx: usize, end: usize) -> Option<NumberScan> {
    if idx >= end || !is_nonzero_digit(bytes[idx]) {
        return None;
    }
    let mut line: u32 = 0;
    let mut next = idx;
    while next < end && is_digit(bytes[next]) {
        line = line
            .checked_mul(10)?
            .checked_add((bytes[next] - b'0') as u32)?;
        next += 1;
    }
    Some(NumberScan { line, next })
}

/// Parse a bare line-number anchor. Errors on malformed input.
pub fn parse_lid(raw: &str, line_num: u32) -> Result<Anchor, HashlineError> {
    let bytes = raw.as_bytes();
    let end = trim_end(bytes);
    let number_start = skip_ws(bytes, 0, end);
    let number = scan_line_number(bytes, number_start, end)
        .ok_or_else(|| HashlineError::parse(line_num, expected_lid_message(raw)))?;
    if skip_ws(bytes, number.next, end) != end {
        return Err(HashlineError::parse(line_num, expected_lid_message(raw)));
    }
    Ok(Anchor { line: number.line })
}

fn expected_lid_message(raw: &str) -> String {
    format!(
        "expected a line number such as {examples}; got `{raw}`. \
         Use `{p}PATH{s}hash{e}` from your latest read for file-version binding.",
        examples = crate::messages::describe_anchor_examples("119"),
        p = HL_FILE_PREFIX,
        s = HL_FILE_HASH_SEP,
        e = HL_FILE_SUFFIX,
    )
}

struct RangeScan {
    range: ParsedRange,
    next: usize,
}

/// Scan the range separator (`..`, `.=`, `-`, `…`, or whitespace) between two
/// line numbers. Returns the index where the end-number begins.
fn scan_range_separator(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut cursor = start;
    let mut consumed = false;
    let ellipsis = "\u{2026}".as_bytes(); // …  (U+2026, 3 UTF-8 bytes)
    while cursor < end {
        let b = bytes[cursor];
        if is_ws(b) {
            cursor += 1;
            consumed = true;
            continue;
        }
        if b == b'-' {
            cursor += 1;
            consumed = true;
            continue;
        }
        if bytes[cursor..end].starts_with(ellipsis) {
            cursor += ellipsis.len();
            consumed = true;
            continue;
        }
        if b == b'.' && cursor + 1 < end && (bytes[cursor + 1] == b'.' || bytes[cursor + 1] == b'=')
        {
            cursor += 2;
            consumed = true;
            continue;
        }
        break;
    }
    if !consumed {
        return None;
    }
    if cursor >= end || !is_nonzero_digit(bytes[cursor]) {
        return None;
    }
    Some(cursor)
}

/// Parse a `start.=end` range; with `allow_single` a bare `N` yields `{N, N}`.
fn scan_header_range(
    bytes: &[u8],
    idx: usize,
    end: usize,
    allow_single: bool,
) -> Option<RangeScan> {
    let number_start = skip_ws(bytes, idx, end);
    let start = scan_line_number(bytes, number_start, end)?;
    match scan_range_separator(bytes, start.next, end) {
        None => {
            if !allow_single {
                return None;
            }
            Some(RangeScan {
                range: ParsedRange {
                    start: start.line,
                    end: start.line,
                },
                next: skip_ws(bytes, start.next, end),
            })
        }
        Some(after_first) => {
            let end_num = scan_line_number(bytes, after_first, end)?;
            Some(RangeScan {
                range: ParsedRange {
                    start: start.line,
                    end: end_num.line,
                },
                next: skip_ws(bytes, end_num.next, end),
            })
        }
    }
}

// ── Hunk anchor scanning ─────────────────────────────────────────────────

/// Where a hunk header lands. Line-ops only in the default build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockTarget {
    /// `SWAP start.=end:` — replace the inclusive range.
    Replace { range: ParsedRange },
    /// `DEL start` or `DEL start.=end` — delete the inclusive range (no body).
    Delete { range: ParsedRange },
    /// `INS.PRE N:` — insert before line N.
    InsertBefore { anchor: Anchor },
    /// `INS.POST N:` — insert after line N.
    InsertAfter { anchor: Anchor },
    /// `INS.HEAD:` — insert at the very top.
    Bof,
    /// `INS.TAIL:` — insert at the very bottom.
    Eof,
}

struct TargetScan {
    target: BlockTarget,
    next: usize,
}

/// Match a keyword at `idx`; the byte after it must be ws, `:`, or `.` (so
/// `SWAP` does not match `SWAPPER`).
fn scan_keyword(bytes: &[u8], idx: usize, end: usize, keyword: &[u8]) -> Option<usize> {
    if !bytes[idx..end].starts_with(keyword) {
        return None;
    }
    let next = idx + keyword.len();
    if next < end {
        let b = bytes[next];
        if !is_ws(b) && b != HL_HEADER_COLON as u8 && b != b'.' {
            return None;
        }
    }
    Some(next)
}

/// Skip optional trailing whitespace + colon + whitespace.
fn consume_optional_colon(bytes: &[u8], idx: usize, end: usize) -> usize {
    let cursor = skip_ws(bytes, idx, end);
    if cursor < end && bytes[cursor] == HL_HEADER_COLON as u8 {
        skip_ws(bytes, cursor + 1, end)
    } else {
        cursor
    }
}

/// Parse the `.PRE N` / `.POST N` / `.HEAD` / `.TAIL` tail of an `INS` header.
fn scan_insert_target(bytes: &[u8], idx: usize, end: usize) -> Option<TargetScan> {
    if idx >= end || bytes[idx] != b'.' {
        return None;
    }
    let cursor = skip_ws(bytes, idx + 1, end);
    if let Some(e) = scan_keyword(bytes, cursor, end, HL_INSERT_BEFORE.as_bytes()) {
        let anchor = scan_line_number(bytes, skip_ws(bytes, e, end), end)?;
        return Some(TargetScan {
            target: BlockTarget::InsertBefore {
                anchor: Anchor { line: anchor.line },
            },
            next: consume_optional_colon(bytes, anchor.next, end),
        });
    }
    if let Some(e) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER.as_bytes()) {
        let anchor = scan_line_number(bytes, skip_ws(bytes, e, end), end)?;
        return Some(TargetScan {
            target: BlockTarget::InsertAfter {
                anchor: Anchor { line: anchor.line },
            },
            next: consume_optional_colon(bytes, anchor.next, end),
        });
    }
    if let Some(e) = scan_keyword(bytes, cursor, end, HL_INSERT_HEAD.as_bytes()) {
        return Some(TargetScan {
            target: BlockTarget::Bof,
            next: consume_optional_colon(bytes, e, end),
        });
    }
    if let Some(e) = scan_keyword(bytes, cursor, end, HL_INSERT_TAIL.as_bytes()) {
        return Some(TargetScan {
            target: BlockTarget::Eof,
            next: consume_optional_colon(bytes, e, end),
        });
    }
    None
}

/// Parse the verb + target of a hunk header line.
fn scan_hunk_anchor(bytes: &[u8], start: usize, end: usize) -> Option<TargetScan> {
    let cursor = skip_ws(bytes, start, end);

    if let Some(e) = scan_keyword(bytes, cursor, end, HL_REPLACE_KEYWORD.as_bytes()) {
        let range = scan_header_range(bytes, e, end, true)?;
        return Some(TargetScan {
            target: BlockTarget::Replace { range: range.range },
            next: consume_optional_colon(bytes, range.next, end),
        });
    }
    if let Some(e) = scan_keyword(bytes, cursor, end, HL_DELETE_KEYWORD.as_bytes()) {
        let range = scan_header_range(bytes, e, end, true)?;
        let next = skip_ws(bytes, range.next, end);
        // `DEL` takes no body and no trailing colon.
        if next < end && bytes[next] == HL_HEADER_COLON as u8 {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::Delete { range: range.range },
            next,
        });
    }
    if let Some(e) = scan_keyword(bytes, cursor, end, HL_INSERT_KEYWORD.as_bytes()) {
        return scan_insert_target(bytes, e, end);
    }
    None
}

fn try_parse_hunk_header(line: &str) -> Option<BlockTarget> {
    let bytes = line.as_bytes();
    let end = trim_end(bytes);
    let start = skip_ws(bytes, 0, end);
    if start >= end {
        return None;
    }
    let scan = scan_hunk_anchor(bytes, start, end)?;
    if scan.next != end {
        return None;
    }
    Some(scan.target)
}

// ── Section header scanning ──────────────────────────────────────────────

struct HeaderScan {
    path: String,
    file_hash: Option<String>,
}

/// Parse a `[PATH]` or `[PATH#HASH]` section header line. Returns `None` for
/// lines that are not bracketed headers, and for bracketed lines whose
/// interior is malformed (embedded `#`, bad-length/non-hex tag, etc.).
fn try_parse_header(line: &str) -> Option<HeaderScan> {
    let bytes = line.as_bytes();
    if !bytes.starts_with(HL_FILE_PREFIX.as_bytes()) {
        return None;
    }
    let end = trim_end(bytes);
    if HL_FILE_PREFIX.len() + HL_FILE_SUFFIX.len() >= end {
        return None;
    }
    if !bytes[..end].ends_with(HL_FILE_SUFFIX.as_bytes()) {
        return None;
    }
    let body_end = end - HL_FILE_SUFFIX.len();
    if HL_FILE_PREFIX.len() >= body_end {
        return None;
    }

    // A trailing `#XXXX` (4 hex) is the snapshot tag, anchored at the body end
    // so the path may legitimately contain whitespace.
    let mut path_end = body_end;
    let mut file_hash = None;
    let trailing_hash_start = body_end.saturating_sub(HL_FILE_HASH_LENGTH + 1);
    if trailing_hash_start >= HL_FILE_PREFIX.len() && bytes[trailing_hash_start] == b'#' {
        let mut all_hex = true;
        for &byte in &bytes[(trailing_hash_start + 1)..body_end] {
            if !is_hex_digit(byte) {
                all_hex = false;
                break;
            }
        }
        if all_hex {
            path_end = trailing_hash_start;
            // Slice is 4 ASCII hex chars — char-safe boundary.
            file_hash = Some(line[(trailing_hash_start + 1)..body_end].to_uppercase());
        }
    }

    // `#` is the path/tag separator and is not allowed inside the path body.
    for &byte in &bytes[HL_FILE_PREFIX.len()..path_end] {
        if byte == b'#' {
            return None;
        }
    }
    if path_end == HL_FILE_PREFIX.len() {
        return None;
    }
    let path = line[HL_FILE_PREFIX.len()..path_end].to_string();
    Some(HeaderScan { path, file_hash })
}

// ── Token type ───────────────────────────────────────────────────────────

/// One classified line of patch text.
#[derive(Debug, Clone)]
pub enum Token {
    /// An empty line.
    Blank { line_num: u32 },
    /// `*** Begin Patch` — envelope start, consumed.
    EnvelopeBegin { line_num: u32 },
    /// `*** End Patch` — envelope end, terminates parsing.
    EnvelopeEnd { line_num: u32 },
    /// `*** Abort` — truncation sentinel, terminates parsing.
    Abort { line_num: u32 },
    /// `[PATH]` or `[PATH#HASH]` section header.
    Header {
        line_num: u32,
        path: String,
        file_hash: Option<String>,
    },
    /// A hunk header verb + target.
    Op { line_num: u32, target: BlockTarget },
    /// A `+TEXT` body row.
    PayloadLiteral { line_num: u32, text: String },
    /// Anything else (contamination check happens downstream).
    Raw { line_num: u32, text: String },
}

impl Token {
    /// 1-indexed line number in the source patch text.
    pub fn line_num(&self) -> u32 {
        match self {
            Token::Blank { line_num }
            | Token::EnvelopeBegin { line_num }
            | Token::EnvelopeEnd { line_num }
            | Token::Abort { line_num }
            | Token::Header { line_num, .. }
            | Token::Op { line_num, .. }
            | Token::PayloadLiteral { line_num, .. }
            | Token::Raw { line_num, .. } => *line_num,
        }
    }
}

pub(crate) fn classify_line(line: &str, line_num: u32) -> Token {
    if line.is_empty() {
        return Token::Blank { line_num };
    }
    if marker_line_equals(line, BEGIN_PATCH_MARKER) {
        return Token::EnvelopeBegin { line_num };
    }
    if marker_line_equals(line, END_PATCH_MARKER) {
        return Token::EnvelopeEnd { line_num };
    }
    if marker_line_equals(line, ABORT_MARKER) {
        return Token::Abort { line_num };
    }
    if line.starts_with(HL_FILE_PREFIX)
        && let Some(header) = try_parse_header(line)
    {
        return Token::Header {
            line_num,
            path: header.path,
            file_hash: header.file_hash,
        };
    }
    let bytes = line.as_bytes();
    let lead = skip_ws(bytes, 0, bytes.len());
    let is_hunk_lead = line[lead..].starts_with(HL_REPLACE_KEYWORD)
        || line[lead..].starts_with(HL_DELETE_KEYWORD)
        || line[lead..].starts_with(HL_INSERT_KEYWORD);
    if is_hunk_lead && let Some(target) = try_parse_hunk_header(line) {
        return Token::Op { line_num, target };
    }
    if bytes.first().copied() == Some(HL_PAYLOAD_REPLACE as u8) {
        return Token::PayloadLiteral {
            line_num,
            text: line[1..].to_string(),
        };
    }
    Token::Raw {
        line_num,
        text: line.to_string(),
    }
}

// ── Streaming Tokenizer ──────────────────────────────────────────────────

/// Stateful, reusable line classifier. Buffer text with [`feed`], flush the
/// remainder with [`end`], and reset with [`reset`] before reuse.
///
/// [`feed`]: Tokenizer::feed
/// [`end`]: Tokenizer::end
/// [`reset`]: Tokenizer::reset
#[derive(Debug)]
pub struct Tokenizer {
    buffer: String,
    next_line_num: u32,
    closed: bool,
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Tokenizer {
    /// Construct a fresh tokenizer at line 1.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            next_line_num: 1,
            closed: false,
        }
    }

    /// Feed a chunk and return all complete-line tokens. A partial trailing
    /// line (no trailing `\n`) is buffered until the next [`feed`] or [`end`].
    /// No-op once [`end`] has been called; call [`reset`] to reuse.
    pub fn feed(&mut self, chunk: &str) -> Vec<Token> {
        if self.closed || chunk.is_empty() {
            return Vec::new();
        }
        self.buffer.push_str(chunk);
        self.drain_complete_lines()
    }

    /// Flush any buffered partial line as a final token.
    pub fn end(&mut self) -> Vec<Token> {
        if self.closed {
            return Vec::new();
        }
        self.closed = true;
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let bytes = self.buffer.as_bytes();
        let mut stop = bytes.len();
        if stop > 0 && bytes[stop - 1] == b'\r' {
            stop -= 1;
        }
        let token = classify_line(&self.buffer[..stop], self.next_line_num);
        self.next_line_num = self.next_line_num.wrapping_add(1);
        self.buffer.clear();
        vec![token]
    }

    /// Return to a fresh state for reuse.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.next_line_num = 1;
        self.closed = false;
    }

    /// Tokenize an entire input in one shot.
    pub fn tokenize_all(&mut self, text: &str) -> Vec<Token> {
        self.reset();
        let mut tokens = self.feed(text);
        tokens.extend(self.end());
        tokens
    }

    /// Classify a single line (no state).
    pub fn tokenize(&self, line: &str, line_num: u32) -> Token {
        classify_line(line, line_num)
    }

    /// True when `line` parses as a hunk header verb.
    pub fn is_op(&self, line: &str) -> bool {
        try_parse_hunk_header(line).is_some()
    }

    /// True when `line` parses as a `[PATH(#HASH)?]` header.
    pub fn is_header(&self, line: &str) -> bool {
        try_parse_header(line).is_some()
    }

    /// True when `line` is an envelope begin/end/abort marker.
    pub fn is_envelope_marker(&self, line: &str) -> bool {
        marker_line_equals(line, BEGIN_PATCH_MARKER)
            || marker_line_equals(line, END_PATCH_MARKER)
            || marker_line_equals(line, ABORT_MARKER)
    }

    fn drain_complete_lines(&mut self) -> Vec<Token> {
        let bytes = self.buffer.as_bytes();
        let mut tokens = Vec::new();
        let mut start = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b != b'\n' {
                continue;
            }
            let mut stop = i;
            if stop > start && bytes[stop - 1] == b'\r' {
                stop -= 1;
            }
            let line = self.buffer[start..stop].to_string();
            tokens.push(classify_line(&line, self.next_line_num));
            self.next_line_num = self.next_line_num.wrapping_add(1);
            start = i + 1;
        }
        if start == 0 {
            return tokens;
        }
        // Drop consumed prefix; keep the remainder buffered.
        let remainder = self.buffer.split_off(start);
        self.buffer = remainder;
        tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_strips_cr() {
        assert_eq!(split_hashline_lines(""), vec![""]);
        assert_eq!(split_hashline_lines("a\nb"), vec!["a", "b"]);
        assert_eq!(split_hashline_lines("a\r\nb\r\n"), vec!["a", "b"]);
        assert_eq!(split_hashline_lines("a\nb"), vec!["a", "b"]);
        // trailing partial line kept
        assert_eq!(split_hashline_lines("a\nb\nc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn parses_header_with_hash() {
        let h = try_parse_header("[src/foo.ts#1A2B]").unwrap();
        assert_eq!(h.path, "src/foo.ts");
        assert_eq!(h.file_hash.as_deref(), Some("1A2B"));
    }

    #[test]
    fn parses_header_without_hash() {
        let h = try_parse_header("[src/foo.ts]").unwrap();
        assert_eq!(h.path, "src/foo.ts");
        assert!(h.file_hash.is_none());
    }

    #[test]
    fn uppercases_hex_tag() {
        let h = try_parse_header("[a#1a2b]").unwrap();
        assert_eq!(h.file_hash.as_deref(), Some("1A2B"));
    }

    #[test]
    fn rejects_embedded_hash_in_path() {
        assert!(try_parse_header("[a#1A2#b]").is_none());
        assert!(try_parse_header("[a#1A2G]").is_none()); // non-hex
        assert!(try_parse_header("[a#1A2]").is_none()); // too short
        assert!(try_parse_header("[a#1A2B5]").is_none()); // too long
    }

    #[test]
    fn path_with_spaces_ok() {
        let h = try_parse_header("[OneDrive - Co/x.ts#1A2B]").unwrap();
        assert_eq!(h.path, "OneDrive - Co/x.ts");
    }

    #[test]
    fn parses_swap_range() {
        let t = try_parse_hunk_header("SWAP 5.=10:").unwrap();
        assert_eq!(
            t,
            BlockTarget::Replace {
                range: ParsedRange { start: 5, end: 10 }
            }
        );
    }

    #[test]
    fn parses_swap_single() {
        let t = try_parse_hunk_header("SWAP 5:").unwrap();
        assert_eq!(
            t,
            BlockTarget::Replace {
                range: ParsedRange { start: 5, end: 5 }
            }
        );
    }

    #[test]
    fn parses_delete_range_and_single() {
        assert_eq!(
            try_parse_hunk_header("DEL 3.=7"),
            Some(BlockTarget::Delete {
                range: ParsedRange { start: 3, end: 7 }
            })
        );
        assert_eq!(
            try_parse_hunk_header("DEL 3"),
            Some(BlockTarget::Delete {
                range: ParsedRange { start: 3, end: 3 }
            })
        );
    }

    #[test]
    fn delete_rejects_colon() {
        assert!(try_parse_hunk_header("DEL 3.=7:").is_none());
    }

    #[test]
    fn parses_insert_variants() {
        assert_eq!(
            try_parse_hunk_header("INS.PRE 5:"),
            Some(BlockTarget::InsertBefore {
                anchor: Anchor { line: 5 }
            })
        );
        assert_eq!(
            try_parse_hunk_header("INS.POST 5:"),
            Some(BlockTarget::InsertAfter {
                anchor: Anchor { line: 5 }
            })
        );
        assert_eq!(try_parse_hunk_header("INS.HEAD:"), Some(BlockTarget::Bof));
        assert_eq!(try_parse_hunk_header("INS.TAIL:"), Some(BlockTarget::Eof));
    }

    #[test]
    fn classify_envelope_and_payload() {
        assert!(matches!(
            classify_line("*** Begin Patch", 1),
            Token::EnvelopeBegin { .. }
        ));
        assert!(matches!(
            classify_line("*** End Patch", 1),
            Token::EnvelopeEnd { .. }
        ));
        assert!(matches!(classify_line("*** Abort", 1), Token::Abort { .. }));
        assert!(matches!(
            classify_line("+hello", 1),
            Token::PayloadLiteral { text, .. } if text == "hello"
        ));
        assert!(matches!(classify_line("", 1), Token::Blank { .. }));
        assert!(matches!(
            classify_line("# comment", 1),
            Token::Raw { text, .. } if text == "# comment"
        ));
    }

    #[test]
    fn tokenize_all_full_flow() {
        let mut tok = Tokenizer::new();
        let toks = tok.tokenize_all("[a.ts#1A2B]\nSWAP 1.=2:\n+x\n");
        assert_eq!(toks.len(), 3);
        assert!(matches!(toks[0], Token::Header { .. }));
        assert!(matches!(toks[1], Token::Op { .. }));
        assert!(matches!(toks[2], Token::PayloadLiteral { .. }));
        // line numbers ascend from 1
        assert_eq!(toks[0].line_num(), 1);
        assert_eq!(toks[1].line_num(), 2);
        assert_eq!(toks[2].line_num(), 3);
    }

    #[test]
    fn parse_lid_valid_and_invalid() {
        assert_eq!(parse_lid("42", 1).unwrap(), Anchor { line: 42 });
        assert!(parse_lid("x", 1).is_err());
        assert!(parse_lid("42x", 1).is_err());
    }
}
