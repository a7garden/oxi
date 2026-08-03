//! Error type raised when a section's snapshot tag does not match the live file
//! content and recovery is unavailable / has failed.
//!
//! Carries enough context to render a useful diagnostic: the anchored lines
//! plus a couple of lines of surrounding context. [`MismatchError`] formats
//! this into a message at construction time.
//!
//! Ported from omp `packages/hashline/src/mismatch.ts`.

use crate::format::{HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX};
use crate::messages::format_anchored_context;

/// Example content-hash tag shown in the anchor-requirement diagnostic.
/// (omp exposes this as `HL_FILE_HASH_EXAMPLES[0]`; the format module here does
/// not, so the first example is repeated locally.)
const EXAMPLE_HASH: &str = "1A2B";

// ── Details ──────────────────────────────────────────────────────────────

/// Diagnostic context carried by a [`MismatchError`].
#[derive(Debug, Clone)]
pub struct MismatchDetails {
    /// Canonical path, when known.
    pub path: Option<String>,
    /// Hash tag the section was bound to.
    pub expected_file_hash: String,
    /// Hash tag the live file actually produced.
    pub actual_file_hash: String,
    /// Live file lines, for an anchored-context preview.
    pub file_lines: Vec<String>,
    /// 1-indexed lines the edit anchored to.
    pub anchor_lines: Vec<u32>,
    /// `true` when the expected hash resolved to a recorded snapshot (file
    /// content drifted since that snapshot); `false` when no snapshot was ever
    /// recorded for the hash (likely fabricated or carried over from a prior
    /// session). Drives a more actionable rejection message.
    pub hash_recognized: bool,
}

impl Default for MismatchDetails {
    fn default() -> Self {
        // omp defaults `hashRecognized` to `true` for backward compatibility.
        Self {
            path: None,
            expected_file_hash: String::new(),
            actual_file_hash: String::new(),
            file_lines: Vec::new(),
            anchor_lines: Vec::new(),
            hash_recognized: true,
        }
    }
}

// ── Error ────────────────────────────────────────────────────────────────

/// Raised when a hashline section's snapshot tag doesn't match the live file's
/// content (and recovery, if configured, declined the merge). Implements
/// [`std::error::Error`]; its [`Display`](std::fmt::Display) is the formatted
/// diagnostic produced by [`format_message`].
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct MismatchError {
    /// Pre-formatted diagnostic (header + anchored context).
    pub message: String,
    /// Structured context behind the diagnostic.
    pub details: MismatchDetails,
}

impl MismatchError {
    /// Build the error, computing its message from `details`.
    pub fn new(details: MismatchDetails) -> Self {
        let message = format_message(&details);
        Self { message, details }
    }

    /// The formatted diagnostic (alias for the [`Display`](std::fmt::Display)
    /// rendering).
    pub fn display_message(&self) -> &str {
        &self.message
    }
}

// ── Crate umbrella error ─────────────────────────────────────────────────

/// The umbrella error type for the hashline crate.
///
/// Structured errors raised across the parser, tokenizer, patcher, and recovery
/// paths. Defined here (per the omp-adoption design §3.10) so the lower-level
/// tokenizer/parser can reference it without an extra module.
#[derive(Debug, thiserror::Error)]
pub enum HashlineError {
    /// A patch grammar / structural parse error at a given line.
    #[error("Parse error at line {line}: {msg}")]
    Parse {
        /// 1-indexed source line where the parse error occurred.
        line: u32,
        /// Human-readable parse error detail.
        msg: String,
    },
    /// File does not exist.
    #[error("File not found: {path}. Use the write tool to create new files.")]
    NotFound {
        /// Canonical file path that could not be found.
        path: String,
    },
    /// Section omitted the mandatory snapshot tag (omp `missingSnapshotTagMessage`).
    #[error("{0}")]
    MissingSnapshotTag(String),
    /// An anchored edit referenced unseen lines (omp `unseenLinesMessage`).
    #[error("{0}")]
    UnseenLines(String),
    /// Snapshot tag mismatch with live content (omp `MismatchError`).
    #[error("{detail}")]
    Mismatch {
        /// Pre-formatted diagnostic message.
        detail: String,
        /// Snapshot tag the section was bound to.
        expected: String,
        /// Snapshot tag the live file actually produced.
        actual: String,
    },
    /// Multiple sections resolve to the same canonical path.
    #[error("Multiple sections resolve to {path}")]
    DuplicateCanonicalPath {
        /// Canonical path targeted by more than one section.
        path: String,
    },
    /// Edits resulted in no net change.
    #[error("Edits to {path} resulted in no changes")]
    NoOp {
        /// Path whose applied edits produced no net change.
        path: String,
    },
    /// Anchor line out of bounds for the file.
    #[error("Line {line} does not exist (file has {total} lines)")]
    LineOutOfBounds {
        /// 1-indexed anchor line that exceeds the file's bounds.
        line: u32,
        /// Total number of lines in the file.
        total: usize,
    },
    /// Underlying I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// No block resolver configured (block-ops feature only).
    #[cfg(feature = "block-ops")]
    #[error("Block resolver unavailable for {path}")]
    BlockResolverUnavailable { path: String },
}

