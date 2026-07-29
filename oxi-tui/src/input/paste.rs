//! Bracketed paste handler.
//!
//! Terminals that implement bracketed paste mode wrap pasted content between
//! the markers `\x1b[200~` (start) and `\x1b[201~` (end). These markers may
//! arrive split across multiple reads from the terminal: the start marker
//! might be the tail of one read, with the actual paste content arriving in
//! the next, and the end marker split similarly across reads at the end.
//!
//! [`PasteBuffer`] accumulates incoming bytes and emits [`PasteEvent`]s as
//! state transitions happen:
//!
//! - The start marker triggers [`PasteEvent::Started`].
//! - Each subsequent chunk of paste content triggers [`PasteEvent::Chunk`].
//! - The end marker triggers [`PasteEvent::Completed`] with the full pasted
//!   text (decoded as UTF-8 with replacement for invalid sequences, so a
//!   mid-paste disconnect does not lose data).
//! - Any input that does not begin a paste triggers [`PasteEvent::NotPaste`],
//!   leaving the buffer in its idle state so the caller can pass the bytes
//!   on to its normal key-parser.
//!
//! The buffer handles partial markers by keeping a small "lookahead" tail of
//! recent input bytes when idle (long enough to catch a marker that was split
//! across two reads) and a similar lookahead when active (to catch a split
//! end marker). The lookahead is bounded by the marker length minus one
//! (5 bytes), so memory is O(1) per buffer regardless of input rate.
//!
//! The byte cap (default 64 MiB) defends against a lost/corrupted end marker
//! (ssh dropouts, tmux truncation) consuming unbounded memory. When the cap
//! trips, the accumulated content is delivered as [`PasteEvent::Completed`]
//! with whatever bytes were buffered, so nothing is lost.
//!
//! Reference: <https://cirw.in/blog/bracketed-paste>

/// Default cap on buffered paste content. 64 MiB matches the value used by
/// the upstream omp handler and is generous enough that a real clipboard paste
/// never trips it; only lost-end-marker pathology hits the cap.
pub const DEFAULT_BYTE_LIMIT: usize = 64 * 1024 * 1024;

/// Start-of-paste marker.
const PASTE_START_MARKER: &[u8] = b"\x1b[200~";
/// End-of-paste marker.
const PASTE_END_MARKER: &[u8] = b"\x1b[201~";

/// Event emitted by [`PasteBuffer::feed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteEvent {
    /// No paste is in progress. The fed bytes are not (the tail of) a paste
    /// start marker; the caller should route them to its normal key/event
    /// pipeline.
    NotPaste,
    /// Paste mode just began on this call (the start marker was observed).
    /// The buffer is now accumulating content; subsequent [`PasteBuffer::feed`]
    /// calls will emit [`PasteEvent::Chunk`]s or [`PasteEvent::Completed`].
    Started,
    /// A chunk of paste content arrived. The string contains the bytes that
    /// arrived in this single feed call (UTF-8 decoded with replacement for
    /// invalid sequences). Multiple `Chunk`s build up the full paste before
    /// the [`PasteEvent::Completed`] is emitted.
    Chunk(String),
    /// A complete paste payload was assembled. The string contains all bytes
    /// between the start and end markers. After this event, the buffer resets
    /// to its idle state and the next [`PasteBuffer::feed`] will start fresh.
    Completed(String),
}

