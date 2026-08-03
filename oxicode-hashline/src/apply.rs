//! Apply a parsed list of [`Edit`]s to a text body and return the post-edit
//! text plus diagnostic warnings.
//!
//! Replacement groups are first normalized by boundary repair, which absorbs
//! common model mistakes where a payload restates unchanged range boundaries
//! or duplicates/drops structural closers. After-insert landings are then
//! corrected when a body's indentation claims a depth different from its
//! anchor's.
//!
//! Ported from omp `packages/hashline/src/apply.ts`.

use crate::mismatch::HashlineError;
use crate::types::{Anchor, ApplyResult, Cursor, Edit, InsertMode};
use std::collections::{BTreeMap, HashSet};

// ═══════════════════════════════════════════════════════════════════════════
// Delimiter balance
// ═══════════════════════════════════════════════════════════════════════════

/// Net `()` / `[]` / `{}` delta across a set of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct DelimiterBalance {
    paren: i32,
    bracket: i32,
    brace: i32,
}

/// Net `()` / `[]` / `{}` delta across `lines`, skipping delimiters inside line
/// comments (`//`), block comments, and string/template literals. Block-comment
/// and backtick-template state carry across lines; `"` / `'` reset at EOL since
/// they cannot span lines.
///
/// Byte-scanned: all relevant delimiter characters are ASCII, and UTF-8
/// guarantees multi-byte continuation bytes are ≥ 0x80, so they never collide
/// with the ASCII bytes we test.
fn compute_delimiter_balance(lines: &[String]) -> DelimiterBalance {
    let mut bal = DelimiterBalance::default();
    let mut in_block_comment = false;
    let mut quote: Option<u8> = None;

    for line in lines {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let ch = bytes[i];
            if in_block_comment {
                if ch == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    in_block_comment = false;
                    i += 1;
                }
                i += 1;
                continue;
            }
            if let Some(q) = quote {
                if ch == b'\\' {
                    i += 2; // skip backslash + escaped char
                } else if ch == q {
                    quote = None;
                    i += 1;
                } else {
                    i += 1;
                }
                continue;
            }
            match ch {
                b'"' | b'\'' | b'`' => {
                    quote = Some(ch);
                    i += 1;
                }
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => break, // line comment
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                    in_block_comment = true;
                    i += 2;
                }
                b'(' => {
                    bal.paren += 1;
                    i += 1;
                }
                b')' => {
                    bal.paren -= 1;
                    i += 1;
                }
                b'[' => {
                    bal.bracket += 1;
                    i += 1;
                }
                b']' => {
                    bal.bracket -= 1;
                    i += 1;
                }
                b'{' => {
                    bal.brace += 1;
                    i += 1;
                }
                b'}' => {
                    bal.brace -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        // `"` / `'` cannot span lines; only backtick templates and block comments do.
        if quote == Some(b'"') || quote == Some(b'\'') {
            quote = None;
        }
    }
    bal
}

fn balance_delta(a: DelimiterBalance, b: DelimiterBalance) -> DelimiterBalance {
    DelimiterBalance {
        paren: a.paren - b.paren,
        bracket: a.bracket - b.bracket,
        brace: a.brace - b.brace,
    }
}

fn balance_negate(a: DelimiterBalance) -> DelimiterBalance {
    DelimiterBalance {
        paren: -a.paren,
        bracket: -a.bracket,
        brace: -a.brace,
    }
}

fn balance_is_zero(a: DelimiterBalance) -> bool {
    a.paren == 0 && a.bracket == 0 && a.brace == 0
}

// ═══════════════════════════════════════════════════════════════════════════
// Closer detection
// ═══════════════════════════════════════════════════════════════════════════

/// Matches omp `STRUCTURAL_CLOSER_RE`: `^\s*[)\]}]+[;,]?\s*$` — a line of
/// nothing but closing brackets, optionally terminated by `;` or `,`.
fn is_bracket_closer_line(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let first = bytes[0];
    if first != b')' && first != b']' && first != b'}' {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b')' | b']' | b'}' => i += 1,
            _ => break,
        }
    }
    if i < bytes.len() && (bytes[i] == b';' || bytes[i] == b',') {
        i += 1;
    }
    i == bytes.len()
}

/// A byte matching `[\w.:-]` (JS without `u` flag): ASCII alphanumeric, `_`,
/// `.`, `:`, `-`.
fn is_jsx_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'-'
}

/// `[A-Za-z][\w.:-]*`
fn is_valid_jsx_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    bytes[1..].iter().all(|&b| is_jsx_name_byte(b))
}

/// Matches omp `JSX_CLOSER_RE`: `^\s*(?:<\/>|<\/Name>|\/>)\s*[;,]?\s*$`.
fn is_jsx_closer_line(text: &str) -> bool {
    let s = strip_optional_trailing_punct(text.trim());
    if s == "</>" || s == "/>" {
        return true;
    }
    if let Some(rest) = s.strip_prefix("</")
        && let Some(name) = rest.strip_suffix('>')
    {
        return is_valid_jsx_name(name);
    }
    false
}

/// A structural closer: bracket closers or JSX closers.
fn is_structural_closer_line(text: &str) -> bool {
    is_bracket_closer_line(text) || is_jsx_closer_line(text)
}

/// omp `jsxCloserName`: `Some("")` for `</>`, `Some("Name")` for `</Name>`,
/// `None` otherwise.
fn jsx_closer_name(text: &str) -> Option<String> {
    let s = strip_optional_trailing_punct(text.trim());
    if s == "</>" {
        return Some(String::new());
    }
    let rest = s.strip_prefix("</")?;
    let name = rest.strip_suffix('>')?;
    if is_valid_jsx_name(name) {
        Some(name.to_string())
    } else {
        None
    }
}