impl HashlineError {
    /// Construct a [`HashlineError::Parse`] (the form the tokenizer raises).
    pub fn parse(line: u32, msg: impl Into<String>) -> Self {
        HashlineError::Parse {
            line,
            msg: msg.into(),
        }
    }
}

// ── Message builders ─────────────────────────────────────────────────────

/// The two-line rejection header, branching on [`MismatchDetails::hash_recognized`].
pub fn rejection_header(details: &MismatchDetails) -> Vec<String> {
    let path_text = details
        .path
        .as_deref()
        .map(|p| format!(" for {p}"))
        .unwrap_or_default();
    if !details.hash_recognized {
        vec![
            format!(
                "Edit rejected{path_text}: hash {sep}{expected} is not from this session.",
                sep = HL_FILE_HASH_SEP,
                expected = details.expected_file_hash,
            ),
            format!(
                "The current file hashes to {sep}{actual}. Re-read the file with `read` to copy a \
                 current {pfx}path{sep}tag{sfx} header — never invent the tag and never reuse one \
                 from a prior session.",
                sep = HL_FILE_HASH_SEP,
                actual = details.actual_file_hash,
                pfx = HL_FILE_PREFIX,
                sfx = HL_FILE_SUFFIX,
            ),
        ]
    } else {
        vec![
            format!("Edit rejected{path_text}: file changed between read and edit."),
            format!(
                "Section is bound to {sep}{expected}, but the current file hashes to \
                 {sep}{actual}. If a prior edit in this session modified this file, copy the \
                 {pfx}path{sep}newhash{sfx} header from that edit's response; otherwise re-read \
                 the file with `read` to refresh the tag before retrying.",
                sep = HL_FILE_HASH_SEP,
                expected = details.expected_file_hash,
                actual = details.actual_file_hash,
                pfx = HL_FILE_PREFIX,
                sfx = HL_FILE_SUFFIX,
            ),
        ]
    }
}

/// Full diagnostic: the rejection header followed by an anchored-context
/// preview (when anchor lines and file lines are available).
pub fn format_message(details: &MismatchDetails) -> String {
    let mut lines = rejection_header(details);
    let context = format_anchored_context(&details.anchor_lines, &details.file_lines);
    if context.is_empty() {
        lines.join("\n")
    } else {
        lines.push(String::new());
        lines.extend(context);
        lines.join("\n")
    }
}

/// Alias of [`format_message`] (omp's `formatDisplayMessage`).
pub fn format_display_message(details: &MismatchDetails) -> String {
    format_message(details)
}

/// Throws (returns `Err`) when the line reference is out of bounds for the
/// given file.
pub fn validate_line_ref(line: u32, file_lines: &[String]) -> Result<(), String> {
    if line < 1 || (line as usize) > file_lines.len() {
        return Err(format!(
            "Line {line} does not exist (file has {} lines)",
            file_lines.len()
        ));
    }
    Ok(())
}

/// Format the required-shape diagnostic shown when a line reference is malformed.
pub fn format_full_anchor_requirement(raw: Option<&str>) -> String {
    let received = match raw {
        Some(r) => format!(" Received {r:?}."),
        None => String::new(),
    };
    format!(
        "a bare line number from read/search output plus the section header content-hash tag \
         (for example {pfx}src/foo.ts{sep}{ex}{sfx} and line \"160\"){received}",
        pfx = HL_FILE_PREFIX,
        sep = HL_FILE_HASH_SEP,
        ex = EXAMPLE_HASH,
        sfx = HL_FILE_SUFFIX,
    )
}

/// Parse a decorated bare line-number anchor like `42`, `*42:foo`, `> 7`.
///
/// Equivalent to omp's `parseTag` regex `/^\s*[>+\-*]*\s*(\d+)(?::.*)?\s*$/`.
/// Returns the parsed 1-indexed line number, or an error whose message is the
/// required-shape diagnostic.
pub fn parse_tag(reference: &str) -> Result<u32, String> {
    match try_parse_line_ref(reference) {
        Some(line) if line >= 1 => Ok(line),
        Some(line) => Err(format!(
            "Line number must be >= 1, got {line} in {reference:?}."
        )),
        None => Err(format!(
            "Invalid line reference. Expected {}. Expected {}.",
            reference,
            format_full_anchor_requirement(Some(reference))
        )),
    }
}

