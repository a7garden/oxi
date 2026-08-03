//! Token-driven state machine that turns a stream of [`Token`]s into a flat
//! list of [`Edit`]s, plus the top-level envelope splitter that carves an
//! authored patch into [`PatchSection`]s.
//!
//! Ported from omp `packages/hashline/src/parser.ts` (the `Executor` state
//! machine) and `packages/hashline/src/input.ts` (`splitPatchInput`,
//! `PatchSection`).

use std::collections::HashMap;
use std::path::{Component, Path};

use crate::format::{
    HL_FILE_HASH_LENGTH, HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX, HL_RANGE_SEP,
};
use crate::messages::{
    BARE_BODY_AUTO_PIPED_WARNING, DELETE_TAKES_NO_BODY, EMPTY_INSERT, MINUS_ROW_REJECTED,
};
use crate::mismatch::HashlineError;
use crate::tokenizer::{BlockTarget, Token, Tokenizer, classify_line, split_hashline_lines};
use crate::types::{Anchor, Cursor, Edit, InsertMode, ParsedRange, SplitOptions};

// ── Internal error ───────────────────────────────────────────────────────

/// A parse failure carrying the source line number and a focused message.
/// Converted to [`HashlineError::Parse`] at public boundaries.
#[derive(Debug)]
pub(crate) struct ParseError {
    line: u32,
    msg: String,
}

fn perr(line: u32, msg: impl Into<String>) -> ParseError {
    ParseError {
        line,
        msg: msg.into(),
    }
}

type PResult<T> = Result<T, ParseError>;

impl ParseError {
    fn into_hash(self) -> HashlineError {
        HashlineError::parse(self.line, self.msg)
    }
}

// ── Small byte helpers (parser-local) ────────────────────────────────────

#[inline]
fn is_ws(b: u8) -> bool {
    b == b' ' || (b'\t'..=b'\r').contains(&b)
}

#[inline]
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[inline]
fn is_nonzero_digit(b: u8) -> bool {
    (b'1'..=b'9').contains(&b)
}

#[inline]
fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

#[inline]
fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

// ── Read-output prefix stripping (prefixes.ts) ───────────────────────────

/// Strip at most one leading hashline/snapshot prefix (`N:`, `>>>N:`, `+N:`,
/// `*-N:` …). Single-pass: does NOT loop, so genuine content beginning with
/// `digits:` is left intact when not uniformly prefixed.
///
/// Mirrors omp `stripOneLeadingHashlinePrefix` / `HL_PREFIX_RE`.
fn strip_one_leading_hashline_prefix(line: &str) -> String {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    if bytes[i..].starts_with(b">>>") {
        i += 3;
    } else if bytes[i..].starts_with(b">>") {
        i += 2;
    }
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    if i < n && (bytes[i] == b'+' || bytes[i] == b'*' || bytes[i] == b'-') {
        i += 1;
        while i < n && is_ws(bytes[i]) {
            i += 1;
        }
    }
    if i < n && is_digit(bytes[i]) {
        while i < n && is_digit(bytes[i]) {
            i += 1;
        }
        if i < n && bytes[i] == b':' {
            return line[i + 1..].to_string();
        }
    }
    line.to_string()
}

/// A stripped remainder that is a lone quoted or numeric literal (optionally
/// comma-terminated) — the shape of a numeric-keyed dict/YAML body rather than
/// read-output paste. Mirrors omp `BARE_LITERAL_VALUE_RE`.
fn is_bare_literal_value(s: &str) -> bool {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    let matched = if i < n && (bytes[i] == b'"' || bytes[i] == b'\'') {
        let quote = bytes[i];
        i += 1;
        while i < n && bytes[i] != quote {
            i += 1;
        }
        if i >= n {
            return false; // unterminated quote
        }
        i += 1; // closing quote
        true
    } else {
        if i < n && (bytes[i] == b'-' || bytes[i] == b'+') {
            i += 1;
        }
        if i >= n || !is_digit(bytes[i]) {
            return false;
        }
        while i < n && is_digit(bytes[i]) {
            i += 1;
        }
        if i < n && bytes[i] == b'.' {
            i += 1;
            if i >= n || !is_digit(bytes[i]) {
                return false;
            }
            while i < n && is_digit(bytes[i]) {
                i += 1;
            }
        }
        true
    };
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    if i < n && bytes[i] == b',' {
        i += 1;
    }
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    matched && i == n
}

/// Strip a single read-output `N:` prefix from every bare body row, but only
/// when *all* bare rows carry one (and the result is not a dict/YAML literal
/// body). Mirrors omp `Executor.#stripBarePrefixesIfUniform`.
fn strip_bare_prefixes_if_uniform(payloads: &mut [PayloadRows]) {
    let mut saw_bare = false;
    let mut all_literal_values = true;
    for row in payloads.iter() {
        if !row.bare || row.text.trim().is_empty() {
            continue;
        }
        saw_bare = true;
        let stripped = strip_one_leading_hashline_prefix(&row.text);
        if stripped == row.text {
            return; // not every bare row carries a prefix → leave untouched
        }
        if all_literal_values && !is_bare_literal_value(&stripped) {
            all_literal_values = false;
        }
    }
    if !saw_bare || all_literal_values {
        return;
    }
    for row in payloads.iter_mut() {
        if row.bare && !row.text.trim().is_empty() {
            row.text = strip_one_leading_hashline_prefix(&row.text);
        }
    }
}

// ── Contamination detection (parser.ts) ─────────────────────────────────