/// Bracketed-paste-mode state machine.
///
/// One instance per input pipeline. Feed it raw terminal bytes via
/// [`PasteBuffer::feed`] and react to the emitted [`PasteEvent`]s.
///
/// # Examples
///
/// ```
/// use oxi_tui::input::paste::{PasteBuffer, PasteEvent};
///
/// let mut buf = PasteBuffer::new(1024);
///
/// // Start marker alone — no content yet.
/// assert_eq!(buf.feed(b"\x1b[200~"), PasteEvent::Started);
///
/// // Content chunk.
/// assert_eq!(buf.feed(b"hello"), PasteEvent::Chunk("hello".to_string()));
///
/// // End marker alone (no trailing content) — completes the paste.
/// assert_eq!(
///     buf.feed(b"\x1b[201~"),
///     PasteEvent::Completed("hello".to_string())
/// );
///
/// // After completion the buffer is idle again.
/// assert_eq!(buf.feed(b"normal text"), PasteEvent::NotPaste);
/// ```
#[derive(Debug)]
pub struct PasteBuffer {
    /// Accumulated paste content (between start and end markers).
    buffer: String,
    /// Whether we are between the start and end markers.
    active: bool,
    /// Lookahead for partial-marker prefixes. Holds the trailing bytes of the
    /// previous feed that are a strict prefix of either the start marker
    /// (when idle) or the end marker (when active). Bounded to fewer than
    /// `marker.len()` bytes so memory is O(1).
    lookahead: Vec<u8>,
    /// Hard cap on buffered bytes.
    byte_limit: usize,
}

impl Default for PasteBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_BYTE_LIMIT)
    }
}

impl PasteBuffer {
    /// Create a new paste buffer with the given byte cap.
    ///
    /// `byte_limit` is the maximum number of bytes that may accumulate
    /// between the start and end markers. Past that, the buffer force-
    /// completes the paste to defend against lost end markers.
    #[must_use]
    pub fn new(byte_limit: usize) -> Self {
        Self {
            buffer: String::new(),
            active: false,
            lookahead: Vec::new(),
            byte_limit,
        }
    }

    /// Returns `true` if a paste is currently being accumulated (start marker
    /// observed, end marker not yet).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns the number of bytes currently buffered for the in-progress paste.
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// Feed a chunk of terminal bytes into the buffer and report what happened.
    ///
    /// `data` may be any contiguous slice of bytes from the terminal — an entire
    /// read, a partial read, or a sequence of bytes concatenated from a writev.
    /// All three cases work the same way: the buffer scans for the start marker,
    /// accumulates content, and scans for the end marker.
    pub fn feed(&mut self, data: &[u8]) -> PasteEvent {
        if self.active {
            self.feed_active_tail(data, /* started_now = */ false)
        } else {
            self.feed_idle(data)
        }
    }

    /// Feed bytes while no paste is active: look for the start marker,
    /// honoring a partial prefix at the tail of the previous feed.
    fn feed_idle(&mut self, data: &[u8]) -> PasteEvent {
        // Prepend any leftover lookahead from the previous feed. The combined
        // window is bounded by `data.len() + marker.len() - 1`.
        let mut combined: Vec<u8> = Vec::with_capacity(data.len());
        let prev = std::mem::take(&mut self.lookahead);
        combined.extend_from_slice(&prev);
        combined.extend_from_slice(data);

        // Look for the start marker anywhere in the combined input.
        if let Some(start_idx) = find_subsequence(&combined, PASTE_START_MARKER) {
            self.active = true;
            self.buffer.clear();
            let after_start = start_idx + PASTE_START_MARKER.len();
            let tail = &combined[after_start..];
            // Process the tail as if we just entered active state. This
            // correctly handles the case where start + end (and possibly
            // content) all arrive in the same feed.
            return self.feed_active_tail(tail, /* started_now = */ true);
        }

        // No start marker in combined input. Hold back any trailing
        // partial start-marker prefix as lookahead for the next feed.
        self.lookahead = partial_prefix(&combined, PASTE_START_MARKER);
        PasteEvent::NotPaste
    }