/// Inner worker for [`parse_tag`]: matches omp's `LINE_REF_RE`.
fn try_parse_line_ref(reference: &str) -> Option<u32> {
    let s = reference.trim_start();
    let bytes = s.as_bytes();
    // Skip optional leading decorators [> + - *].
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b'>' | b'+' | b'-' | b'*') {
        i += 1;
    }
    let rest = &s[i..];
    let rest = rest.trim_start();
    // A run of ASCII digits is the line number.
    let digit_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }
    let line: u32 = rest[..digit_len].parse().ok()?;
    // After the digits: optional `:…`, then trailing whitespace only.
    let tail = rest[digit_len..].trim_end();
    if tail.is_empty() || tail.starts_with(':') {
        Some(line)
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn details(expected: &str, actual: &str, recognized: bool) -> MismatchDetails {
        MismatchDetails {
            path: Some("src/foo.rs".to_string()),
            expected_file_hash: expected.to_string(),
            actual_file_hash: actual.to_string(),
            file_lines: vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
            anchor_lines: vec![2],
            hash_recognized: recognized,
        }
    }

    #[test]
    fn recognized_mismatch_renders_header_and_context() {
        let d = details("AAAA", "BBBB", true);
        let err = MismatchError::new(d);
        assert!(err.message.contains("file changed between read and edit"));
        assert!(err.message.contains("#AAAA"));
        assert!(err.message.contains("#BBBB"));
        // Anchored context is separated from the header by a blank line; the
        // anchor line itself appears within the context window.
        assert!(err.message.contains("\n\n"));
        assert!(
            err.message.contains("*2:b"),
            "anchored line 2 appears in context"
        );
        // Structured fields are preserved.
        assert_eq!(err.details.expected_file_hash, "AAAA");
        assert_eq!(err.details.actual_file_hash, "BBBB");
        assert!(err.details.hash_recognized);
    }

    #[test]
    fn unrecognized_mismatch_uses_fabrication_message() {
        let d = details("AAAA", "BBBB", false);
        let msg = format_message(&d);
        assert!(msg.contains("is not from this session"));
        assert!(msg.contains("never invent the tag"));
    }

    #[test]
    fn no_context_when_file_lines_absent() {
        let d = MismatchDetails {
            path: None,
            expected_file_hash: "AAAA".into(),
            actual_file_hash: "BBBB".into(),
            file_lines: Vec::new(),
            anchor_lines: Vec::new(),
            hash_recognized: true,
        };
        let msg = format_message(&d);
        assert!(!msg.contains("\n\n"));
        assert!(msg.contains("Edit rejected"));
    }

    #[test]
    fn validate_line_ref_bounds() {
        let file: Vec<String> = vec!["x".into(), "y".into()];
        assert!(validate_line_ref(1, &file).is_ok());
        assert!(validate_line_ref(2, &file).is_ok());
        assert!(validate_line_ref(0, &file).is_err());
        assert!(validate_line_ref(3, &file).is_err());
    }

    #[test]
    fn parse_tag_accepts_decorated_refs() {
        assert_eq!(parse_tag("42").unwrap(), 42);
        assert_eq!(parse_tag("  *42:foo").unwrap(), 42);
        assert_eq!(parse_tag(" > 7").unwrap(), 7);
        assert_eq!(parse_tag("160:some content").unwrap(), 160);
    }

    #[test]
    fn parse_tag_rejects_garbage() {
        assert!(parse_tag("not a line").is_err());
        assert!(parse_tag(":42").is_err());
        assert!(parse_tag("").is_err());
        // Non-decorator, non-digit content after the number fails.
        assert!(parse_tag("42 extra").is_err());
    }

    #[test]
    fn mismatch_error_implements_std_error() {
        let err = MismatchError::new(details("AAAA", "BBBB", true));
        // Ensures the thiserror `Error` + `Display` derive is wired.
        let _: &dyn std::error::Error = &err;
        assert_eq!(err.display_message(), format!("{}", err));
    }

    #[test]
    fn format_full_anchor_requirement_includes_example() {
        let req = format_full_anchor_requirement(None);
        assert!(req.contains("[src/foo.ts#1A2B]"));
        assert!(req.contains("\"160\""));
        let req_with = format_full_anchor_requirement(Some("xyz"));
        assert!(req_with.contains("Received \"xyz\"."));
    }
}