/// Detect apply_patch / unified-diff contamination that is not valid in
/// hashline. Returns a focused error message when the line is a known foreign
/// shape, else `None`. Mirrors omp `detectApplyPatchContamination`.
fn detect_apply_patch_contamination(text: &str, _has_pending: bool) -> Option<String> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("*** Update File:")
        || trimmed.starts_with("*** Add File:")
        || trimmed.starts_with("*** Delete File:")
        || trimmed.starts_with("*** Move to:")
    {
        return Some(format!(
            "apply_patch sentinel {prev} is not valid in hashline. File sections start with \
             `[path#HASH]` (no `Update File:` / `Add File:` keyword). Use `SWAP N.=M:`, \
             `DEL N.=M`, or `INS.PRE|POST|HEAD|TAIL:` ops.",
            prev = contamination_preview(trimmed)
        ));
    }
    if is_unified_diff_hunk(trimmed) {
        return Some(
            "unified-diff hunk header (`@@ -N,M +N,M @@`) is not valid in hashline. Use \
             `SWAP N.=M:`, `DEL N.=M`, or `INS.PRE|POST|HEAD|TAIL:` ops."
                .to_string(),
        );
    }
    if trimmed.starts_with("@@") {
        return Some(format!(
            "`@@`-bracketed hunk header {prev} is not valid in hashline. Drop the `@@ ... @@` \
             brackets and write a verb header such as `SWAP N.=M:`.",
            prev = contamination_preview(trimmed)
        ));
    }
    if is_del_with_colon(trimmed) {
        return Some(
            "`DEL N.=M` has no colon and no body. Remove the colon and body rows.".to_string(),
        );
    }
    if is_bare_line_number(trimmed) {
        let n = trimmed.trim();
        return Some(format!(
            "hunk headers need a verb. Use `SWAP {n}{sep}{n}:` to replace, or `DEL {n}` to delete.",
            sep = HL_RANGE_SEP
        ));
    }
    if let Some((a, b)) = parse_bare_range(trimmed) {
        return Some(format!(
            "bare range hunk header `{trimmed}` is not valid. Hunk headers need a verb: write \
             `SWAP {a}{sep}{b}:` or `DEL {a}{sep}{b}`.",
            sep = HL_RANGE_SEP
        ));
    }
    None
}

/// `@@ -N,M +N,M @@` unified-diff hunk header shape.
fn is_unified_diff_hunk(s: &str) -> bool {
    let b = s.as_bytes();
    let n = b.len();
    if !b.starts_with(b"@@") {
        return false;
    }
    let Some(mut j) = ws1(b, 2, n) else {
        return false;
    };
    j = opt_sign(b, j);
    let Some(k) = digits(b, j, n) else {
        return false;
    };
    if k >= n || b[k] != b',' {
        return false;
    }
    let Some(k2) = digits(b, k + 1, n) else {
        return false;
    };
    let Some(k3) = ws1(b, k2, n) else {
        return false;
    };
    let k4 = opt_sign(b, k3);
    let Some(k5) = digits(b, k4, n) else {
        return false;
    };
    if k5 >= n || b[k5] != b',' {
        return false;
    }
    let Some(k6) = digits(b, k5 + 1, n) else {
        return false;
    };
    let Some(k7) = ws1(b, k6, n) else {
        return false;
    };
    b[k7..].starts_with(b"@@")
}

/// `DEL N` or `DEL N.=M` followed by a stray colon.
fn is_del_with_colon(s: &str) -> bool {
    let b = s.as_bytes();
    let n = b.len();
    if !b.starts_with(b"DEL") {
        return false;
    }
    let mut i = 3;
    let ws_start = i;
    while i < n && is_ws(b[i]) {
        i += 1;
    }
    if i == ws_start || i >= n || !is_nonzero_digit(b[i]) {
        return false;
    }
    i += 1;
    while i < n && is_digit(b[i]) {
        i += 1;
    }
    let after_num = i;
    // optional (sep + second number)
    let mut k = i;
    while k < n && is_ws(b[k]) {
        k += 1;
    }
    let consumed_sep_ws = k > i;
    let after_sep = if b[k..n].starts_with(b"..") || b[k..n].starts_with(b".=") {
        Some(k + 2)
    } else if k < n && b[k] == b'-' {
        Some(k + 1)
    } else if b[k..n].starts_with("\u{2026}".as_bytes()) {
        Some(k + 3)
    } else if consumed_sep_ws {
        Some(k)
    } else {
        None
    };
    if let Some(k2) = after_sep {
        let mut k3 = k2;
        while k3 < n && is_ws(b[k3]) {
            k3 += 1;
        }
        if k3 < n && is_nonzero_digit(b[k3]) {
            k3 += 1;
            while k3 < n && is_digit(b[k3]) {
                k3 += 1;
            }
            i = k3;
        } else {
            i = after_num;
        }
    }
    while i < n && is_ws(b[i]) {
        i += 1;
    }
    i < n && b[i] == b':'
}

/// A bare line number (`42`, with optional trailing whitespace).
fn is_bare_line_number(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || !is_nonzero_digit(b[0]) {
        return false;
    }
    let mut i = 1;
    while i < b.len() && is_digit(b[i]) {
        i += 1;
    }
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    i == b.len()
}

/// A bare range `N … M` (optionally colon-terminated). Returns the two numbers.
fn parse_bare_range(s: &str) -> Option<(String, String)> {
    let b = s.as_bytes();
    let n = b.len();
    if b.is_empty() || !is_nonzero_digit(b[0]) {
        return None;
    }
    let mut i = 1;
    while i < n && is_digit(b[i]) {
        i += 1;
    }
    let first = s[..i].to_string();
    while i < n && is_ws(b[i]) {
        i += 1;
    }
    let sep_start = i;
    while i < n {
        let c = b[i];
        if c == b'-' || c == b'.' || c == b'=' || is_ws(c) {
            i += 1;
        } else if b[i..n].starts_with("\u{2026}".as_bytes()) {
            i += 3;
        } else {
            break;
        }
    }
    if i == sep_start {
        return None;
    }
    while i < n && is_ws(b[i]) {
        i += 1;
    }
    if i >= n || !is_nonzero_digit(b[i]) {
        return None;
    }
    let second_start = i;
    i += 1;
    while i < n && is_digit(b[i]) {
        i += 1;
    }
    let second = s[second_start..i].to_string();
    while i < n && is_ws(b[i]) {
        i += 1;
    }
    if i < n && b[i] == b':' {
        i += 1;
    }
    if i != n {
        return None;
    }
    Some((first, second))
}

#[inline]
fn opt_sign(b: &[u8], i: usize) -> usize {
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i + 1
    } else {
        i
    }
}

fn digits(b: &[u8], i: usize, n: usize) -> Option<usize> {
    let start = i;
    let mut j = i;
    while j < n && is_digit(b[j]) {
        j += 1;
    }
    (j != start).then_some(j)
}

fn ws1(b: &[u8], i: usize, n: usize) -> Option<usize> {
    let start = i;
    let mut j = i;
    while j < n && is_ws(b[j]) {
        j += 1;
    }
    (j != start).then_some(j)
}

fn contamination_preview(trimmed: &str) -> String {
    const MAX: usize = 48;
    let chars: Vec<char> = trimmed.chars().collect();
    let preview = if chars.len() > MAX {
        let head: String = chars[..MAX].iter().collect();
        format!("{head}\u{2026}")
    } else {
        trimmed.to_string()
    };
    json_quote(&preview)
}