    /// Internal active-paste feed: handles both the "already-active" case and
    /// the "just transitioned to active" case (where the tail of the start
    /// marker may contain end-marker bytes or content).
    /// When `started_now` is true, the function emits [`PasteEvent::Started`]
    /// as the return value ONLY if the paste is still in progress at the end
    /// of the call. If the paste completes (end marker found), the Completed
    /// event takes precedence.
    fn feed_active_tail(&mut self, data: &[u8], started_now: bool) -> PasteEvent {
        // The previous feed may have left a partial-marker prefix in the
        // lookahead; combine it with the new data to find the end marker.
        let prev = std::mem::take(&mut self.lookahead);
        let mut combined: Vec<u8> = Vec::with_capacity(data.len() + prev.len());
        combined.extend_from_slice(&prev);
        combined.extend_from_slice(data);

        // Search for the end marker in the combined buffer.
        if let Some(end_idx) = find_subsequence(&combined, PASTE_END_MARKER) {
            // Append everything before the end marker (this includes the
            // prior lookahead bytes if they were partial-marker prefix that
            // turned out to be part of the content stream).
            let content = &combined[..end_idx];
            self.append_bytes(content);

            // Capture and reset state.
            let payload = std::mem::take(&mut self.buffer);
            self.active = false;
            self.lookahead.clear();

            return PasteEvent::Completed(payload);
        }

        // No end marker yet — split into "content" and "lookahead suffix".
        // Both the previous-feed lookahead bytes AND any new trailing partial
        // prefix count as content; only the trailing partial prefix becomes
        // the new lookahead.
        let new_lookahead = partial_prefix(&combined, PASTE_END_MARKER);
        let content_end = combined.len() - new_lookahead.len();
        let content = &combined[..content_end];
        self.append_bytes(content);

        if self.buffer.len() > self.byte_limit {
            let payload = std::mem::take(&mut self.buffer);
            self.active = false;
            self.lookahead.clear();
            return PasteEvent::Completed(payload);
        }

        self.lookahead = new_lookahead;
        if started_now {
            PasteEvent::Started
        } else {
            // Report just the new bytes as the chunk, not the entire combined
            // buffer — that keeps "Chunk" semantically aligned with "what just
            // arrived" and lets callers incrementally render paste content.
            PasteEvent::Chunk(String::from_utf8_lossy(data).into_owned())
        }
    }

    /// Append a byte slice to the buffer, lossy-decoding as UTF-8.
    fn append_bytes(&mut self, bytes: &[u8]) {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
    }
}
/// Find the longest proper prefix of `haystack` that is also a prefix of
/// `needle` (and is strictly shorter than `needle`). Returns those bytes.
///
/// Used to retain lookahead: if a feed ends with the first 3 bytes of a
/// 6-byte marker, we hold those 3 bytes until the next feed to test whether
/// the marker is split.
///
/// Returns an empty `Vec` if `haystack` ends with bytes that don't match any
/// prefix of `needle`. Always returns fewer than `needle.len()` bytes (we
/// only retain a partial prefix, never a complete match — a complete match
/// would have been consumed by the caller).
fn partial_prefix(haystack: &[u8], needle: &[u8]) -> Vec<u8> {
    let max = needle.len().saturating_sub(1);
    let n = haystack.len().min(max);
    // Try lengths from largest (n) down to 1.
    for len in (1..=n).rev() {
        let start = haystack.len() - len;
        if haystack[start..].starts_with(&needle[..len]) {
            return haystack[start..].to_vec();
        }
    }
    Vec::new()
}

