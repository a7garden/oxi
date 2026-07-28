//! Shared SSE byte-stream framing utilities.
//!
//! Centralizes the low-level "raw HTTP byte chunk → complete SSE lines"
//! decoding that every SSE-speaking provider needs: partial-UTF-8 handling
//! at chunk boundaries and line splitting. This is oxi's analogue of omp's
//! `readSseEvents()` (omp keeps it in `pi-utils`).
//!
//! Provider-specific SSE *interpretation* (turning `data:` payloads into
//! `ProviderEvent`s) stays per-provider — only the framing layer is shared.

/// Extract the longest valid UTF-8 prefix from a byte slice.
///
/// Returns the valid string and the trailing bytes that form an incomplete
/// UTF-8 sequence. These trailing bytes should be prepended to the next chunk
/// to ensure no characters are lost at HTTP chunk boundaries.
pub fn find_valid_utf8_prefix(bytes: &[u8]) -> (String, Vec<u8>) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Vec::new()),
        Err(e) => {
            let valid = &bytes[..e.valid_up_to()];
            let trailing = bytes[e.valid_up_to()..].to_vec();
            (String::from_utf8_lossy(valid).to_string(), trailing)
        }
    }
}

/// Split bytes into complete lines (ending with `\n`) and trailing incomplete
/// data. This ensures per-provider SSE parsers only receive complete `data:`
/// lines, preventing JSON parse failures from lines split across HTTP chunks.
///
/// Callers feed each `bytes_stream()` chunk through this, prepending the
/// returned `trailing` bytes to the next chunk.
pub fn split_complete_lines(bytes: &[u8]) -> (String, Vec<u8>) {
    // Find the last newline — everything up to and including it is complete.
    match bytes.iter().rposition(|&b| b == b'\n') {
        Some(last_nl) => {
            let split_at = last_nl + 1;
            let complete = match std::str::from_utf8(&bytes[..split_at]) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    let (s, _) = find_valid_utf8_prefix(&bytes[..split_at]);
                    s
                }
            };
            let trailing = bytes[split_at..].to_vec();
            (complete, trailing)
        }
        None => {
            // No newline at all — the entire buffer is incomplete.
            // Check if it's valid UTF-8; if not, save as pending.
            (String::new(), bytes.to_vec())
        }
    }
}