fn json_truncated(s: &str, max: usize) -> String {
    let taken: String = s.chars().take(max).collect();
    json_quote(&taken)
}

fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── Range / comment helpers ──────────────────────────────────────────────

fn validate_range_order(range: ParsedRange, line_num: u32) -> PResult<()> {
    if range.end < range.start {
        return Err(perr(
            line_num,
            format!(
                "range {a}{sep}{b} ends before it starts.",
                a = range.start,
                b = range.end,
                sep = HL_RANGE_SEP
            ),
        ));
    }
    Ok(())
}

fn expand_range(range: ParsedRange) -> Vec<Anchor> {
    (range.start..=range.end)
        .map(|line| Anchor { line })
        .collect()
}

fn is_skippable_comment_line(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

// ── Executor state machine ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PayloadRows {
    text: String,
    bare: bool,
}

struct Pending {
    target: BlockTarget,
    line_num: u32,
    payloads: Vec<PayloadRows>,
    deferred_blanks: Vec<PayloadRows>,
}

struct PendingComment {
    line_num: u32,
    text: String,
}

/// Token-driven state machine: feeds produce pending hunks; a new op or the
/// final flush lowers each hunk into [`Edit`]s. Mirrors omp `Executor`.
pub(crate) struct Executor {
    edits: Vec<Edit>,
    warnings: Vec<String>,
    edit_index: usize,
    pending: Option<Pending>,
    terminated: bool,
    skippable_comments: Vec<PendingComment>,
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Self {
        Self {
            edits: Vec::new(),
            warnings: Vec::new(),
            edit_index: 0,
            pending: None,
            terminated: false,
            skippable_comments: Vec::new(),
        }
    }

    /// Feed one token. Errors on illegal hunk shapes / contamination.
    pub(crate) fn feed(&mut self, token: Token) -> PResult<()> {
        if self.terminated {
            return Ok(());
        }
        match token {
            Token::EnvelopeBegin { .. } => {
                self.consume_pending_skippable_comments()?;
            }
            Token::EnvelopeEnd { .. } => {
                self.consume_pending_skippable_comments()?;
                self.terminated = true;
            }
            Token::Abort { .. } => {
                self.terminated = true;
            }
            Token::Header { .. } => {
                self.consume_pending_skippable_comments()?;
                self.flush_pending()?;
            }
            Token::Blank { .. } => {
                self.consume_pending_skippable_comments()?;
                self.handle_blank("");
            }
            Token::PayloadLiteral { text, line_num, .. } => {
                self.consume_pending_skippable_comments()?;
                self.handle_literal_payload(text, line_num)?;
            }
            Token::Raw { text, line_num, .. } => {
                if self.pending.is_none() && is_skippable_comment_line(&text) {
                    self.skippable_comments
                        .push(PendingComment { line_num, text });
                } else {
                    self.consume_pending_skippable_comments()?;
                    self.handle_raw(text, line_num)?;
                }
            }
            Token::Op {
                target, line_num, ..
            } => {
                self.discard_pending_skippable_comments();
                if let BlockTarget::Replace { range } | BlockTarget::Delete { range } = &target {
                    validate_range_order(*range, line_num)?;
                }
                self.flush_pending()?;
                self.pending = Some(Pending {
                    target,
                    line_num,
                    payloads: Vec::new(),
                    deferred_blanks: Vec::new(),
                });
            }
        }
        Ok(())
    }

    /// Drain pending and validate the final edit list (strict path).
    pub(crate) fn finish(mut self) -> PResult<(Vec<Edit>, Vec<String>)> {
        self.consume_pending_skippable_comments()?;
        self.flush_pending()?;
        self.validate_no_overlapping_deletes()?;
        Ok((self.edits, self.warnings))
    }

    /// Streaming-tolerant finish: a trailing op with no payload yet is dropped
    /// rather than emitting a phantom empty-payload error.
    pub(crate) fn finish_streaming(mut self) -> PResult<(Vec<Edit>, Vec<String>)> {
        self.consume_pending_skippable_comments()?;
        if let Some(pending) = self.pending.take() {
            let flush = !pending.payloads.is_empty()
                || matches!(pending.target, BlockTarget::Delete { .. });
            if flush {
                self.pending = Some(pending);
                self.flush_pending()?;
            }
        }
        self.validate_no_overlapping_deletes()?;
        Ok((self.edits, self.warnings))
    }

    fn discard_pending_skippable_comments(&mut self) {
        self.skippable_comments.clear();
    }

    fn consume_pending_skippable_comments(&mut self) -> PResult<()> {
        if self.skippable_comments.is_empty() {
            return Ok(());
        }
        let comments = std::mem::take(&mut self.skippable_comments);
        for c in comments {
            self.handle_raw(c.text, c.line_num)?;
        }
        Ok(())
    }

    fn warn(&mut self, msg: &'static str) {
        if !self.warnings.iter().any(|w| w == msg) {
            self.warnings.push(msg.to_string());
        }
    }

    fn handle_literal_payload(&mut self, text: String, line_num: u32) -> PResult<()> {
        let Some(mut pending) = self.pending.take() else {
            return Err(perr(
                line_num,
                format!("payload line has no preceding hunk header. Got `+{text}`."),
            ));
        };
        if matches!(pending.target, BlockTarget::Delete { .. }) {
            self.pending = Some(pending);
            return Err(perr(line_num, DELETE_TAKES_NO_BODY.to_string()));
        }
        self.commit_deferred_blanks(&mut pending);
        pending.payloads.push(PayloadRows { text, bare: false });
        self.pending = Some(pending);
        Ok(())
    }

    fn handle_raw(&mut self, text: String, line_num: u32) -> PResult<()> {
        if let Some(msg) = detect_apply_patch_contamination(&text, self.pending.is_some()) {
            return Err(perr(line_num, msg));
        }
        let Some(mut pending) = self.pending.take() else {
            if text.trim().is_empty() {
                return Ok(());
            }
            return Err(perr(
                line_num,
                format!(
                    "payload line has no preceding hunk header. Use `SWAP N.=M:`, `DEL N.=M`, \
                     or `INS.PRE|POST|HEAD|TAIL:` above the body. Got `{text}`."
                ),
            ));
        };
        if text.trim().is_empty() {
            self.pending = Some(pending);
            self.handle_blank(&text);
            return Ok(());
        }
        let is_delete = matches!(pending.target, BlockTarget::Delete { .. });
        let is_minus = text.trim_start().as_bytes().first() == Some(&b'-');
        if is_delete || is_minus {
            self.pending = Some(pending);
            let msg = if is_delete {
                DELETE_TAKES_NO_BODY
            } else {
                MINUS_ROW_REJECTED
            };
            return Err(perr(line_num, msg.to_string()));
        }
        self.warn(BARE_BODY_AUTO_PIPED_WARNING);
        self.commit_deferred_blanks(&mut pending);
        pending.payloads.push(PayloadRows { text, bare: true });
        self.pending = Some(pending);
        Ok(())
    }

