//! Content sanitizer — ported from omp [`core/content-sanitizer.ts`].
//!
//! Detects and strips data URIs and high-entropy base64-style blobs from
//! memory content before it is stored, replacing each with a compact
//! placeholder plus a [`BlobMetadata`] describing the extracted payload.
//!
//! The .ts implementation persists blobs to disk via `storeBlob` (writing
//! under `~/.hermes/mnemopi/blobs/<sha[0:2]>/<sha[0:4]>/<sha>`) and returns
//! a `blob://sha256/<hex>` reference. The Rust port keeps the same
//! reference format and SHA256 identity but does not touch the filesystem
//! — callers can use [`compute_sha256`] to verify and persist if needed.
//! This keeps the sanitizer pure and side-effect-free, which matches the
//! contract specified for the port.
//!
//! ## Thresholds
//!
//! - [`SIZE_HARD_CAP`] (1 MB) — anything larger is unconditionally
//!   extracted, regardless of entropy.
//! - [`SIZE_BASE64_CHECK`] (100 KB) — payloads above this size are
//!   entropy-tested; if they look like random bytes (entropy above
//!   [`ENTROPY_THRESHOLD`]) they're extracted as a blob.
//!
//! ## Deviations from the .ts source
//!
//! - The .ts `looksLikeBase64Blob` is `length >= 100KB && entropy > 5.0`.
//!   The assignment spec adds a "all base64 chars" requirement; we follow
//!   the spec.
//! - The .ts uses `>` at SIZE_BASE64_CHECK; the assignment uses `>`.
//!   We match the assignment exactly.
//! - `shannonEntropy` in .ts iterates code points and divides by char
//!   count. The Rust port uses `chars().count()` for the denominator so
//!   non-ASCII input produces the correct entropy.
//!
//! MIT — adapted from
//! [omp](https://github.com/can1357/oh-my-pi)
//! `packages/mnemopi/src/core/content-sanitizer.ts`.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ── Constants ────────────────────────────────────────────────────────────

/// Hard size cap — content larger than this is unconditionally extracted.
pub const SIZE_HARD_CAP: usize = 1_000_000;

/// Lower bound for entropy-based base64 detection.
pub const SIZE_BASE64_CHECK: usize = 100_000;

/// Shannon-entropy threshold (bits/char) above which a long payload is
/// treated as a high-entropy base64-style blob.
pub const ENTROPY_THRESHOLD: f64 = 5.0;

// ── Public types ────────────────────────────────────────────────────────

/// Metadata for an extracted blob (data URI, oversized payload, or
/// high-entropy base64-style content).
///
/// `blob_ref` is formatted as `blob://sha256/<hex>`, matching the .ts
/// reference scheme. `extraction_reason` is one of `"data_uri"`,
/// `"size_cap"`, or `"high_entropy"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BlobMetadata {
    pub blob_ref: Option<String>,
    pub original_size: usize,
    pub mime: Option<String>,
    pub extraction_reason: Option<String>,
    pub entropy: Option<f64>,
}

// ── Internal regexes ────────────────────────────────────────────────────

/// `data:<mime>?;base64?,<payload>` — captures optional mime and the
/// payload after the comma. Mirrors the .ts `DATA_URI_RE`.
static DATA_URI_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"^data:(?<mime>[^;,]+)?(?:;base64)?,(?<payload>.*)$").expect("valid data-URI regex")
});

/// Standard base64 alphabet — `=` padding allowed only at the tail.
static BASE64_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$")
        .expect("valid base64 regex")
});

// ── Helpers ─────────────────────────────────────────────────────────────

/// SHA256 hex digest of a byte slice.
fn compute_sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    format!("{digest:x}")
}

/// Parse a `data:` URI into `(mime_type, raw_bytes)`.
///
/// Returns `None` for non-`data:` content or malformed payloads. Mirrors
/// the .ts `parseDataUri` — payload must be valid base64 (length multiple
/// of 4, charset match, decode succeeds).
fn parse_data_uri(content: &str) -> Option<(String, Vec<u8>)> {
    let caps = DATA_URI_RE.captures(content)?;
    let mime = caps
        .name("mime")
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let payload = caps.name("payload").map(|m| m.as_str()).unwrap_or("");

    if !is_valid_base64(payload) {
        return None;
    }

    let raw = base64_decode(payload)?;
    Some((mime, raw))
}