/// Remove an optional trailing `;` or `,` (with any whitespace before it)
/// from an already-trimmed string.
fn strip_optional_trailing_punct(s: &str) -> &str {
    if s.ends_with(';') || s.ends_with(',') {
        s[..s.len() - 1].trim_end()
    } else {
        s
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// JSX tag parsing (for single-line one-sided echo guard)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
struct JsxPayloadTag {
    name: String,
    closing: bool,
    self_closing: bool,
}

fn is_jsx_tag_start(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index + 1) {
        Some(&b) => b == b'>' || b == b'/' || b.is_ascii_alphabetic(),
        None => false,
    }
}

/// Find the `>` that closes the JSX tag starting at `start`, respecting
/// string literals and `{...}` expression braces. Returns a byte index or
/// `None`.
fn find_jsx_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut braces = 0i32;
    let mut i = start + 1;
    while i < bytes.len() {
        let ch = bytes[i];
        if let Some(q) = quote {
            if ch == b'\\' && i + 1 < bytes.len() {
                i += 1; // skip escaped char
            } else if ch == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match ch {
            b'"' | b'\'' | b'`' => quote = Some(ch),
            b'{' => braces += 1,
            b'}' if braces > 0 => braces -= 1,
            b'>' if braces == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_jsx_payload_tag(raw: &str) -> Option<JsxPayloadTag> {
    if raw == "<>" {
        return Some(JsxPayloadTag {
            name: String::new(),
            closing: false,
            self_closing: false,
        });
    }
    if raw == "</>" {
        return Some(JsxPayloadTag {
            name: String::new(),
            closing: true,
            self_closing: false,
        });
    }
    let closing = raw.starts_with("</");
    let name_start = if closing { 2 } else { 1 };
    let bytes = raw.as_bytes();
    let mut name_end = name_start;
    while name_end < bytes.len() && is_jsx_name_byte(bytes[name_end]) {
        name_end += 1;
    }
    if name_end == name_start {
        return None;
    }
    let name = raw[name_start..name_end].to_string();
    let self_closing = !closing && raw.ends_with("/>");
    Some(JsxPayloadTag {
        name,
        closing,
        self_closing,
    })
}

fn read_jsx_payload_tags(text: &str) -> Vec<JsxPayloadTag> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut pos = 0;
    while let Some(p) = bytes[pos..].iter().position(|&b| b == b'<') {
        let start = pos + p;
        pos = start + 1;
        if !is_jsx_tag_start(bytes, start) {
            continue;
        }
        let end = match find_jsx_tag_end(text, start) {
            Some(e) => e,
            None => break,
        };
        let raw = &text[start..=end];
        if let Some(tag) = parse_jsx_payload_tag(raw) {
            tags.push(tag);
        }
        pos = end + 1;
    }
    tags
}

/// Whether the payload prefix opens a JSX tag that one of the echo closers
/// would close.
fn payload_has_jsx_opener_for_echo(payload_prefix: &[String], echo_lines: &[String]) -> bool {
    let joined = payload_prefix.join("\n");
    let mut open_tags: Vec<String> = Vec::new();
    for tag in read_jsx_payload_tags(&joined) {
        if tag.closing {
            if open_tags.last().map(|n| n == &tag.name).unwrap_or(false) {
                open_tags.pop();
            }
        } else if !tag.self_closing {
            open_tags.push(tag.name);
        }
    }
    echo_lines
        .iter()
        .any(|line| jsx_closer_name(line).is_some_and(|name| open_tags.contains(&name)))
}

// ═══════════════════════════════════════════════════════════════════════════
// Replacement group detection
// ═══════════════════════════════════════════════════════════════════════════

/// A run of replacement-mode inserts sharing one source op line, immediately
/// followed by the contiguous range deletes for that same op.
struct ReplacementGroup {
    /// Positions in the edit array of the payload inserts, in payload order.
    insert_indices: Vec<usize>,
    /// Positions in the edit array of the range deletes, ascending by line.
    delete_indices: Vec<usize>,
    payload: Vec<String>,
    /// First deleted line (1-indexed).
    start_line: u32,
    /// Last deleted line (1-indexed).
    end_line: u32,
}

/// Detect a replacement group starting at `start`.
fn find_replacement_group(edits: &[Edit], start: usize) -> Option<ReplacementGroup> {
    let first = edits.get(start)?;
    let (line_num, anchor_line) = match first {
        Edit::Insert {
            mode: Some(InsertMode::Replacement),
            cursor: Cursor::BeforeAnchor(anchor),
            line_num,
            ..
        } => (*line_num, anchor.line),
        _ => return None,
    };

    let mut insert_indices = Vec::new();
    let mut payload = Vec::new();
    let mut i = start;
    while i < edits.len() {
        let edit = &edits[i];
        match edit {
            Edit::Insert {
                mode: Some(InsertMode::Replacement),
                cursor: Cursor::BeforeAnchor(a),
                line_num: ln,
                text,
                ..
            } if *ln == line_num && a.line == anchor_line => {
                insert_indices.push(i);
                payload.push(text.clone());
                i += 1;
            }
            _ => break,
        }
    }

    let mut delete_indices = Vec::new();
    let mut expected_line = anchor_line;
    while i < edits.len() {
        let edit = &edits[i];
        match edit {
            Edit::Delete {
                anchor,
                line_num: ln,
                ..
            } if *ln == line_num && anchor.line == expected_line => {
                delete_indices.push(i);
                expected_line += 1;
                i += 1;
            }
            _ => break,
        }
    }

    if delete_indices.is_empty() {
        return None;
    }
    let end_line = anchor_line + delete_indices.len() as u32 - 1;

    Some(ReplacementGroup {
        insert_indices,
        delete_indices,
        payload,
        start_line: anchor_line,
        end_line,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Boundary echo detection
// ═══════════════════════════════════════════════════════════════════════════

fn has_non_whitespace(text: &str) -> bool {
    text.bytes()
        .any(|b| !matches!(b, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' '))
}

/// Largest `count` such that the payload's first `count` lines exactly equal
/// the `count` surviving file lines just above the range, with at least one
/// non-whitespace line among them.
fn count_duplicate_leading_boundary_lines(
    group: &ReplacementGroup,
    file_lines: &[String],
) -> usize {
    let payload_len = group.payload.len();
    let max = payload_len.min((group.start_line - 1) as usize);
    for count in (1..=max).rev() {
        let mut matches = true;
        let mut has_content = false;
        for offset in 0..count {
            let payload_line = &group.payload[offset];
            let file_idx = (group.start_line - 1) as usize - count + offset;
            if payload_line != &file_lines[file_idx] {
                matches = false;
                break;
            }
            if has_non_whitespace(payload_line) {
                has_content = true;
            }
        }
        if matches && has_content {
            return count;
        }
    }
    0
}

/// Largest `count` such that the payload's last `count` lines exactly equal
/// the `count` surviving file lines just below the range, with at least one
/// non-whitespace line among them.
fn count_duplicate_trailing_boundary_lines(
    group: &ReplacementGroup,
    file_lines: &[String],
) -> usize {
    let payload_len = group.payload.len();
    let max = payload_len.min(file_lines.len().saturating_sub(group.end_line as usize));
    for count in (1..=max).rev() {
        let mut matches = true;
        let mut has_content = false;
        for offset in 0..count {
            let payload_idx = payload_len - count + offset;
            let payload_line = &group.payload[payload_idx];
            let file_idx = group.end_line as usize + offset;
            if payload_line != &file_lines[file_idx] {
                matches = false;
                break;
            }
            if has_non_whitespace(payload_line) {
                has_content = true;
            }
        }
        if matches && has_content {
            return count;
        }
    }
    0
}

struct BoundaryEcho {
    leading: usize,
    trailing: usize,
}

/// Two-sided boundary echo: the payload restates unchanged lines on BOTH sides
/// of the range. Balance-neutral unless the dropped echo exactly explains the
/// payload/range delta.
fn find_boundary_echo(group: &ReplacementGroup, file_lines: &[String]) -> Option<BoundaryEcho> {
    let leading_max = count_duplicate_leading_boundary_lines(group, file_lines);
    if leading_max == 0 {
        return None;
    }
    let trailing_max = count_duplicate_trailing_boundary_lines(group, file_lines);
    if trailing_max == 0 {
        return None;
    }
    if leading_max + trailing_max >= group.payload.len() {
        return None;
    }

    let leading_balance = compute_delimiter_balance(&group.payload[..leading_max]);
    let trailing_balance =
        compute_delimiter_balance(&group.payload[group.payload.len() - trailing_max..]);
    let dropped_balance = balance_delta(leading_balance, balance_negate(trailing_balance));

    if !balance_is_zero(dropped_balance) {
        let delta = balance_delta(
            compute_delimiter_balance(&group.payload),
            compute_delimiter_balance(
                &file_lines[(group.start_line - 1) as usize..group.end_line as usize],
            ),
        );
        if dropped_balance != delta {
            return None;
        }
    }
    Some(BoundaryEcho {
        leading: leading_max,
        trailing: trailing_max,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// One-sided boundary echo
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EchoSide {
    Leading,
    Trailing,
}

struct OneSidedEcho {
    side: EchoSide,
    count: usize,
}

/// A single-sided boundary echo in an otherwise delimiter-balanced multi-line
/// replacement: the payload's leading XOR trailing edge restates surviving
/// line(s) just outside the range. Single-line ranges are only repaired when
/// the edge is a trailing structural closer.
fn find_one_sided_boundary_echo(
    group: &ReplacementGroup,
    file_lines: &[String],
) -> Option<OneSidedEcho> {
    let leading = count_duplicate_leading_boundary_lines(group, file_lines);
    let trailing = count_duplicate_trailing_boundary_lines(group, file_lines);
    if (leading > 0) == (trailing > 0) {
        return None;
    }
    let (side, count) = if leading > 0 {
        (EchoSide::Leading, leading)
    } else {
        (EchoSide::Trailing, trailing)
    };
    if count >= group.payload.len() {
        return None;
    }
    let echo_lines: &[String] = match side {
        EchoSide::Leading => &group.payload[..count],
        EchoSide::Trailing => &group.payload[group.payload.len() - count..],
    };
    if !balance_is_zero(compute_delimiter_balance(echo_lines)) {
        return None;
    }
    if group.delete_indices.len() <= 1 {
        if side != EchoSide::Trailing {
            return None;
        }
        if !echo_lines.iter().all(|l| is_structural_closer_line(l)) {
            return None;
        }
        let payload_prefix = &group.payload[..group.payload.len() - count];
        if payload_has_jsx_opener_for_echo(payload_prefix, echo_lines) {
            return None;
        }
    }
    Some(OneSidedEcho { side, count })
}

// ═══════════════════════════════════════════════════════════════════════════
// Duplicate / dropped-closer detection (delimiter-imbalanced groups)
// ═══════════════════════════════════════════════════════════════════════════

/// Largest `k` such that the payload's last `k` lines equal the surviving file
/// lines just below the range AND dropping them zeroes `delta`.
fn find_duplicate_suffix(
    group: &ReplacementGroup,
    file_lines: &[String],
    delta: DelimiterBalance,
) -> usize {
    if balance_is_zero(delta) {
        return 0;
    }
    let payload_len = group.payload.len();
    let max_k = payload_len.min(file_lines.len().saturating_sub(group.end_line as usize));
    for k in (1..=max_k).rev() {
        let mut matches = true;
        for t in 0..k {
            let payload_idx = payload_len - k + t;
            let file_idx = group.end_line as usize + t;
            if group.payload[payload_idx] != file_lines[file_idx] {
                matches = false;
                break;
            }
        }
        if !matches {
            continue;
        }
        let suffix = &group.payload[payload_len - k..];
        if compute_delimiter_balance(suffix) == delta {
            return k;
        }
    }
    0
}

/// Largest `j` such that the payload's first `j` lines equal the surviving file
/// lines just above the range AND dropping them zeroes `delta`.
fn find_duplicate_prefix(
    group: &ReplacementGroup,
    file_lines: &[String],
    delta: DelimiterBalance,
) -> usize {
    if balance_is_zero(delta) {
        return 0;
    }
    let payload_len = group.payload.len();
    let max_j = payload_len.min((group.start_line - 1) as usize);
    for j in (1..=max_j).rev() {
        let mut matches = true;
        for t in 0..j {
            let file_idx = (group.start_line - 1) as usize - j + t;
            if group.payload[t] != file_lines[file_idx] {
                matches = false;
                break;
            }
        }
        if !matches {
            continue;
        }
        let prefix = &group.payload[..j];
        if compute_delimiter_balance(prefix) == delta {
            return j;
        }
    }
    0
}

fn payload_ends_with_deleted_suffix(
    group: &ReplacementGroup,
    file_lines: &[String],
    count: usize,
) -> bool {
    if group.payload.len() < count {
        return false;
    }
    let deleted_start = group.end_line as usize - count;
    let payload_start = group.payload.len() - count;
    for offset in 0..count {
        if group.payload[payload_start + offset] != file_lines[deleted_start + offset] {
            return false;
        }
    }
    true
}

/// Smallest `m` such that the range's last `m` deleted lines are all structural
/// closers, the payload does not already restate them, and sparing them zeroes
/// `delta`.
fn find_dropped_suffix_closers(
    group: &ReplacementGroup,
    file_lines: &[String],
    delta: DelimiterBalance,
) -> usize {
    let wanted = balance_negate(delta);
    let max_m = group.delete_indices.len();
    for m in 1..=max_m {
        let idx = group.end_line as usize - m;
        let line = file_lines.get(idx).map(|s| s.as_str()).unwrap_or("");
        if !is_bracket_closer_line(line) {
            break;
        }
        if payload_ends_with_deleted_suffix(group, file_lines, m) {
            continue;
        }
        let suffix = &file_lines[group.end_line as usize - m..group.end_line as usize];
        if compute_delimiter_balance(suffix) == wanted {
            return m;
        }
    }
    0
}

// ═══════════════════════════════════════════════════════════════════════════
// Warning messages
// ═══════════════════════════════════════════════════════════════════════════

fn describe_boundary_echo_repair(group: &ReplacementGroup, echo: &BoundaryEcho) -> String {
    format!(
        "Auto-repaired a replacement boundary echo at line {start}: \
         dropped {leading} leading and {trailing} trailing payload line(s) \
         already present outside the range. \
         Issue the payload as the final desired content for the selected range only \
         — never restate unchanged lines bordering the range.",
        start = group.start_line,
        leading = echo.leading,
        trailing = echo.trailing,
    )
}

fn describe_boundary_repair(group: &ReplacementGroup, action: &str) -> String {
    format!(
        "Auto-repaired a delimiter-balance mismatch in the replacement at line {start}: {action}. \
         Issue the payload as the final desired content only \
         — never restate or omit a closing bracket bordering the range.",
        start = group.start_line,
    )
}

fn describe_one_sided_echo_repair(
    group: &ReplacementGroup,
    side: EchoSide,
    count: usize,
) -> String {
    let (side_str, where_str) = match side {
        EchoSide::Leading => ("leading", "above"),
        EchoSide::Trailing => ("trailing", "below"),
    };
    format!(
        "Auto-repaired a replacement boundary echo at line {start}: \
         dropped {count} {side} payload line(s) identical to the surviving line(s) just {where} the range. \
         The range was one line short of the content you retyped — \
         issue the payload as the final content for the selected range only, \
         and widen the range to consume any keeper you restate.",
        start = group.start_line,
        side = side_str,
        where = where_str,
    )
}

fn after_insert_landing_shift_warning(
    anchor_line: u32,
    landing_line: u32,
    crossed: usize,
) -> String {
    let plural = if crossed == 1 { "" } else { "s" };
    format!(
        "INS.POST {anchor}: body indented shallower than the anchor, \
         so the landing moved past {crossed} closing line{plural} to after line {landing}. \
         For the deeper position inside the block, re-issue with the body indented to match.",
        anchor = anchor_line,
        crossed = crossed,
        plural = plural,
        landing = landing_line,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Boundary repair
// ═══════════════════════════════════════════════════════════════════════════

/// Normalize replacement groups so common off-by-one boundaries do not
/// duplicate unchanged surrounding lines or structural closers.
fn repair_replacement_boundaries(
    edits: Vec<Edit>,
    file_lines: &[String],
) -> (Vec<Edit>, Vec<String>) {
    let mut out: Vec<Edit> = Vec::with_capacity(edits.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut i = 0;
    while i < edits.len() {
        let group = match find_replacement_group(&edits, i) {
            Some(g) => g,
            None => {
                out.push(edits[i].clone());
                i += 1;
                continue;
            }
        };
        i = group.delete_indices[group.delete_indices.len() - 1] + 1;

        let push_inserts = |out: &mut Vec<Edit>, edits: &[Edit], range: std::ops::Range<usize>| {
            for &idx in &group.insert_indices[range] {
                out.push(edits[idx].clone());
            }
        };
        let push_all_deletes = |out: &mut Vec<Edit>, edits: &[Edit]| {
            for &idx in &group.delete_indices {
                out.push(edits[idx].clone());
            }
        };

        // 1. Two-sided boundary echo
        if let Some(echo) = find_boundary_echo(&group, file_lines) {
            warnings.push(describe_boundary_echo_repair(&group, &echo));
            push_inserts(
                &mut out,
                &edits,
                echo.leading..group.insert_indices.len() - echo.trailing,
            );
            push_all_deletes(&mut out, &edits);
            continue;
        }

        let delta = balance_delta(
            compute_delimiter_balance(&group.payload),
            compute_delimiter_balance(
                &file_lines[(group.start_line - 1) as usize..group.end_line as usize],
            ),
        );

        if balance_is_zero(delta) {
            // 2. One-sided echo (balance-neutral)
            if let Some(one_sided) = find_one_sided_boundary_echo(&group, file_lines) {
                warnings.push(describe_one_sided_echo_repair(
                    &group,
                    one_sided.side,
                    one_sided.count,
                ));
                match one_sided.side {
                    EchoSide::Leading => {
                        push_inserts(
                            &mut out,
                            &edits,
                            one_sided.count..group.insert_indices.len(),
                        );
                    }
                    EchoSide::Trailing => {
                        push_inserts(
                            &mut out,
                            &edits,
                            0..group.insert_indices.len() - one_sided.count,
                        );
                    }
                }
                push_all_deletes(&mut out, &edits);
                continue;
            }
            push_inserts(&mut out, &edits, 0..group.insert_indices.len());
            push_all_deletes(&mut out, &edits);
            continue;
        }

        // 3. Duplicate suffix (trailing edge restates a closer/opener below)
        let dup_suffix = find_duplicate_suffix(&group, file_lines, delta);
        if dup_suffix > 0 {
            warnings.push(describe_boundary_repair(
                &group,
                &format!(
                    "dropped {dup_suffix} duplicated trailing payload line(s) already present below the range"
                ),
            ));
            push_inserts(&mut out, &edits, 0..group.insert_indices.len() - dup_suffix);
            push_all_deletes(&mut out, &edits);
            continue;
        }

        // 4. Duplicate prefix (leading edge restates a closer/opener above)
        let dup_prefix = find_duplicate_prefix(&group, file_lines, delta);
        if dup_prefix > 0 {
            warnings.push(describe_boundary_repair(
                &group,
                &format!(
                    "dropped {dup_prefix} duplicated leading payload line(s) already present above the range"
                ),
            ));
            push_inserts(&mut out, &edits, dup_prefix..group.insert_indices.len());
            push_all_deletes(&mut out, &edits);
            continue;
        }

        // 5. Dropped suffix closers (range swallowed a closer the payload never restated)
        let dropped_closers = find_dropped_suffix_closers(&group, file_lines, delta);
        if dropped_closers > 0 {
            warnings.push(describe_boundary_repair(
                &group,
                &format!(
                    "kept {dropped_closers} structural closing line(s) the range deleted without restating"
                ),
            ));
            push_inserts(&mut out, &edits, 0..group.insert_indices.len());
            for &idx in &group.delete_indices[..group.delete_indices.len() - dropped_closers] {
                out.push(edits[idx].clone());
            }
            continue;
        }

        push_inserts(&mut out, &edits, 0..group.insert_indices.len());
        push_all_deletes(&mut out, &edits);
    }
    (out, warnings)
}

// ═══════════════════════════════════════════════════════════════════════════
// After-insert landing correction
// ═══════════════════════════════════════════════════════════════════════════

/// Leading run of tabs and spaces (byte length).
fn leading_indent(s: &str) -> &str {
    let end = s
        .bytes()
        .position(|b| b != b'\t' && b != b' ')
        .unwrap_or(s.len());
    &s[..end]
}

/// `deeper` strictly extends `shallower` (same indent style, more depth).
fn is_indent_deeper(deeper: &str, shallower: &str) -> bool {
    deeper.len() > shallower.len() && deeper.starts_with(shallower)
}

/// An after-insert hunk: rows sharing one anchor line and one patch header
/// line.
struct AfterInsertGroup {
    anchor: u32,
    members: Vec<usize>,
}

/// Shallowest indentation across non-blank body rows, or `None` when no depth
/// claim can be made (all-blank, all-closer, or incomparable indent styles).
fn body_target_indent(rows: &[String]) -> Option<&str> {
    let non_blank: Vec<&str> = rows
        .iter()
        .filter(|r| has_non_whitespace(r))
        .map(|s| s.as_str())
        .collect();
    if non_blank.is_empty() {
        return None;
    }
    if non_blank.iter().all(|r| is_bracket_closer_line(r)) {
        return None;
    }
    let first_indent = leading_indent(non_blank[0]);
    let mut target = first_indent;
    for &row in &non_blank {
        let indent = leading_indent(row);
        if indent.starts_with(target) {
            continue;
        }
        if target.starts_with(indent) {
            target = &first_indent[..indent.len()];
        } else {
            return None;
        }
    }
    Some(target)
}

/// Resolve where an after-insert hunk should land when its body is shallower
/// than the anchor: slide forward past structural closer lines whose
/// indentation still covers the body's target depth.
fn resolve_shifted_landing(
    group: &AfterInsertGroup,
    target: &str,
    file_lines: &[String],
    targeted_lines: &HashSet<u32>,
) -> Option<(u32, usize)> {
    let anchor_idx = (group.anchor - 1) as usize;
    let anchor_text = file_lines.get(anchor_idx)?;
    if !has_non_whitespace(anchor_text) {
        return None;
    }
    let anchor_indent = leading_indent(anchor_text);
    if !is_indent_deeper(anchor_indent, target) {
        return None;
    }

    let mut landing = group.anchor;
    let mut crossed = 0usize;
    let mut line = group.anchor + 1;
    while (line as usize) <= file_lines.len() {
        let text = file_lines
            .get((line - 1) as usize)
            .map(|s| s.as_str())
            .unwrap_or("");
        if !has_non_whitespace(text) {
            line += 1;
            continue;
        }
        if !is_bracket_closer_line(text) {
            break;
        }
        let indent = leading_indent(text);
        if !indent.starts_with(target) {
            break;
        }
        if targeted_lines.contains(&line) {
            return None;
        }
        landing = line;
        crossed += 1;
        if indent.len() == target.len() {
            break;
        }
        line += 1;
    }
    if landing == group.anchor {
        None
    } else {
        Some((landing, crossed))
    }
}

/// Re-target an insert's cursor to `AfterAnchor(line)`.
fn retarget_after_anchor(edit: &Edit, line: u32) -> Edit {
    match edit {
        Edit::Insert {
            cursor: _,
            text,
            line_num,
            index,
            mode,
        } => Edit::Insert {
            cursor: Cursor::AfterAnchor(Anchor { line }),
            text: text.clone(),
            line_num: *line_num,
            index: *index,
            mode: *mode,
        },
        other => other.clone(),
    }
}

/// Slide mis-anchored after-insert hunks outward to the depth their body
/// indentation claims.
fn repair_after_insert_landings(edits: &[Edit], file_lines: &[String]) -> (Vec<Edit>, Vec<String>) {
    // Group plain (non-replacement) after-anchor inserts per authored hunk.
    let mut groups: BTreeMap<(u32, u32), AfterInsertGroup> = BTreeMap::new();
    for (idx, edit) in edits.iter().enumerate() {
        let Edit::Insert {
            cursor: Cursor::AfterAnchor(anchor),
            line_num,
            mode,
            ..
        } = edit
        else {
            continue;
        };
        if *mode == Some(InsertMode::Replacement) {
            continue;
        }
        groups
            .entry((anchor.line, *line_num))
            .or_insert_with(|| AfterInsertGroup {
                anchor: anchor.line,
                members: Vec::new(),
            })
            .members
            .push(idx);
    }
    if groups.is_empty() {
        return (edits.to_vec(), Vec::new());
    }

    // Lines explicitly targeted by any edit; a shift never crosses them.
    let mut targeted_lines: HashSet<u32> = HashSet::new();
    for edit in edits {
        match edit {
            Edit::Delete { anchor, .. } => {
                targeted_lines.insert(anchor.line);
            }
            Edit::Insert {
                cursor: Cursor::BeforeAnchor(a) | Cursor::AfterAnchor(a),
                ..
            } => {
                targeted_lines.insert(a.line);
            }
            _ => {}
        }
    }

    let mut out: Vec<Edit> = edits.to_vec();
    let mut warnings = Vec::new();

    for group in groups.values() {
        let rows: Vec<String> = group
            .members
            .iter()
            .filter_map(|&idx| match &edits[idx] {
                Edit::Insert { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let target = match body_target_indent(&rows) {
            Some(t) => t,
            None => continue,
        };
        if let Some((landing, crossed)) =
            resolve_shifted_landing(group, target, file_lines, &targeted_lines)
        {
            for &idx in &group.members {
                out[idx] = retarget_after_anchor(&out[idx], landing);
            }
            warnings.push(after_insert_landing_shift_warning(
                group.anchor,
                landing,
                crossed,
            ));
        }
    }

    (out, warnings)
}

// ═══════════════════════════════════════════════════════════════════════════
// Bucket / apply helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Index of the trailing phantom sentinel line (0 if none).
fn trailing_phantom_line(file_lines: &[String]) -> usize {
    if file_lines.len() > 1 && file_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        file_lines.len()
    } else {
        0
    }
}

/// Drop delete edits that target the trailing phantom line — deleting it only
/// strips the file's final newline.
fn drop_trailing_phantom_deletes(edits: Vec<Edit>, file_lines: &[String]) -> Vec<Edit> {
    let phantom = trailing_phantom_line(file_lines);
    if phantom == 0 {
        return edits;
    }
    edits
        .into_iter()
        .filter(
            |edit| !matches!(edit, Edit::Delete { anchor, .. } if anchor.line as usize == phantom),
        )
        .collect()
}

/// Verify every anchored edit points at an existing line.
fn validate_line_bounds(edits: &[Edit], file_lines: &[String]) -> Result<(), HashlineError> {
    for edit in edits {
        let anchor_line = match edit {
            Edit::Delete { anchor, .. } => Some(anchor.line),
            Edit::Insert { cursor, .. } => match cursor {
                Cursor::BeforeAnchor(a) | Cursor::AfterAnchor(a) => Some(a.line),
                Cursor::Bof | Cursor::Eof => None,
            },
        };
        if let Some(line) = anchor_line
            && (line < 1 || (line as usize) > file_lines.len())
        {
            return Err(HashlineError::LineOutOfBounds {
                line,
                total: file_lines.len(),
            });
        }
    }
    Ok(())
}

/// Clone an edit with a new sequential `index`.
fn with_index(edit: &Edit, index: usize) -> Edit {
    match edit {
        Edit::Insert {
            cursor,
            text,
            line_num,
            index: _,
            mode,
        } => Edit::Insert {
            cursor: cursor.clone(),
            text: text.clone(),
            line_num: *line_num,
            index,
            mode: *mode,
        },
        Edit::Delete {
            anchor,
            line_num,
            index: _,
            old_assertion,
        } => Edit::Delete {
            anchor: *anchor,
            line_num: *line_num,
            index,
            old_assertion: old_assertion.clone(),
        },
    }
}

fn insert_at_start(file_lines: &mut Vec<String>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    if file_lines.len() == 1 && file_lines[0].is_empty() {
        file_lines.splice(0..1, lines.iter().cloned());
    } else {
        file_lines.splice(0..0, lines.iter().cloned());
    }
}

fn insert_at_end(file_lines: &mut Vec<String>, lines: &[String]) -> Option<u32> {
    if lines.is_empty() {
        return None;
    }
    if file_lines.len() == 1 && file_lines[0].is_empty() {
        file_lines.splice(0..1, lines.iter().cloned());
        return Some(1);
    }
    let has_trailing_newline = file_lines.last().map(|s| s.is_empty()).unwrap_or(false);
    let insert_index = if has_trailing_newline {
        file_lines.len() - 1
    } else {
        file_lines.len()
    };
    file_lines.splice(insert_index..insert_index, lines.iter().cloned());
    Some(insert_index as u32 + 1)
}

// ═══════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Apply a parsed list of [`Edit`]s to `text`. Pure function — no I/O.
///
/// Returns the post-edit text, the first changed line number (1-indexed), and
/// any diagnostic warnings from boundary repair / landing correction.
pub fn apply_edits(text: &str, edits: &[Edit]) -> Result<ApplyResult, HashlineError> {
    if edits.is_empty() {
        return Ok(ApplyResult {
            text: text.to_string(),
            first_changed_line: None,
            warnings: Vec::new(),
        });
    }

    let mut file_lines: Vec<String> = text.split('\n').map(String::from).collect();
    let mut first_changed_line: Option<u32> = None;
    let track = |current: &mut Option<u32>, line: u32| match current {
        None => *current = Some(line),
        Some(existing) if line < *existing => *current = Some(line),
        _ => {}
    };

    let cloned: Vec<Edit> = edits
        .iter()
        .enumerate()
        .map(|(i, e)| with_index(e, i))
        .collect();
    let target_edits = drop_trailing_phantom_deletes(cloned, &file_lines);
    validate_line_bounds(&target_edits, &file_lines)?;

    let (repaired, boundary_warnings) = repair_replacement_boundaries(target_edits, &file_lines);
    let (landed, landing_warnings) = repair_after_insert_landings(&repaired, &file_lines);

    let mut warnings = boundary_warnings;
    warnings.extend(landing_warnings);

    // Partition into bof, eof, and anchor-targeted buckets.
    let mut bof_lines: Vec<String> = Vec::new();
    let mut eof_lines: Vec<String> = Vec::new();
    let mut anchor_edits: Vec<(usize, &Edit)> = Vec::new();
    for (idx, edit) in landed.iter().enumerate() {
        match edit {
            Edit::Insert {
                cursor: Cursor::Bof,
                text,
                ..
            } => bof_lines.push(text.clone()),
            Edit::Insert {
                cursor: Cursor::Eof,
                text,
                ..
            } => eof_lines.push(text.clone()),
            _ => anchor_edits.push((idx, edit)),
        }
    }

    // Bucket anchor edits by line, then apply bottom-up.
    let mut by_line: BTreeMap<u32, Vec<(usize, &Edit)>> = BTreeMap::new();
    for &(idx, edit) in &anchor_edits {
        by_line
            .entry(edit.anchor_line())
            .or_default()
            .push((idx, edit));
    }

    for line in by_line.keys().copied().rev().collect::<Vec<_>>() {
        let mut bucket = by_line.remove(&line).unwrap_or_default();
        bucket.sort_by_key(|(idx, _)| *idx);

        let idx = (line - 1) as usize;
        let current_line = file_lines.get(idx).cloned().unwrap_or_default();
        let mut before_insert: Vec<String> = Vec::new();
        let mut after_insert: Vec<String> = Vec::new();
        let mut replacement: Vec<String> = Vec::new();
        let mut delete_line = false;

        for (_, edit) in &bucket {
            match edit {
                Edit::Insert {
                    mode: Some(InsertMode::Replacement),
                    text,
                    ..
                } => replacement.push(text.clone()),
                Edit::Insert {
                    cursor: Cursor::AfterAnchor(_),
                    text,
                    ..
                } => after_insert.push(text.clone()),
                Edit::Insert { text, .. } => before_insert.push(text.clone()),
                Edit::Delete { .. } => delete_line = true,
            }
        }

        if before_insert.is_empty()
            && replacement.is_empty()
            && after_insert.is_empty()
            && !delete_line
        {
            continue;
        }

        let mut new_lines = before_insert;
        new_lines.extend(replacement);
        if !delete_line {
            new_lines.push(current_line);
        }
        new_lines.extend(after_insert);

        file_lines.splice(idx..=idx, new_lines);
        track(&mut first_changed_line, line);
    }

    if !bof_lines.is_empty() {
        insert_at_start(&mut file_lines, &bof_lines);
        track(&mut first_changed_line, 1);
    }
    if let Some(eof_line) = insert_at_end(&mut file_lines, &eof_lines) {
        track(&mut first_changed_line, eof_line);
    }

    Ok(ApplyResult {
        text: file_lines.join("\n"),
        first_changed_line,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Edit constructors ────────────────────────────────────────────────

    fn ins_before(line: u32, text: &str) -> Edit {
        Edit::Insert {
            cursor: Cursor::BeforeAnchor(Anchor { line }),
            text: text.to_string(),
            line_num: 1,
            index: 0,
            mode: None,
        }
    }

    fn ins_after(line: u32, text: &str) -> Edit {
        Edit::Insert {
            cursor: Cursor::AfterAnchor(Anchor { line }),
            text: text.to_string(),
            line_num: 1,
            index: 0,
            mode: None,
        }
    }

    fn ins_head(text: &str) -> Edit {
        Edit::Insert {
            cursor: Cursor::Bof,
            text: text.to_string(),
            line_num: 1,
            index: 0,
            mode: None,
        }
    }

    fn ins_tail(text: &str) -> Edit {
        Edit::Insert {
            cursor: Cursor::Eof,
            text: text.to_string(),
            line_num: 1,
            index: 0,
            mode: None,
        }
    }

    fn del(line: u32) -> Edit {
        Edit::Delete {
            anchor: Anchor { line },
            line_num: 1,
            index: 0,
            old_assertion: None,
        }
    }

    /// Lower a SWAP start.=end: to replacement inserts (BeforeAnchor) + range
    /// deletes, matching the parser's lowering.
    fn swap(start: u32, end: u32, body: &[&str]) -> Vec<Edit> {
        let mut edits = Vec::new();
        for (i, text) in body.iter().enumerate() {
            edits.push(Edit::Insert {
                cursor: Cursor::BeforeAnchor(Anchor { line: start }),
                text: text.to_string(),
                line_num: 1,
                index: i,
                mode: Some(InsertMode::Replacement),
            });
        }
        for (i, line) in (start..=end).enumerate() {
            edits.push(Edit::Delete {
                anchor: Anchor { line },
                line_num: 1,
                index: body.len() + i,
                old_assertion: None,
            });
        }
        edits
    }

    // ── Basic operations ────────────────────────────────────────────────

    #[test]
    fn basic_replace() {
        let edits = swap(2, 3, &["B", "C"]);
        let result = apply_edits("a\nb\nc\nd", &edits).unwrap();
        assert_eq!(result.text, "a\nB\nC\nd");
        assert_eq!(result.first_changed_line, Some(2));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn basic_replace_single_line() {
        let edits = swap(2, 2, &["X"]);
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "a\nX\nc");
    }

    #[test]
    fn basic_insert_before() {
        let edits = vec![ins_before(2, "X")];
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "a\nX\nb\nc");
    }

    #[test]
    fn basic_insert_after() {
        let edits = vec![ins_after(2, "X")];
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "a\nb\nX\nc");
    }

    #[test]
    fn basic_insert_head() {
        let edits = vec![ins_head("X")];
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "X\na\nb\nc");
        assert_eq!(result.first_changed_line, Some(1));
    }

    #[test]
    fn basic_insert_tail() {
        let edits = vec![ins_tail("X")];
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "a\nb\nc\nX");
        assert_eq!(result.first_changed_line, Some(4));
    }

    #[test]
    fn basic_delete_single() {
        let edits = vec![del(2)];
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "a\nc");
    }

    #[test]
    fn basic_delete_range() {
        let edits = vec![del(2), del(3)];
        let result = apply_edits("a\nb\nc\nd", &edits).unwrap();
        assert_eq!(result.text, "a\nd");
    }

    #[test]
    fn empty_edits_noop() {
        let result = apply_edits("a\nb", &[]).unwrap();
        assert_eq!(result.text, "a\nb");
        assert_eq!(result.first_changed_line, None);
    }

    #[test]
    fn insert_into_empty_file() {
        let edits = vec![ins_head("X")];
        let result = apply_edits("", &edits).unwrap();
        assert_eq!(result.text, "X");
    }

    // ── Trailing phantom line ────────────────────────────────────────────

    #[test]
    fn trailing_phantom_delete_dropped() {
        // "a\nb\n" → ["a", "b", ""], phantom at line 3.
        // DEL 2.=3 should only delete line 2, preserving the final newline.
        let edits = vec![del(2), del(3)];
        let result = apply_edits("a\nb\n", &edits).unwrap();
        assert_eq!(result.text, "a\n");
    }

    #[test]
    fn no_phantom_when_no_trailing_newline() {
        let edits = vec![del(2)];
        let result = apply_edits("a\nb", &edits).unwrap();
        assert_eq!(result.text, "a");
    }

    #[test]
    fn insert_tail_preserves_trailing_newline() {
        let edits = vec![ins_tail("X")];
        let result = apply_edits("a\nb\n", &edits).unwrap();
        assert_eq!(result.text, "a\nb\nX\n");
    }

    // ── Line bounds validation ──────────────────────────────────────────

    #[test]
    fn line_out_of_bounds() {
        let edits = vec![ins_before(99, "X")];
        let result = apply_edits("a\nb", &edits);
        assert!(result.is_err());
        match result {
            Err(HashlineError::LineOutOfBounds { line, total }) => {
                assert_eq!(line, 99);
                assert_eq!(total, 2);
            }
            _ => panic!("expected LineOutOfBounds"),
        }
    }

    #[test]
    fn bof_eof_not_validated_against_bounds() {
        let edits = vec![ins_head("X"), ins_tail("Y")];
        let result = apply_edits("a", &edits).unwrap();
        assert_eq!(result.text, "X\na\nY");
    }

    // ── Delimiter balance ───────────────────────────────────────────────

    #[test]
    fn delimiter_balance_simple() {
        assert_eq!(
            compute_delimiter_balance(&["foo(bar)".to_string()]),
            DelimiterBalance::default()
        );
        assert_eq!(
            compute_delimiter_balance(&["foo(bar".to_string()]),
            DelimiterBalance {
                paren: 1,
                bracket: 0,
                brace: 0
            }
        );
    }

    #[test]
    fn delimiter_balance_skips_string_literals() {
        let lines = vec!["x = \"(\"".to_string()];
        assert_eq!(
            compute_delimiter_balance(&lines),
            DelimiterBalance::default()
        );
    }

    #[test]
    fn delimiter_balance_skips_single_quotes() {
        let lines = vec!["y = ']'".to_string()];
        assert_eq!(
            compute_delimiter_balance(&lines),
            DelimiterBalance::default()
        );
    }

    #[test]
    fn delimiter_balance_skips_line_comments() {
        let lines = vec!["// ({[".to_string()];
        assert_eq!(
            compute_delimiter_balance(&lines),
            DelimiterBalance::default()
        );
    }

    #[test]
    fn delimiter_balance_block_comment_spans_lines() {
        let lines = vec!["/* (".to_string(), "[ */ )".to_string()];
        let bal = compute_delimiter_balance(&lines);
        assert_eq!(bal.paren, -1); // the ) after the comment closes nothing
        assert_eq!(bal.bracket, 0); // the [ is inside the comment
    }

    #[test]
    fn delimiter_balance_template_spans_lines() {
        let lines = vec!["`(".to_string(), ")`".to_string()];
        assert_eq!(
            compute_delimiter_balance(&lines),
            DelimiterBalance::default()
        );
    }

    #[test]
    fn delimiter_balance_single_quote_resets_at_eol() {
        // `"` / `'` cannot span lines; the open paren before the quote counts.
        let lines = vec!["(x = '".to_string(), "  bar".to_string()];
        let bal = compute_delimiter_balance(&lines);
        assert_eq!(bal.paren, 1);
    }

    #[test]
    fn delimiter_balance_mixed() {
        let lines = vec![
            "function f() {".to_string(),
            "  return [1, 2];".to_string(),
            "}".to_string(),
        ];
        let bal = compute_delimiter_balance(&lines);
        assert_eq!(bal, DelimiterBalance::default());
    }

    // ── Closer detection ────────────────────────────────────────────────

    #[test]
    fn bracket_closer_lines() {
        assert!(is_bracket_closer_line("}"));
        assert!(is_bracket_closer_line("});"));
        assert!(is_bracket_closer_line("})"));
        assert!(is_bracket_closer_line("];"));
        assert!(is_bracket_closer_line("  }"));
        assert!(!is_bracket_closer_line("} ,"));
        assert!(!is_bracket_closer_line("x = 1"));
        assert!(!is_bracket_closer_line("return }"));
        assert!(!is_bracket_closer_line(""));
    }

    #[test]
    fn jsx_closer_lines() {
        assert!(is_jsx_closer_line("</div>"));
        assert!(is_jsx_closer_line("</>"));
        assert!(is_jsx_closer_line("/>"));
        assert!(is_jsx_closer_line("  </Section>"));
        assert!(is_jsx_closer_line("</div>;"));
        assert!(!is_jsx_closer_line("<div>"));
        assert!(!is_jsx_closer_line("x = 1"));
    }

    #[test]
    fn structural_closer_includes_both() {
        assert!(is_structural_closer_line("}"));
        assert!(is_structural_closer_line("</div>"));
    }

    #[test]
    fn jsx_closer_name_extraction() {
        assert_eq!(jsx_closer_name("</div>"), Some("div".to_string()));
        assert_eq!(jsx_closer_name("</>"), Some(String::new()));
        assert_eq!(jsx_closer_name("x = 1"), None);
    }

    // ── Boundary echo: two-sided ────────────────────────────────────────

    #[test]
    fn boundary_echo_drops_both_sides() {
        // File: a b c d e
        // SWAP 2.=3: payload restates line 1 (leading) and line 4 (trailing).
        let edits = swap(2, 3, &["a", "B", "C", "d"]);
        let result = apply_edits("a\nb\nc\nd\ne", &edits).unwrap();
        assert_eq!(result.text, "a\nB\nC\nd\ne");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("boundary echo"));
    }

    #[test]
    fn boundary_echo_with_brackets() {
        // Function body replacement where the model restates header + closer.
        let file = "function foo() {\n  return 1;\n}";
        // Payload restates header and closer (balance-neutral check).
        let edits = swap(2, 2, &["function foo() {", "  return 2;", "}"]);
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "function foo() {\n  return 2;\n}");
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn no_boundary_echo_when_payload_too_short() {
        // Both sides echo but their sum covers the whole payload → bail.
        // SWAP 2.=2 on "a\nb\nc" with payload ["a", "c"]: "a" echoes line 1,
        // "c" echoes line 3. leading+trailing = 2 >= payload.len() = 2 → bail.
        let edits = swap(2, 2, &["a", "c"]);
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "a\na\nc\nc");
        assert!(result.warnings.is_empty());
    }

    // ── One-sided echo ──────────────────────────────────────────────────

    #[test]
    fn one_sided_echo_trailing_structural_closer() {
        // Single-line replacement, trailing edge restates a JSX structural
        // closer below the range, delta is zero, echo line is balance-neutral.
        // File: "x\n</div>"
        // SWAP 1.=1 with payload ["X", "</div>"] — the model retyped the closer.
        // delta = payload_balance - deleted_balance = 0 - 0 = 0.
        // Trailing: payload[1] = "</div>" == fileLines[1] = "</div>". Count 1.
        // Leading: 0. XOR satisfied. Single-line range, trailing, structural
        // closer with zero delimiter balance, no JSX opener in prefix.
        let edits = swap(1, 1, &["X", "</div>"]);
        let result = apply_edits("x\n</div>", &edits).unwrap();
        assert_eq!(result.text, "X\n</div>");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("one line short"));
    }

    #[test]
    fn one_sided_echo_leading_multi_line() {
        // Multi-line range, leading edge restates a line above, balance is zero.
        let file = "a\nb\nc\nd";
        // SWAP 2.=3 replaces "b" and "c" with "a\nB\nC".
        // Leading: payload[0]="a" == fileLines[0]. Count 1. Content.
        // Trailing: payload[2]="C" vs fileLines[3]="d". No match. Count 0.
        // XOR: leading>0, trailing=0. Multi-line range (2 deletes).
        // Echo balance ["a"] = 0. Pass.
        let edits = swap(2, 3, &["a", "B", "C"]);
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "a\nB\nC\nd");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("leading"));
    }

    // ── Duplicate suffix / prefix (delimiter-imbalanced) ────────────────

    #[test]
    fn duplicate_suffix_dropped() {
        // File: foo( \n bar \n ) \n )
        // SWAP 1.=3 replaces with payload that restates the trailing ")".
        let file = "foo(\n  bar\n)\n)";
        let edits = swap(1, 3, &["new(", "  content", ")", ")"]);
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "new(\n  content\n)\n)");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("trailing"));
    }

    #[test]
    fn duplicate_prefix_dropped() {
        // File: "(\nx\n)" — SWAP 2.=2 replaces "x" with "(\nY".
        // The model restated "(" (line 1) at the start of the payload.
        // deleted "x" → balance 0. payload "(\nY" → paren 1.
        // delta = 1 - 0 = paren 1. prefix "(" balance = paren 1 = delta. Match!
        let file = "(\nx\n)";
        let edits = swap(2, 2, &["(", "Y"]);
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "(\nY\n)");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("leading"));
    }

    // ── Dropped suffix closers ──────────────────────────────────────────

    #[test]
    fn dropped_suffix_closer_preserved() {
        // File: x \n } \n y
        // SWAP 1.=2 replaces "x" and "}" with "X" (forgot the closer).
        // delta = payload_balance - deleted_balance = 0 - (-1) = brace 1.
        // wanted = negate(delta) = brace -1.
        // m=1: fileLines[endLine-1] = fileLines[1] = "}". Closer.
        // payload doesn't end with deleted suffix. suffix "}" balance = -1 == wanted. Match!
        let file = "x\n}\ny";
        let edits = swap(1, 2, &["X"]);
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "X\n}\ny");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("kept 1"));
    }

    // ── Landing correction ──────────────────────────────────────────────

    #[test]
    fn landing_shift_outward() {
        // Body indented shallower than the anchor → slide past closer lines.
        let file = "function foo() {\n  bar();\n}";
        let edits = vec![ins_after(2, "baz();")];
        let result = apply_edits(file, &edits).unwrap();
        // baz(); (no indent) should land after line 3 "}", not after line 2.
        assert_eq!(result.text, "function foo() {\n  bar();\n}\nbaz();");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("INS.POST 2"));
        assert!(result.warnings[0].contains("after line 3"));
    }

    #[test]
    fn landing_no_shift_when_indent_matches() {
        // Body at same indent as anchor → no shift.
        let file = "function foo() {\n  bar();\n}";
        let edits = vec![ins_after(2, "  baz();")];
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "function foo() {\n  bar();\n  baz();\n}");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn landing_no_shift_for_content_line_after_anchor() {
        // A content (non-closer) line follows the anchor → no crossing.
        let file = "a\nb\nc";
        let edits = vec![ins_after(1, "X")];
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "a\nX\nb\nc");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn landing_abandoned_when_targeted_line_crossed() {
        // Another edit targets the closer → shift abandoned.
        let file = "function foo() {\n  bar();\n}";
        // Delete line 3 (the closer) + insert after line 2 at depth 0.
        let edits = vec![ins_after(2, "baz();"), del(3)];
        let result = apply_edits(file, &edits).unwrap();
        // The closer is deleted, so the insert stays at line 2.
        assert_eq!(result.text, "function foo() {\n  bar();\nbaz();");
        assert!(result.warnings.is_empty());
    }

    // ── Indent helpers ──────────────────────────────────────────────────

    #[test]
    fn leading_indent_extraction() {
        assert_eq!(leading_indent("  foo"), "  ");
        assert_eq!(leading_indent("\t\tbar"), "\t\t");
        assert_eq!(leading_indent("foo"), "");
        assert_eq!(leading_indent("   "), "   ");
    }

    #[test]
    fn indent_deeper_check() {
        assert!(is_indent_deeper("    ", "  "));
        assert!(is_indent_deeper("  ", ""));
        assert!(!is_indent_deeper("  ", "    "));
        assert!(!is_indent_deeper("  ", "  "));
        assert!(!is_indent_deeper("\t", "  ")); // tab vs space — not a prefix
    }

    #[test]
    fn body_target_indent_computation() {
        let rows = vec!["  a".to_string(), "  b".to_string()];
        assert_eq!(body_target_indent(&rows), Some("  "));

        let rows = vec!["  a".to_string(), "b".to_string()];
        assert_eq!(body_target_indent(&rows), Some(""));

        let rows = vec!["  a".to_string(), "\tb".to_string()];
        assert_eq!(body_target_indent(&rows), None); // incomparable

        let rows = vec!["}".to_string()];
        assert_eq!(body_target_indent(&rows), None); // all closers

        let rows = vec!["".to_string()];
        assert_eq!(body_target_indent(&rows), None); // all blank
    }

    // ── Phantom detection ───────────────────────────────────────────────

    #[test]
    fn trailing_phantom_detection() {
        assert_eq!(
            trailing_phantom_line(&["a".to_string(), "b".to_string(), "".to_string()]),
            3
        );
        assert_eq!(
            trailing_phantom_line(&["a".to_string(), "b".to_string()]),
            0
        );
        assert_eq!(trailing_phantom_line(&["".to_string()]), 0); // single empty line
    }

    // ── Combined operations ─────────────────────────────────────────────

    #[test]
    fn multiple_edits_same_file() {
        let edits = vec![ins_before(1, "header"), ins_after(2, "tail_of_2"), del(3)];
        let result = apply_edits("a\nb\nc\nd", &edits).unwrap();
        assert_eq!(result.text, "header\na\nb\ntail_of_2\nd");
    }

    #[test]
    fn first_changed_line_tracks_minimum() {
        // Insert at line 1 and delete at line 3 → first changed is 1.
        let edits = vec![ins_head("X"), del(3)];
        let result = apply_edits("a\nb\nc", &edits).unwrap();
        assert_eq!(result.text, "X\na\nb");
        assert_eq!(result.first_changed_line, Some(1));
    }

    #[test]
    fn replacement_preserves_unaffected_lines() {
        let file = "line1\nline2\nline3\nline4\nline5";
        let edits = swap(2, 4, &["new2", "new3", "new4"]);
        let result = apply_edits(file, &edits).unwrap();
        assert_eq!(result.text, "line1\nnew2\nnew3\nnew4\nline5");
    }

    #[test]
    fn has_non_whitespace_check() {
        assert!(has_non_whitespace("  x  "));
        assert!(!has_non_whitespace("   "));
        assert!(!has_non_whitespace("\t\n\r"));
        assert!(has_non_whitespace("a"));
    }
}