    fn handle_blank(&mut self, text: &str) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if matches!(pending.target, BlockTarget::Delete { .. }) {
            return;
        }
        if pending.payloads.is_empty() {
            return;
        }
        pending.deferred_blanks.push(PayloadRows {
            text: text.to_string(),
            bare: true,
        });
    }

    fn commit_deferred_blanks(&mut self, pending: &mut Pending) {
        if pending.deferred_blanks.is_empty() {
            return;
        }
        self.warn(BARE_BODY_AUTO_PIPED_WARNING);
        let mut blanks = std::mem::take(&mut pending.deferred_blanks);
        pending.payloads.append(&mut blanks);
    }

    fn flush_pending(&mut self) -> PResult<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let Pending {
            target,
            line_num,
            mut payloads,
            ..
        } = pending;
        strip_bare_prefixes_if_uniform(&mut payloads);
        match target {
            BlockTarget::Delete { range } => {
                for anchor in expand_range(range) {
                    self.push_delete(anchor, line_num);
                }
            }
            BlockTarget::Replace { range } => {
                if payloads.is_empty() {
                    for anchor in expand_range(range) {
                        self.push_delete(anchor, line_num);
                    }
                } else {
                    let cursor = Cursor::BeforeAnchor(Anchor { line: range.start });
                    self.emit_payload_rows(
                        cursor,
                        &payloads,
                        line_num,
                        Some(InsertMode::Replacement),
                    );
                    for anchor in expand_range(range) {
                        self.push_delete(anchor, line_num);
                    }
                }
            }
            BlockTarget::InsertBefore { anchor } => {
                if payloads.is_empty() {
                    return Err(perr(line_num, EMPTY_INSERT.to_string()));
                }
                self.emit_payload_rows(Cursor::BeforeAnchor(anchor), &payloads, line_num, None);
            }
            BlockTarget::InsertAfter { anchor } => {
                if payloads.is_empty() {
                    return Err(perr(line_num, EMPTY_INSERT.to_string()));
                }
                self.emit_payload_rows(Cursor::AfterAnchor(anchor), &payloads, line_num, None);
            }
            BlockTarget::Bof => {
                if payloads.is_empty() {
                    return Err(perr(line_num, EMPTY_INSERT.to_string()));
                }
                self.emit_payload_rows(Cursor::Bof, &payloads, line_num, None);
            }
            BlockTarget::Eof => {
                if payloads.is_empty() {
                    return Err(perr(line_num, EMPTY_INSERT.to_string()));
                }
                self.emit_payload_rows(Cursor::Eof, &payloads, line_num, None);
            }
        }
        Ok(())
    }

    fn push_insert(
        &mut self,
        cursor: Cursor,
        text: String,
        line_num: u32,
        mode: Option<InsertMode>,
    ) {
        self.edits.push(Edit::Insert {
            cursor,
            text,
            line_num,
            index: self.edit_index,
            mode,
        });
        self.edit_index += 1;
    }

    fn push_delete(&mut self, anchor: Anchor, line_num: u32) {
        self.edits.push(Edit::Delete {
            anchor,
            line_num,
            index: self.edit_index,
            old_assertion: None,
        });
        self.edit_index += 1;
    }

    fn emit_payload_rows(
        &mut self,
        cursor: Cursor,
        payloads: &[PayloadRows],
        line_num: u32,
        mode: Option<InsertMode>,
    ) {
        for p in payloads {
            self.push_insert(cursor.clone(), p.text.clone(), line_num, mode);
        }
    }

    fn validate_no_overlapping_deletes(&self) -> PResult<()> {
        let mut by_anchor: HashMap<u32, Vec<u32>> = HashMap::new();
        for edit in &self.edits {
            if let Edit::Delete {
                anchor, line_num, ..
            } = edit
            {
                let v = by_anchor.entry(anchor.line).or_default();
                if !v.contains(line_num) {
                    v.push(*line_num);
                }
            }
        }
        for (anchor_line, mut source_lines) in by_anchor {
            if source_lines.len() < 2 {
                continue;
            }
            source_lines.sort_unstable();
            let first = source_lines[0];
            let second = source_lines[1];
            return Err(perr(
                second,
                format!(
                    "anchor line {anchor_line} is already targeted by another hunk on line \
                     {first}. Issue ONE hunk per range; payload is only the final desired \
                     content, never a before/after pair."
                ),
            ));
        }
        Ok(())
    }
}

// ── Standalone diff → edits ──────────────────────────────────────────────

/// Parse a single section's diff body into `(edits, warnings)`. Mirrors omp
/// `parsePatch`.
pub fn parse_patch(diff: &str) -> Result<(Vec<Edit>, Vec<String>), HashlineError> {
    let mut tokenizer = Tokenizer::new();
    let mut executor = Executor::new();
    for token in tokenizer.tokenize_all(diff) {
        executor.feed(token).map_err(ParseError::into_hash)?;
    }
    executor.finish().map_err(ParseError::into_hash)
}

/// Streaming-tolerant variant of [`parse_patch`]. Mirrors omp
/// `parsePatchStreaming`.
pub fn parse_patch_streaming(diff: &str) -> Result<(Vec<Edit>, Vec<String>), HashlineError> {
    let mut tokenizer = Tokenizer::new();
    let mut executor = Executor::new();
    for token in tokenizer.tokenize_all(diff) {
        executor.feed(token).map_err(ParseError::into_hash)?;
    }
    executor.finish_streaming().map_err(ParseError::into_hash)
}

// ── Envelope splitting (input.ts) ────────────────────────────────────────

#[derive(Debug, Clone)]
struct RawSection {
    path: String,
    file_hash: Option<String>,
    diff: String,
}

fn unquote_hashline_path(path_text: &str) -> String {
    let bytes = path_text.as_bytes();
    if bytes.len() < 2 {
        return path_text.to_string();
    }
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    if (first == b'"' || first == b'\'') && first == last {
        path_text[1..bytes.len() - 1].to_string()
    } else {
        path_text.to_string()
    }
}