/// Validate a base64 payload (length, alphabet, decodability).
fn is_valid_base64(payload: &str) -> bool {
    if payload.is_empty() {
        return true;
    }
    if !payload.len().is_multiple_of(4) {
        return false;
    }
    if !BASE64_RE.is_match(payload) {
        return false;
    }
    base64_decode(payload).is_some()
}

/// Decode a standard-base64 string to bytes. `=` padding is tolerated
/// (skipped — the trailing bits are simply not emitted).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input.as_bytes() {
        // Skip padding — bits already accounted for by completed groups.
        if b == b'=' {
            continue;
        }
        let v: u32 = match b {
            b'A'..=b'Z' => u32::from(b - b'A'),
            b'a'..=b'z' => u32::from(b - b'a') + 26,
            b'0'..=b'9' => u32::from(b - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
            // Clear consumed bits to prevent buf overflow on long inputs
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn blob_ref(sha256: &str) -> String {
    format!("blob://sha256/{sha256}")
}

/// Format a byte count with thousands separators (en-US style).
fn format_bytes(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let first = bytes.len() % 3;
    if first > 0 {
        out.push_str(std::str::from_utf8(&bytes[..first]).expect("ascii"));
        if bytes.len() > first {
            out.push(',');
        }
    }
    let mut i = first;
    while i + 3 <= bytes.len() {
        if i > first {
            out.push(',');
        }
        out.push_str(std::str::from_utf8(&bytes[i..i + 3]).expect("ascii"));
        i += 3;
    }
    out
}

// ── Public API ──────────────────────────────────────────────────────────

/// SHA256 hex digest of `data`.
pub fn compute_sha256(data: &[u8]) -> String {
    compute_sha256_bytes(data)
}

/// True when `content` starts with the `data:` URI scheme.
pub fn is_data_uri(content: &str) -> bool {
    content.starts_with("data:")
}

/// Shannon entropy of `text`'s character distribution, in bits/char.
///
/// Counts characters (Unicode code points), not bytes, so non-ASCII input
/// is weighted correctly. Returns `0.0` for empty input.
pub fn shannon_entropy(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }

    let len = text.chars().count();
    let mut counts: HashMap<char, usize> = HashMap::new();
    for ch in text.chars() {
        *counts.entry(ch).or_insert(0) += 1;
    }

    let mut entropy = 0.0;
    let len_f = len as f64;
    for &count in counts.values() {
        let p = f64::from(count as u32) / len_f;
        entropy -= p * p.log2();
    }
    entropy
}

/// True when `content` is plausibly a base64-style encoded blob —
/// length above [`SIZE_BASE64_CHECK`], content composed entirely of
/// base64 alphabet chars (plus optional `=` padding), *and* entropy above
/// [`ENTROPY_THRESHOLD`].
///
/// The "all base64 chars" check is an assignment requirement (the .ts
/// source skips it); see the module-level doc comment.
pub fn looks_like_base64_blob(content: &str) -> bool {
    if content.len() <= SIZE_BASE64_CHECK {
        return false;
    }
    if !BASE64_RE.is_match(content) {
        return false;
    }
    shannon_entropy(content) > ENTROPY_THRESHOLD
}

/// Detect and strip data URIs and high-entropy base64-style blobs from
/// `content`. Returns the sanitized content and, if any extraction
/// happened, a populated [`BlobMetadata`].
///
/// Branch order (matching the .ts):
/// 1. `data:` URI → decode payload, hash, return placeholder + metadata.
/// 2. Length > [`SIZE_HARD_CAP`] → hash raw bytes, return placeholder.
/// 3. Length > [`SIZE_BASE64_CHECK`] and [`looks_like_base64_blob`] →
///    hash raw bytes, return placeholder + entropy reading.
/// 4. Otherwise, return `(content.to_string(), None)`.
pub fn sanitize_content(content: &str) -> (String, Option<BlobMetadata>) {
    if is_data_uri(content)
        && let Some((mime, raw_bytes)) = parse_data_uri(content)
    {
        let sha = compute_sha256_bytes(&raw_bytes);
        let blob_ref = blob_ref(&sha);
        return (
            format!(
                "[Binary content extracted: {}, {} bytes → {}]",
                mime,
                format_bytes(raw_bytes.len()),
                blob_ref,
            ),
            Some(BlobMetadata {
                blob_ref: Some(blob_ref),
                original_size: raw_bytes.len(),
                mime: Some(mime),
                extraction_reason: Some("data_uri".to_string()),
                entropy: None,
            }),
        );
    }

    let original_size = content.len();

    if original_size > SIZE_HARD_CAP {
        let sha = compute_sha256_bytes(content.as_bytes());
        let blob_ref = blob_ref(&sha);
        return (
            format!(
                "[Large content extracted: {} bytes → {}]",
                format_bytes(original_size),
                blob_ref,
            ),
            Some(BlobMetadata {
                blob_ref: Some(blob_ref),
                original_size,
                mime: None,
                extraction_reason: Some("size_cap".to_string()),
                entropy: None,
            }),
        );
    }

    if original_size > SIZE_BASE64_CHECK && looks_like_base64_blob(content) {
        let sha = compute_sha256_bytes(content.as_bytes());
        let entropy = (shannon_entropy(content) * 100.0).round() / 100.0;
        let blob_ref = blob_ref(&sha);
        return (
            format!(
                "[Encoded content extracted: {} bytes, entropy {:.1} bits/char → {}]",
                format_bytes(original_size),
                entropy,
                blob_ref,
            ),
            Some(BlobMetadata {
                blob_ref: Some(blob_ref),
                original_size,
                mime: None,
                extraction_reason: Some("high_entropy".to_string()),
                entropy: Some(entropy),
            }),
        );
    }

    (content.to_string(), None)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_sha256_matches_known_vectors() {
        // Known SHA256 of empty input.
        assert_eq!(
            compute_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // Known SHA256 of "abc".
        assert_eq!(
            compute_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn is_data_uri_detects_only_data_scheme() {
        assert!(is_data_uri("data:image/png;base64,iVBORw0KGgo="));
        assert!(is_data_uri("data:text/plain;base64,SGVsbG8="));
        assert!(!is_data_uri("Hello world"));
        assert!(!is_data_uri("just some text"));
        assert!(!is_data_uri("DATA:uppercase-not-matched"));
    }

    #[test]
    fn shannon_entropy_handles_extremes_and_non_ascii() {
        assert!((shannon_entropy("") - 0.0).abs() < f64::EPSILON);

        // Uniform across the alphabet — high entropy.
        let uniform: String = "abcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZ"
            .chars()
            .cycle()
            .take(64 * 100)
            .collect();
        assert!(shannon_entropy(&uniform) > 5.5);

        // Real English prose — bounded entropy.
        let prose =
            "hello world this is normal english text with common letters and patterns ".repeat(100);
        assert!(shannon_entropy(&prose) < 5.0);

        // All same char — near-zero entropy.
        let repeated = "a".repeat(10_000);
        assert!(shannon_entropy(&repeated) < 0.1);

        // Non-ASCII: counts must be by char, not byte. Two distinct
        // multi-byte chars over 4 chars → entropy = 1 bit/char.
        let two_chars = "ééüü".repeat(10);
        assert!(
            (shannon_entropy(&two_chars) - 1.0).abs() < 1e-9,
            "got {}",
            shannon_entropy(&two_chars),
        );
    }

    #[test]
    fn looks_like_base64_blob_requires_size_charset_and_entropy() {
        // Random-byte base64 (~200KB) — should flag.
        let raw: Vec<u8> = (0u32..150_000).map(|i| (i & 0xff) as u8).collect();
        let b64 = base64_encode(&raw);
        assert!(
            looks_like_base64_blob(&b64),
            "expected high-entropy base64 blob",
        );

        // Repetitive Python code — should NOT flag (low entropy).
        let code = "def foo():\n    return 42\n".repeat(20_000);
        assert!(!looks_like_base64_blob(&code));

        // Short content — always false regardless of entropy.
        let tiny = "abcdefghijklmnop";
        assert!(!looks_like_base64_blob(tiny));

        // Long but contains non-base64 chars (spaces, newlines) — rejected
        // by the charset check even if entropy happens to be high.
        let noisy = "ABC def GHI jkl MNO pqr STU vwx YZA".repeat(5_000);
        assert!(!looks_like_base64_blob(&noisy));
    }

    #[test]
    fn sanitize_content_passes_through_normal_text() {
        let content = "This is normal conversational text.";
        let (out, meta) = sanitize_content(content);
        assert_eq!(out, content);
        assert!(meta.is_none());

        let (out2, meta2) = sanitize_content("Small text, under all thresholds.");
        assert_eq!(out2, "Small text, under all thresholds.");
        assert!(meta2.is_none());
    }

    #[test]
    fn sanitize_content_extracts_data_uri() {
        // PNG magic header.
        let raw: Vec<u8> = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let b64 = base64_encode(&raw);
        let input = format!("data:image/png;base64,{b64}");

        let (out, meta) = sanitize_content(&input);

        assert!(out.contains("Binary content extracted"), "got: {out}");
        assert!(out.contains("blob://sha256/"), "got: {out}");

        let meta = meta.expect("expected blob metadata");
        assert_eq!(meta.extraction_reason.as_deref(), Some("data_uri"));
        assert_eq!(meta.mime.as_deref(), Some("image/png"));
        assert_eq!(meta.original_size, raw.len());

        let expected_sha = compute_sha256(&raw);
        assert_eq!(
            meta.blob_ref.as_deref(),
            Some(blob_ref(&expected_sha).as_str()),
        );
    }

    #[test]
    fn sanitize_content_extracts_oversized_content() {
        let big = "x".repeat(SIZE_HARD_CAP + 1);
        let (out, meta) = sanitize_content(&big);
        assert!(out.contains("Large content extracted"), "got: {out}");
        let meta = meta.expect("expected blob metadata");
        assert_eq!(meta.extraction_reason.as_deref(), Some("size_cap"));
        assert_eq!(meta.original_size, big.len());
    }

    #[test]
    fn sanitize_content_extracts_high_entropy_payload() {
        // Build >100KB of high-entropy bytes, encode as base64.
        let raw: Vec<u8> = (0u32..150_000).map(|i| ((i * 31) & 0xff) as u8).collect();
        let b64 = base64_encode(&raw);

        let (out, meta) = sanitize_content(&b64);
        assert!(out.contains("Encoded content extracted"), "got: {out}");
        let meta = meta.expect("expected blob metadata");
        assert_eq!(meta.extraction_reason.as_deref(), Some("high_entropy"));
        assert!(meta.entropy.unwrap_or(0.0) > 5.0);

        // Large prose should still pass through unchanged.
        let prose = "This is a normal paragraph of English text. It discusses various topics in a conversational tone. "
            .repeat(3_000);
        let (p_out, p_meta) = sanitize_content(&prose);
        assert_eq!(p_out, prose);
        assert!(p_meta.is_none());
    }

    #[test]
    fn parse_data_uri_decodes_and_rejects_invalid() {
        // Valid base64 data URI with explicit mime.
        let png: Vec<u8> = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let input = format!("data:image/png;base64,{}", base64_encode(&png));
        let (mime, raw) = parse_data_uri(&input).expect("should parse");
        assert_eq!(mime, "image/png");
        assert_eq!(raw, png);

        // Valid base64 with default mime.
        let (mime, raw) = parse_data_uri(&format!("data:;base64,{}", base64_encode(b"Hello")))
            .expect("should parse");
        assert_eq!(mime, "application/octet-stream");
        assert_eq!(raw, b"Hello");

        // Invalid base64 — reject.
        assert!(parse_data_uri("data:image/png;base64,!!!not-valid!!!").is_none());
        // Missing scheme — reject.
        assert!(parse_data_uri("just text").is_none());
    }

    #[test]
    fn format_bytes_inserts_thousands_separators() {
        assert_eq!(format_bytes(0), "0");
        assert_eq!(format_bytes(7), "7");
        assert_eq!(format_bytes(42), "42");
        assert_eq!(format_bytes(999), "999");
        assert_eq!(format_bytes(1_000), "1,000");
        assert_eq!(format_bytes(1_234), "1,234");
        assert_eq!(format_bytes(1_234_567), "1,234,567");
        assert_eq!(format_bytes(1_000_000), "1,000,000");
    }

    // ── Test helpers ─────────────────────────────────────────────────

    /// Standard-base64 encoder used by the data-URI / entropy tests.
    fn base64_encode(input: &[u8]) -> String {
        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        let mut i = 0;
        while i + 3 <= input.len() {
            let b = (u32::from(input[i]) << 16)
                | (u32::from(input[i + 1]) << 8)
                | u32::from(input[i + 2]);
            out.push(chars[((b >> 18) & 0x3f) as usize] as char);
            out.push(chars[((b >> 12) & 0x3f) as usize] as char);
            out.push(chars[((b >> 6) & 0x3f) as usize] as char);
            out.push(chars[(b & 0x3f) as usize] as char);
            i += 3;
        }
        let rem = input.len() - i;
        if rem == 1 {
            let b = u32::from(input[i]) << 16;
            out.push(chars[((b >> 18) & 0x3f) as usize] as char);
            out.push(chars[((b >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        } else if rem == 2 {
            let b = (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8);
            out.push(chars[((b >> 18) & 0x3f) as usize] as char);
            out.push(chars[((b >> 12) & 0x3f) as usize] as char);
            out.push(chars[((b >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        out
    }
}
