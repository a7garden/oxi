//! Hashline format primitives: sigils, separators, and display helpers.
//! Single source of truth for the parser, tokenizer, prompt, and grammar.
//!
//! Ported from omp `packages/hashline/src/format.ts`.

use crate::types::Cursor;

// ── Sigils & separators ──────────────────────────────────────────────────

/// Opening sigil of a hashline section header.
pub const HL_FILE_PREFIX: &str = "[";
/// Closing sigil of a hashline section header.
pub const HL_FILE_SUFFIX: &str = "]";
/// Sigil prefixing each literal payload row.
pub const HL_PAYLOAD_REPLACE: char = '+';

/// Keyword marking a replacement hunk header.
pub const HL_REPLACE_KEYWORD: &str = "SWAP";
/// Keyword marking a deletion hunk header.
pub const HL_DELETE_KEYWORD: &str = "DEL";
/// Keyword marking an insertion hunk header.
pub const HL_INSERT_KEYWORD: &str = "INS";
/// `INS` sub-keyword: insert before the anchor line.
pub const HL_INSERT_BEFORE: &str = "PRE";
/// `INS` sub-keyword: insert after the anchor line.
pub const HL_INSERT_AFTER: &str = "POST";
/// `INS` sub-keyword: insert at the very top of the file.
pub const HL_INSERT_HEAD: &str = "HEAD";
/// `INS` sub-keyword: insert at the very bottom of the file.
pub const HL_INSERT_TAIL: &str = "TAIL";

/// Colon terminating a hunk header.
pub const HL_HEADER_COLON: char = ':';
/// Separator between a path and its hash tag.
pub const HL_FILE_HASH_SEP: char = '#';
/// Separator in an inclusive `start.=end` range.
pub const HL_RANGE_SEP: &str = ".=";
/// Separator between a line number and its body text.
pub const HL_LINE_BODY_SEP: char = ':';

/// Length, in hex characters, of a content hash tag.
pub const HL_FILE_HASH_LENGTH: usize = 4;

// ── Normalisation & hashing ──────────────────────────────────────────────

/// Trim trailing `[ \t\r]` from every line (and the final line) in a single
/// pass so CRLF endings and display-trimmed lines do not invalidate a tag.
///
/// Equivalent to omp's `text.replace(/[ \t\r]+(?=\n|$)/g, "")`.
fn normalize_file_hash_text(text: &str) -> String {
    // Manual scan to avoid a regex dependency.
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let len = bytes.len();
    while i < len {
        // Check if we're at a run of [ \t\r] followed by \n or end-of-string.
        if matches!(bytes[i], b' ' | b'\t' | b'\r') {
            // Find the end of the whitespace run.
            let start = i;
            while i < len && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                i += 1;
            }
            // If the next char is \n or end-of-string, drop the run.
            if i >= len || bytes[i] == b'\n' {
                // skip — don't copy the whitespace
            } else {
                out.extend_from_slice(&bytes[start..i]);
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Safety: input is valid UTF-8, and we only remove ASCII whitespace bytes.
    String::from_utf8(out).expect("normalization preserves UTF-8 validity")
}

/// Compute the content-derived hash tag carried by a hashline section header.
///
/// xxHash32 seed 0, low 16 bits, 4-hex uppercase.
/// Must be byte-identical to omp's `computeFileHash`.
pub fn compute_file_hash(text: &str) -> String {
    let normalized = normalize_file_hash_text(text);
    let low16 = xxhash_rust::xxh32::xxh32(normalized.as_bytes(), 0) & 0xFFFF;
    format!("{:04X}", low16)
}

// ── Display helpers ──────────────────────────────────────────────────────

/// Format a concrete replacement hunk header: `SWAP start.=end:`.
pub fn format_replace_header(start: u32, end: u32) -> String {
    format!("{HL_REPLACE_KEYWORD} {start}{HL_RANGE_SEP}{end}{HL_HEADER_COLON}")
}

/// Format a concrete deletion hunk header: `DEL start` or `DEL start.=end`.
pub fn format_delete_header(start: u32, end: u32) -> String {
    if start == end {
        format!("{HL_DELETE_KEYWORD} {start}")
    } else {
        format!("{HL_DELETE_KEYWORD} {start}{HL_RANGE_SEP}{end}")
    }
}

/// Format an insertion hunk header for a cursor position.
pub fn format_insert_header(cursor: &Cursor) -> String {
    match cursor {
        Cursor::BeforeAnchor(anchor) => {
            format!(
                "{HL_INSERT_KEYWORD}.{HL_INSERT_BEFORE} {}{HL_HEADER_COLON}",
                anchor.line
            )
        }
        Cursor::AfterAnchor(anchor) => {
            format!(
                "{HL_INSERT_KEYWORD}.{HL_INSERT_AFTER} {}{HL_HEADER_COLON}",
                anchor.line
            )
        }
        Cursor::Bof => {
            format!("{HL_INSERT_KEYWORD}.{HL_INSERT_HEAD}{HL_HEADER_COLON}")
        }
        Cursor::Eof => {
            format!("{HL_INSERT_KEYWORD}.{HL_INSERT_TAIL}{HL_HEADER_COLON}")
        }
    }
}

/// Format a hashline section header: `[path#HASH]`.
pub fn format_hashline_header(file_path: &str, file_hash: &str) -> String {
    format!("{HL_FILE_PREFIX}{file_path}{HL_FILE_HASH_SEP}{file_hash}{HL_FILE_SUFFIX}")
}

/// Format a single numbered line: `LINE:TEXT`.
pub fn format_numbered_line(line_number: u32, line: &str) -> String {
    format!("{line_number}{HL_LINE_BODY_SEP}{line}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Anchor;

    #[test]
    fn hash_stable_and_uppercase() {
        let tag = compute_file_hash("hello world\n");
        assert_eq!(tag.len(), 4);
        assert!(
            tag.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn hash_trailing_whitespace_invariant() {
        let a = compute_file_hash("line one\nline two\n");
        let b = compute_file_hash("line one   \nline two\t\r\n");
        assert_eq!(a, b, "trailing whitespace must not change hash");
    }

    #[test]
    fn hash_crlf_invariant() {
        let a = compute_file_hash("alpha\nbeta\n");
        let b = compute_file_hash("alpha\r\nbeta\r\n");
        assert_eq!(a, b, "CRLF must not change hash");
    }

    #[test]
    fn hash_empty_string() {
        let tag = compute_file_hash("");
        assert_eq!(tag.len(), 4);
    }

    #[test]
    fn format_headers() {
        assert_eq!(format_replace_header(5, 10), "SWAP 5.=10:");
        assert_eq!(format_delete_header(7, 7), "DEL 7");
        assert_eq!(format_delete_header(7, 12), "DEL 7.=12");
        assert_eq!(
            format_insert_header(&Cursor::BeforeAnchor(Anchor { line: 3 })),
            "INS.PRE 3:"
        );
        assert_eq!(
            format_hashline_header("src/foo.rs", "1A2B"),
            "[src/foo.rs#1A2B]"
        );
        assert_eq!(format_numbered_line(42, "let x = 1;"), "42:let x = 1;");
    }
}