fn consume_stars(bytes: &[u8], mut i: usize, n: usize) -> usize {
    let mut count = 0;
    while i < n && bytes[i] == b'*' && count < 3 {
        i += 1;
        count += 1;
    }
    i
}

fn match_word_ci(bytes: &[u8], i: usize, n: usize, word: &[u8]) -> Option<usize> {
    if i + word.len() <= n && bytes[i..i + word.len()].eq_ignore_ascii_case(word) {
        Some(i + word.len())
    } else {
        None
    }
}

fn find_keyword_colon(bytes: &[u8], verb_end: usize, n: usize) -> Option<usize> {
    let mut j = verb_end;
    while j < n && bytes[j] != b':' && !is_alnum(bytes[j]) {
        j += 1;
    }
    if j < n && bytes[j] == b':' {
        return Some(j);
    }
    if j < n && is_alnum(bytes[j]) {
        let after_word =
            match_word_ci(bytes, j, n, b"file").or_else(|| match_word_ci(bytes, j, n, b"to"));
        let after_word = after_word?;
        let mut k = after_word;
        while k < n && bytes[k] != b':' && !is_alnum(bytes[k]) {
            k += 1;
        }
        if k < n && bytes[k] == b':' {
            return Some(k);
        }
    }
    None
}

/// Strip apply_patch-style noise models prepend to the path
/// (`***`, `Update File:`, `Add File:`, `Move to:` …). Mirrors omp
/// `stripApplyPatchPathNoise` / `APPLY_PATCH_PATH_NOISE_RE`.
fn strip_apply_patch_path_noise(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = consume_stars(bytes, 0, n);
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    let after_lead = i;
    if let Some(verb_end) = ["update", "add", "delete", "move"]
        .iter()
        .find_map(|v| match_word_ci(bytes, i, n, v.as_bytes()))
    {
        if let Some(colon) = find_keyword_colon(bytes, verb_end, n) {
            i = colon + 1;
        } else {
            i = after_lead;
        }
    }
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    i = consume_stars(bytes, i, n);
    while i < n && is_ws(bytes[i]) {
        i += 1;
    }
    s[i..].to_string()
}

/// Detect a trailing `#XXXX` (4-hex) snapshot tag at the end of a body.
/// Returns `(hash_uppercase, index_of_hash)`.
fn trailing_hash(body: &str) -> Option<(String, usize)> {
    let trimmed = body.trim_end();
    let bytes = trimmed.as_bytes();
    let n = bytes.len();
    if n < HL_FILE_HASH_LENGTH + 1 {
        return None;
    }
    let hash_start = n - HL_FILE_HASH_LENGTH;
    if hash_start == 0 || bytes[hash_start - 1] != b'#' {
        return None;
    }
    for &b in &bytes[hash_start..n] {
        if !is_hex(b) {
            return None;
        }
    }
    Some((trimmed[hash_start..n].to_uppercase(), hash_start - 1))
}

/// Best-effort recovery for bracketed header lines the strict tokenizer
/// rejects. Mirrors omp `tryParseRecoveryHeader`.
fn try_parse_recovery_header(line: &str, cwd: Option<&Path>) -> Option<RawSection> {
    if !line.starts_with(HL_FILE_PREFIX) || !line.ends_with(HL_FILE_SUFFIX) {
        return None;
    }
    let inner_start = HL_FILE_PREFIX.len();
    let inner_end = line.len().saturating_sub(HL_FILE_SUFFIX.len());
    if inner_start >= inner_end {
        return None;
    }
    let body = strip_apply_patch_path_noise(line[inner_start..inner_end].trim());
    if body.is_empty() {
        return None;
    }
    let (path_text, file_hash) = match trailing_hash(&body) {
        Some((hash, idx)) => (body[..idx].to_string(), Some(hash)),
        None => (body.trim_end().to_string(), None),
    };
    if path_text.contains('#') {
        return None;
    }
    let path = normalize_hashline_path(&path_text, cwd);
    if path.is_empty() {
        return None;
    }
    Some(RawSection {
        path,
        file_hash,
        diff: String::new(),
    })
}