/// Find the first occurrence of `needle` in `haystack`, returning its byte offset.
///
/// Returns `None` if `needle` is empty (caller responsibility) or absent.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_marker_only_returns_started() {
        let mut buf = PasteBuffer::new(1024);
        assert_eq!(buf.feed(b"\x1b[200~"), PasteEvent::Started);
        assert!(buf.is_active());
    }

    #[test]
    fn chunks_emit_chunk_events() {
        let mut buf = PasteBuffer::new(1024);
        assert_eq!(buf.feed(b"\x1b[200~"), PasteEvent::Started);
        assert_eq!(buf.feed(b"hello "), PasteEvent::Chunk("hello ".into()));
        assert_eq!(buf.feed(b"world"), PasteEvent::Chunk("world".into()));
        assert_eq!(
            buf.feed(b"\x1b[201~"),
            PasteEvent::Completed("hello world".into())
        );
        assert!(!buf.is_active());
    }

    #[test]
    fn start_and_content_in_one_call_emits_started() {
        // The contract is that `feed` reports the transition (`Started`),
        // and the content gets buffered for subsequent `Chunk` events.
        let mut buf = PasteBuffer::new(1024);
        assert_eq!(buf.feed(b"\x1b[200~abc"), PasteEvent::Started);
        assert!(buf.is_active());
        assert_eq!(buf.buffered_len(), 3);
        assert_eq!(buf.feed(b"\x1b[201~"), PasteEvent::Completed("abc".into()));
    }

    #[test]
    fn start_and_end_in_one_call_completes_immediately() {
        // Both arrive in the same feed call; the buffer completes the paste
        // and emits Completed (no content between the markers).
        let mut buf = PasteBuffer::new(1024);
        let event = buf.feed(b"\x1b[200~\x1b[201~");
        assert_eq!(event, PasteEvent::Completed(String::new()));
        assert!(!buf.is_active());
    }

    #[test]
    fn non_paste_input_returns_not_paste() {
        let mut buf = PasteBuffer::new(1024);
        assert_eq!(buf.feed(b"hello world"), PasteEvent::NotPaste);
        assert_eq!(buf.feed(b"\x1b[A"), PasteEvent::NotPaste);
        assert!(!buf.is_active());
    }

    #[test]
    fn partial_start_marker_buffered() {
        // The start marker is split: half arrived in one feed, the rest in
        // the next. The buffer must NOT emit NotPaste on the first half —
        // it must keep the partial prefix as lookahead and recover on the
        // next feed.
        let mut buf = PasteBuffer::new(1024);
        let first_half = &b"\x1b[20"[..];
        assert_eq!(buf.feed(first_half), PasteEvent::NotPaste);
        assert!(!buf.is_active());
        // Second half completes the marker.
        let second_half = &b"0~payload\x1b[201~"[..];
        assert_eq!(
            buf.feed(second_half),
            PasteEvent::Completed("payload".into())
        );
        assert!(!buf.is_active());
    }

    #[test]
    fn partial_end_marker_buffered() {
        // The end marker is split across two feeds.
        let mut buf = PasteBuffer::new(1024);
        assert_eq!(buf.feed(b"\x1b[200~hello"), PasteEvent::Started);
        assert!(buf.is_active());
        // First half of end marker.
        assert_eq!(buf.feed(b"\x1b[20"), PasteEvent::Chunk("\x1b[20".into()));
        assert!(buf.is_active());
        // Second half completes the paste.
        assert_eq!(
            buf.feed(b"1~trailing"),
            PasteEvent::Completed("hello".into())
        );
    }

    #[test]
    fn byte_limit_force_completes() {
        let mut buf = PasteBuffer::new(16);
        assert_eq!(buf.feed(b"\x1b[200~"), PasteEvent::Started);
        let first = b"abcdefghijkl";
        assert_eq!(
            buf.feed(first),
            PasteEvent::Chunk(String::from_utf8_lossy(first).into_owned())
        );
        let second = b"mnopqrstuvwxyz0123";
        let event = buf.feed(second);
        assert!(matches!(event, PasteEvent::Completed(_)));
        assert!(!buf.is_active());
    }

    #[test]
    fn default_uses_64_mib_cap() {
        let buf = PasteBuffer::default();
        assert_eq!(buf.byte_limit, DEFAULT_BYTE_LIMIT);
    }

    #[test]
    fn partial_prefix_helper() {
        // Tail of haystack matches a prefix of the marker.
        assert_eq!(
            partial_prefix(b"\x1b[20", PASTE_START_MARKER),
            b"\x1b[20".to_vec()
        );
        // No prefix overlap with start marker → empty.
        let empty: Vec<u8> = Vec::new();
        assert_eq!(partial_prefix(b"0~", PASTE_START_MARKER), empty);
        assert_eq!(partial_prefix(b"~", PASTE_START_MARKER), empty);
        assert_eq!(partial_prefix(b"hello", PASTE_START_MARKER), empty);
        // A complete marker is NOT retained as lookahead (would be infinite-loop bait).
        let empty: Vec<u8> = Vec::new();
        assert_eq!(partial_prefix(b"\x1b[200~", PASTE_START_MARKER), empty);
    }
}