/// Lexically normalize an absolute path: drop `.` components and resolve `..`
/// against preceding normal components. No symlink resolution (pure lib).
fn lexical_normalize_abs(p: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Compute the relative path from `from` (a directory) to `to`. Both must be
/// absolute; returns `None` otherwise.
fn lexical_relative(from: &Path, to: &Path) -> Option<std::path::PathBuf> {
    if !from.is_absolute() || !to.is_absolute() {
        return None;
    }
    let from_c: Vec<Component<'_>> = from.components().collect();
    let to_c: Vec<Component<'_>> = to.components().collect();
    let mut i = 0;
    while i < from_c.len() && i < to_c.len() && from_c[i] == to_c[i] {
        i += 1;
    }
    let mut result = std::path::PathBuf::new();
    for _ in i..from_c.len() {
        result.push("..");
    }
    for c in &to_c[i..] {
        result.push(c.as_os_str());
    }
    Some(result)
}

/// Normalize a header path: unquote, strip apply_patch noise, and (when `cwd`
/// is given) resolve an absolute path to a cwd-relative form. Mirrors omp
/// `normalizeHashlinePath`.
fn normalize_hashline_path(raw_path: &str, cwd: Option<&Path>) -> String {
    let unquoted = strip_apply_patch_path_noise(&unquote_hashline_path(raw_path.trim()));
    let Some(cwd) = cwd else {
        return unquoted;
    };
    let p = Path::new(&unquoted);
    if !p.is_absolute() {
        return unquoted;
    }
    let cwd_abs = lexical_normalize_abs(cwd);
    let target_abs = lexical_normalize_abs(p);
    let Some(rel) = lexical_relative(&cwd_abs, &target_abs) else {
        return unquoted;
    };
    let normalized = rel.to_string_lossy().replace('\\', "/");
    if normalized.is_empty() {
        ".".to_string()
    } else if normalized.starts_with("..") {
        unquoted
    } else {
        normalized
    }
}

/// Parse a `[PATH]` / `[PATH#HASH]` header line. `Ok(None)` for non-bracketed
/// lines; `Err` for bracketed lines whose strict shape fails AND recovery
/// cannot salvage them. Mirrors omp `parseHashlineHeaderLine`.
fn parse_hashline_header_line(
    line: &str,
    cwd: Option<&Path>,
) -> Result<Option<RawSection>, HashlineError> {
    let trimmed = line.trim_end();
    if !trimmed.starts_with(HL_FILE_PREFIX) {
        return Ok(None);
    }
    let token = classify_line(trimmed, 0);
    if !matches!(token, Token::Header { .. }) {
        if let Some(recovered) = try_parse_recovery_header(trimmed, cwd) {
            return Ok(Some(recovered));
        }
        return Err(HashlineError::parse(
            0,
            format!(
                "Input header must be {p}PATH{e} or {p}PATH{s}TAG{e} with a {len}-hex \
                 content-hash tag; got `{trimmed}`.",
                p = HL_FILE_PREFIX,
                e = HL_FILE_SUFFIX,
                s = HL_FILE_HASH_SEP,
                len = HL_FILE_HASH_LENGTH
            ),
        ));
    }
    let Token::Header {
        path, file_hash, ..
    } = token
    else {
        unreachable!("matched Header above");
    };
    let parsed_path = normalize_hashline_path(&path, cwd);
    if parsed_path.is_empty() {
        return Err(HashlineError::parse(
            0,
            format!(
                "Input header `{p}{e}` is empty; provide a file path.",
                p = HL_FILE_PREFIX,
                e = HL_FILE_SUFFIX
            ),
        ));
    }
    Ok(Some(RawSection {
        path: parsed_path,
        file_hash,
        diff: String::new(),
    }))
}

/// Strip a leading BOM and any leading blank / `*** Begin Patch` lines.
/// Mirrors omp `stripLeadingBlankLines`.
fn strip_leading_blank_lines(input: &str) -> String {
    let stripped = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut lines = split_hashline_lines(stripped);
    let tok = Tokenizer::new();
    let mut idx = 0;
    while idx < lines.len() {
        let head = &lines[idx];
        if head.trim().is_empty() || matches!(tok.tokenize(head, 0), Token::EnvelopeBegin { .. }) {
            idx += 1;
            continue;
        }
        break;
    }
    if idx == 0 {
        return stripped.to_string();
    }
    lines.drain(..idx);
    lines.join("\n")
}

fn flush_section(
    current: &mut Option<RawSection>,
    current_lines: &mut Vec<String>,
    sections: &mut Vec<RawSection>,
) {
    let Some(mut section) = current.take() else {
        current_lines.clear();
        return;
    };
    let has_ops = current_lines.iter().any(|l| !l.trim().is_empty());
    if has_ops {
        section.diff = current_lines.join("\n");
        sections.push(section);
    }
    current_lines.clear();
}

/// Split an authored patch into raw sections (path + hash + diff body).
/// Mirrors omp `splitRawSections`.
fn split_raw_sections(input: &str, cwd: Option<&Path>) -> Result<Vec<RawSection>, HashlineError> {
    let stripped = strip_leading_blank_lines(input);
    let lines = split_hashline_lines(&stripped);

    let first_line = lines.first().map(String::as_str).unwrap_or("");
    if parse_hashline_header_line(first_line, cwd)?.is_none() {
        let first_trimmed = first_line.trim_end();
        if is_unified_diff_hunk(first_trimmed) {
            return Err(HashlineError::parse(
                1,
                "unified-diff hunk header (`@@ -N,M +N,M @@`) is not valid in hashline. File \
                 sections start with `[path#HASH]`; use `replace`, `delete`, or `insert` ops."
                    .to_string(),
            ));
        }
        let preview = json_truncated(first_line, 120);
        let example = format!(
            "{p}src/foo.ts{s}1A2B{e}",
            p = HL_FILE_PREFIX,
            s = HL_FILE_HASH_SEP,
            e = HL_FILE_SUFFIX,
        );
        return Err(HashlineError::parse(
            1,
            format!(
                "input must begin with `{p}PATH{s}HASH{e}` on the first non-blank line for \
                 anchored edits; got: {preview}. Example: `{example}` then edit ops.",
                p = HL_FILE_PREFIX,
                s = HL_FILE_HASH_SEP,
                e = HL_FILE_SUFFIX,
            ),
        ));
    }

    let mut sections = Vec::new();
    let mut current: Option<RawSection> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as u32;
        let token = classify_line(line, line_num);
        if matches!(token, Token::EnvelopeEnd { .. } | Token::Abort { .. }) {
            break;
        }
        if matches!(token, Token::EnvelopeBegin { .. }) {
            continue;
        }
        if line.trim_end().starts_with(HL_FILE_PREFIX)
            && let Some(header) = parse_hashline_header_line(line, cwd)?
        {
            flush_section(&mut current, &mut current_lines, &mut sections);
            current = Some(header);
            continue;
        }
        current_lines.push(line.clone());
    }
    flush_section(&mut current, &mut current_lines, &mut sections);
    Ok(sections)
}

/// Collapse consecutive/interleaved sections targeting the same path into one,
/// concatenating diff bodies. Conflicting snapshot tags error. Mirrors omp
/// `mergeSamePathSections`.
fn merge_same_path_sections(sections: Vec<RawSection>) -> Result<Vec<RawSection>, HashlineError> {
    let mut order: Vec<String> = Vec::new();
    let mut by_path: HashMap<String, (Option<String>, Vec<String>)> = HashMap::new();
    for section in sections {
        if !by_path.contains_key(&section.path) {
            order.push(section.path.clone());
            by_path.insert(section.path.clone(), (None, Vec::new()));
        }
        let entry = by_path.get_mut(&section.path).expect("just inserted");
        match (&entry.0, &section.file_hash) {
            (Some(existing), Some(new)) if existing != new => {
                return Err(HashlineError::parse(
                    0,
                    format!(
                        "Conflicting hashline snapshot tags for {path}: #{a} and #{b}. Re-read \
                         the file and retry with one current header.",
                        path = section.path,
                        a = existing,
                        b = new
                    ),
                ));
            }
            (None, Some(new)) => entry.0 = Some(new.clone()),
            _ => {}
        }
        entry.1.push(section.diff);
    }
    Ok(order
        .into_iter()
        .map(|path| {
            let (file_hash, diffs) = by_path.remove(&path).expect("path tracked in order");
            RawSection {
                path,
                file_hash,
                diff: diffs.join("\n"),
            }
        })
        .collect())
}

// ── Public API ───────────────────────────────────────────────────────────

/// One section of a parsed patch: a target file plus its eagerly-parsed edits.
#[derive(Debug, Clone)]
pub struct PatchSection {
    /// Resolved file path from the `[PATH#HASH]` header.
    pub file_path: String,
    /// 4-hex snapshot tag (empty when the header omitted one).
    pub file_hash: String,
    /// Parsed line edits for this section's diff body.
    pub edits: Vec<Edit>,
    /// Warnings emitted during parsing.
    pub warnings: Vec<String>,
}

impl PatchSection {
    fn from_raw(raw: RawSection) -> Result<Self, HashlineError> {
        let (edits, warnings) = parse_patch(&raw.diff)?;
        Ok(PatchSection {
            file_path: raw.path,
            file_hash: raw.file_hash.unwrap_or_default(),
            edits,
            warnings,
        })
    }

    /// Re-parse this section's diff body from `diff` (convenience for callers
    /// holding a raw body rather than going through [`split_patch_input`]).
    pub fn parse_diff(diff: &str) -> Result<(Vec<Edit>, Vec<String>), HashlineError> {
        parse_patch(diff)
    }
}

/// A parsed hashline patch — zero or more [`PatchSection`]s, each rooted at a
/// `[PATH#HASH]` header.
#[derive(Debug, Clone)]
pub struct Patch {
    /// Sections in first-occurrence path order.
    pub sections: Vec<PatchSection>,
}

/// Parse `text` into a [`Patch`]. Splits the `*** Begin Patch … *** End Patch`
/// envelope into `[PATH#HASH]` sections and eagerly parses each section's
/// edits.
///
/// `opts.root` resolves absolute header paths to a root-relative form; omit it
/// for paths as-authored.
pub fn split_patch_input(text: &str, opts: Option<SplitOptions>) -> Result<Patch, HashlineError> {
    let opts = opts.unwrap_or_default();
    let cwd = opts.root.as_deref();
    let raw = split_raw_sections(text, cwd)?;
    let merged = merge_same_path_sections(raw)?;
    let mut sections = Vec::with_capacity(merged.len());
    for section in merged {
        sections.push(PatchSection::from_raw(section)?);
    }
    Ok(Patch { sections })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Cursor, InsertMode};

    fn insert_text(edit: &Edit) -> &str {
        match edit {
            Edit::Insert { text, .. } => text,
            _ => "",
        }
    }

    #[test]
    fn parses_header_path_and_hash() {
        let patch = split_patch_input("[src/foo.ts#1A2B]\nSWAP 1.=2:\n+a\n+b\n", None).unwrap();
        assert_eq!(patch.sections.len(), 1);
        let s = &patch.sections[0];
        assert_eq!(s.file_path, "src/foo.ts");
        assert_eq!(s.file_hash, "1A2B");
        // one Replace → 2 inserts (before line 1) + 2 deletes (lines 1,2)
        assert_eq!(s.edits.len(), 4);
    }

    #[test]
    fn header_without_hash_is_empty_string() {
        let patch = split_patch_input("[a.ts]\nINS.TAIL:\n+z\n", None).unwrap();
        assert_eq!(patch.sections[0].file_hash, "");
    }

    #[test]
    fn envelope_markers_consumed() {
        let input = "*** Begin Patch\n[a.ts#1A2B]\nINS.TAIL:\n+z\n*** End Patch\n";
        let patch = split_patch_input(input, None).unwrap();
        assert_eq!(patch.sections[0].edits.len(), 1);
    }

    #[test]
    fn abort_marker_terminates() {
        let input = "[a.ts#1A2B]\nINS.TAIL:\n+z\n*** Abort\n[b.ts#3C4D]\nINS.TAIL:\n+q\n";
        let patch = split_patch_input(input, None).unwrap();
        assert_eq!(patch.sections.len(), 1);
    }

    #[test]
    fn swap_lowers_to_replacement_inserts_and_deletes() {
        let (edits, _) = parse_patch("SWAP 5.=7:\n+x\n+y\n").unwrap();
        let inserts: Vec<_> = edits
            .iter()
            .filter(|e| matches!(e, Edit::Insert { .. }))
            .collect();
        let deletes: Vec<_> = edits
            .iter()
            .filter(|e| matches!(e, Edit::Delete { .. }))
            .collect();
        assert_eq!(inserts.len(), 2);
        assert_eq!(deletes.len(), 3); // lines 5,6,7
        // inserts land before the range start with Replacement mode
        for e in &inserts {
            match e {
                Edit::Insert { cursor, mode, .. } => {
                    assert!(matches!(cursor, Cursor::BeforeAnchor(_)));
                    assert_eq!(*mode, Some(InsertMode::Replacement));
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn swap_empty_range_becomes_delete() {
        let (edits, _) = parse_patch("SWAP 3.=4:\n").unwrap();
        assert!(edits.iter().all(|e| matches!(e, Edit::Delete { .. })));
        assert_eq!(edits.len(), 2);
    }

    #[test]
    fn del_produces_only_deletes() {
        let (edits, _) = parse_patch("DEL 3.=5\n").unwrap();
        assert!(edits.iter().all(|e| matches!(e, Edit::Delete { .. })));
        assert_eq!(edits.len(), 3);
    }

    #[test]
    fn del_rejects_body() {
        let err = parse_patch("DEL 3.=5\n+oops\n").unwrap_err();
        assert!(matches!(err, HashlineError::Parse { .. }));
    }

    #[test]
    fn ins_variants_parse() {
        let (edits, _) = parse_patch("INS.PRE 2:\n+a\n").unwrap();
        assert_eq!(edits.len(), 1);
        assert!(matches!(
            edits[0],
            Edit::Insert { cursor: Cursor::BeforeAnchor(_), ref text, .. } if text == "a"
        ));

        let (edits, _) = parse_patch("INS.POST 2:\n+a\n").unwrap();
        assert!(matches!(
            edits[0],
            Edit::Insert {
                cursor: Cursor::AfterAnchor(_),
                ..
            }
        ));

        let (edits, _) = parse_patch("INS.HEAD:\n+a\n").unwrap();
        assert!(matches!(
            edits[0],
            Edit::Insert {
                cursor: Cursor::Bof,
                ..
            }
        ));

        let (edits, _) = parse_patch("INS.TAIL:\n+a\n").unwrap();
        assert!(matches!(
            edits[0],
            Edit::Insert {
                cursor: Cursor::Eof,
                ..
            }
        ));
    }

    #[test]
    fn ins_requires_body() {
        assert!(parse_patch("INS.HEAD:\n").is_err());
    }

    #[test]
    fn payload_literal_preserved_verbatim() {
        let (edits, _) = parse_patch("INS.TAIL:\n+  indented\n+\n+-dash-prefixed\n").unwrap();
        assert_eq!(insert_text(&edits[0]), "  indented");
        assert_eq!(insert_text(&edits[1]), "");
        assert_eq!(insert_text(&edits[2]), "-dash-prefixed");
    }

    #[test]
    fn bare_body_auto_piped_warning() {
        let (edits, warnings) = parse_patch("INS.TAIL:\nhello\n").unwrap();
        assert_eq!(insert_text(&edits[0]), "hello");
        assert!(warnings.iter().any(|w| w == BARE_BODY_AUTO_PIPED_WARNING));
    }

    #[test]
    fn minus_row_rejected() {
        let err = parse_patch("INS.TAIL:\n-bad\n").unwrap_err();
        let HashlineError::Parse { msg, .. } = err else {
            panic!("expected Parse error");
        };
        assert!(msg.contains("not valid"));
    }

    #[test]
    fn contamination_apply_patch_sentinel() {
        let msg = detect_apply_patch_contamination("*** Update File: foo", false);
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("apply_patch sentinel"));
    }

    #[test]
    fn contamination_unified_diff_hunk() {
        let msg = detect_apply_patch_contamination("@@ -1,3 +1,3 @@", false);
        assert!(msg.unwrap().contains("unified-diff hunk header"));
    }

    #[test]
    fn contamination_bare_range() {
        let msg = detect_apply_patch_contamination("5.=7:", false);
        assert!(msg.unwrap().contains("bare range hunk header"));
    }

    #[test]
    fn contamination_bare_line_number() {
        let msg = detect_apply_patch_contamination("42", false);
        assert!(msg.unwrap().contains("hunk headers need a verb"));
    }

    #[test]
    fn contamination_del_with_colon() {
        let msg = detect_apply_patch_contamination("DEL 3.=7:", false);
        assert!(msg.unwrap().contains("no colon and no body"));
    }

    #[test]
    fn first_line_not_header_errors() {
        let err = split_patch_input("SWAP 1.=2:\n+a\n", None).unwrap_err();
        assert!(matches!(err, HashlineError::Parse { .. }));
    }

    #[test]
    fn first_line_unified_diff_errors() {
        let err = split_patch_input("@@ -1,3 +1,3 @@\n+a\n", None).unwrap_err();
        let HashlineError::Parse { msg, .. } = err else {
            panic!("expected Parse");
        };
        assert!(msg.contains("unified-diff"));
    }

    #[test]
    fn conflicting_hashes_error() {
        let input = "[a.ts#1A2B]\nINS.TAIL:\n+x\n[a.ts#3C4D]\nINS.TAIL:\n+y\n";
        let err = split_patch_input(input, None).unwrap_err();
        let HashlineError::Parse { msg, .. } = err else {
            panic!("expected Parse");
        };
        assert!(msg.contains("Conflicting hashline snapshot tags"));
    }

    #[test]
    fn merge_same_path_sections() {
        let input = "[a.ts#1A2B]\nINS.TAIL:\n+x\n[a.ts#1A2B]\nINS.TAIL:\n+y\n";
        let patch = split_patch_input(input, None).unwrap();
        assert_eq!(patch.sections.len(), 1);
        assert_eq!(patch.sections[0].edits.len(), 2);
    }

    #[test]
    fn multiple_sections() {
        let input = "[a.ts#1A2B]\nINS.TAIL:\n+x\n[b.ts#3C4D]\nDEL 1\n";
        let patch = split_patch_input(input, None).unwrap();
        assert_eq!(patch.sections.len(), 2);
        assert_eq!(patch.sections[0].file_path, "a.ts");
        assert_eq!(patch.sections[1].file_path, "b.ts");
        assert_eq!(patch.sections[1].edits.len(), 1);
    }

    #[test]
    fn skippable_comment_between_hunks() {
        let input = "[a.ts#1A2B]\nINS.TAIL:\n+x\n# a comment\nINS.HEAD:\n+y\n";
        let patch = split_patch_input(input, None).unwrap();
        // comment consumed; two inserts present
        let s = &patch.sections[0];
        assert!(
            s.edits
                .iter()
                .any(|e| matches!(e, Edit::Insert { text, .. } if text == "x"))
        );
        assert!(
            s.edits
                .iter()
                .any(|e| matches!(e, Edit::Insert { text, .. } if text == "y"))
        );
    }

    #[test]
    fn strip_one_prefix() {
        assert_eq!(strip_one_leading_hashline_prefix("42:hello"), "hello");
        assert_eq!(strip_one_leading_hashline_prefix(">>>42:hi"), "hi");
        assert_eq!(strip_one_leading_hashline_prefix("+5:yo"), "yo");
        assert_eq!(strip_one_leading_hashline_prefix("hello"), "hello");
        assert_eq!(strip_one_leading_hashline_prefix("12:30"), "30"); // \d+: strips the first "12:" prefix (timestamp edge case)
    }

    #[test]
    fn strip_prefix_uniform() {
        // All bare rows carry N: prefix → stripped.
        let diff = "INS.TAIL:\n1:a\n2:b\n";
        let (edits, _) = parse_patch(diff).unwrap();
        assert_eq!(insert_text(&edits[0]), "a");
        assert_eq!(insert_text(&edits[1]), "b");
    }

    #[test]
    fn strip_prefix_not_uniform_keeps() {
        // Mixed: one prefixed, one not → keep both as-is.
        let diff = "INS.TAIL:\n1:a\nb\n";
        let (edits, _) = parse_patch(diff).unwrap();
        assert_eq!(insert_text(&edits[0]), "1:a");
        assert_eq!(insert_text(&edits[1]), "b");
    }

    #[test]
    fn quoted_path_unquoted() {
        let patch = split_patch_input("[\"src/foo.ts\"#1A2B]\nINS.TAIL:\n+z\n", None).unwrap();
        assert_eq!(patch.sections[0].file_path, "src/foo.ts");
    }

    #[test]
    fn recovery_header_strips_noise() {
        let patch =
            split_patch_input("[*** Update File:src/foo.ts#1A2B]\nINS.TAIL:\n+z\n", None).unwrap();
        assert_eq!(patch.sections[0].file_path, "src/foo.ts");
    }

    #[test]
    fn overlapping_deletes_error() {
        let err = parse_patch("DEL 5.=7\nDEL 6\n").unwrap_err();
        let HashlineError::Parse { msg, .. } = err else {
            panic!("expected Parse");
        };
        assert!(msg.contains("already targeted"));
    }

    #[test]
    fn range_order_validated() {
        let err = parse_patch("SWAP 7.=3:\n+x\n").unwrap_err();
        assert!(matches!(err, HashlineError::Parse { .. }));
    }
}
