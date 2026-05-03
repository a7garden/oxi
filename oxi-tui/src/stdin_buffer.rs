//! Stdin buffer for efficient async stdin handling.
//!
//! Buffers input bytes and emits complete escape sequences. This is necessary
//! because stdin data can arrive in partial chunks, especially for escape
//! sequences like mouse events. Without buffering, partial sequences get
//! misinterpreted as regular keypresses.
//!
//! For example, the mouse SGR sequence `\x1b[<35;20;5m` might arrive as:
//! - Chunk 1: `\x1b`
//! - Chunk 2: `[<35`
//! - Chunk 3: `;20;5m`
//!
//! The buffer accumulates these until a complete sequence is detected.
//!
//! Based on code from OpenTUI (https://github.com/anomalyco/opentui)
//! MIT License - Copyright (c) 2025 opentui

use std::io::Read;
use std::time::Duration;

use anyhow::Result;

const ESC: u8 = 0x1b;
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

// ---------------------------------------------------------------------------
// Sequence completeness detection
// ---------------------------------------------------------------------------

/// Result of checking whether a byte buffer holds a complete escape sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceStatus {
    /// The sequence is complete and ready to emit.
    Complete,
    /// More bytes are needed to determine the sequence.
    Incomplete,
    /// The buffer does not start with ESC (plain text).
    NotEscape,
}

/// Check whether `data` (starting with ESC) is a complete escape sequence.
fn is_complete_sequence(data: &[u8]) -> SequenceStatus {
    if data.is_empty() || data[0] != ESC {
        return SequenceStatus::NotEscape;
    }
    if data.len() == 1 {
        return SequenceStatus::Incomplete;
    }

    let after_esc = &data[1..];

    // CSI sequences: ESC [
    if after_esc.starts_with(b"[") {
        // Old-style mouse: ESC [ M + 3 bytes → 6 bytes total
        if after_esc.starts_with(b"[M") {
            return if data.len() >= 6 {
                SequenceStatus::Complete
            } else {
                SequenceStatus::Incomplete
            };
        }
        return is_complete_csi_sequence(data);
    }

    // OSC sequences: ESC ]
    if after_esc.starts_with(b"]") {
        return is_complete_osc_sequence(data);
    }

    // DCS sequences: ESC P
    if after_esc.starts_with(b"P") {
        return is_complete_string_sequence(data, b"P");
    }

    // APC sequences: ESC _
    if after_esc.starts_with(b"_") {
        return is_complete_string_sequence(data, b"_");
    }

    // SS3 sequences: ESC O
    if after_esc.starts_with(b"O") {
        return if after_esc.len() >= 2 {
            SequenceStatus::Complete
        } else {
            SequenceStatus::Incomplete
        };
    }

    // Meta key: ESC + single char
    if after_esc.len() == 1 {
        return SequenceStatus::Complete;
    }

    // Unknown – treat as complete
    SequenceStatus::Complete
}

/// CSI sequences end with a final byte in 0x40..=0x7E.
fn is_complete_csi_sequence(data: &[u8]) -> SequenceStatus {
    if data.len() < 3 {
        return SequenceStatus::Incomplete;
    }

    // The payload after ESC [
    let payload = &data[2..];
    let last = payload[payload.len() - 1];

    // CSI final byte range
    if !(0x40..=0x7e).contains(&last) {
        return SequenceStatus::Incomplete;
    }

    // SGR mouse: ESC[<B;X;Ym or ESC[<B;X;YM
    if payload.starts_with(b"<") {
        if is_valid_sgr_mouse(payload) {
            return SequenceStatus::Complete;
        }
        // Last byte in range but not a valid mouse pattern yet
        return SequenceStatus::Incomplete;
    }

    SequenceStatus::Complete
}

/// Validate SGR mouse payload: `<digits;digits;digits[Mm]`
fn is_valid_sgr_mouse(payload: &[u8]) -> bool {
    if payload.len() < 4 || payload[0] != b'<' {
        return false;
    }
    // Must end with M or m
    let last = payload[payload.len() - 1];
    if last != b'M' && last != b'm' {
        return false;
    }
    let inner = &payload[1..payload.len() - 1];
    let parts: Vec<&[u8]> = inner.split(|&b| b == b';').collect();
    if parts.len() != 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.iter().all(|&b| b.is_ascii_digit()))
}

/// OSC sequences end with ST (ESC \\) or BEL (0x07).
fn is_complete_osc_sequence(data: &[u8]) -> SequenceStatus {
    if data.len() >= 2 && data[data.len() - 1] == 0x07 {
        return SequenceStatus::Complete;
    }
    // ST is ESC \ (two bytes)
    let len = data.len();
    if len >= 2 && data[len - 2] == ESC && data[len - 1] == b'\\' {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

/// DCS / APC sequences end with ST (ESC \\).
fn is_complete_string_sequence(data: &[u8], _kind: &[u8]) -> SequenceStatus {
    let len = data.len();
    if len >= 2 && data[len - 2] == ESC && data[len - 1] == b'\\' {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

// ---------------------------------------------------------------------------
// Extracting complete sequences from a buffer
// ---------------------------------------------------------------------------

/// Split `buf` into a list of complete sequences and an incomplete remainder.
fn extract_complete_sequences(buf: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut sequences: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0;

    while pos < buf.len() {
        if buf[pos] == ESC {
            // Try to find the end of this escape sequence
            let mut found = false;
            for end in (pos + 1)..=buf.len() {
                let candidate = &buf[pos..end];
                match is_complete_sequence(candidate) {
                    SequenceStatus::Complete => {
                        sequences.push(candidate.to_vec());
                        pos = end;
                        found = true;
                        break;
                    }
                    SequenceStatus::Incomplete => continue,
                    SequenceStatus::NotEscape => {
                        // Should not happen – first byte is ESC
                        sequences.push(candidate.to_vec());
                        pos = end;
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                // Ran out of data – remainder is incomplete
                return (sequences, buf[pos..].to_vec());
            }
        } else {
            // Plain character – emit as single-byte sequence
            sequences.push(vec![buf[pos]]);
            pos += 1;
        }
    }

    (sequences, Vec::new())
}

// ---------------------------------------------------------------------------
// Kitty printable codepoint deduplication
// ---------------------------------------------------------------------------

/// Try to parse a Kitty "unmodified printable" sequence: `ESC [ <cp> u` or
/// `ESC [ <cp>:<modifiers> u`.
/// Returns the codepoint if the sequence matches and codepoint >= 32.
fn parse_kitty_printable_codepoint(seq: &[u8]) -> Option<u32> {
    if seq.len() < 4 {
        return None;
    }
    if seq[0] != ESC || seq[1] != b'[' {
        return None;
    }
    // Must end with 'u'
    if *seq.last()? != b'u' {
        return None;
    }
    let inner = &seq[2..seq.len() - 1];
    // Codepoint digits before first ':' or ';'
    let cp_end = inner
        .iter()
        .position(|&b| b == b':' || b == b';')
        .unwrap_or(inner.len());
    if cp_end == 0 {
        return None;
    }
    let cp_bytes = &inner[..cp_end];
    let cp_str = std::str::from_utf8(cp_bytes).ok()?;
    let codepoint: u32 = cp_str.parse().ok()?;
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Buffering mode
// ---------------------------------------------------------------------------

/// Controls how the `StdinBuffer` groups incoming bytes before emitting
/// them via [`StdinBufferEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferingMode {
    /// Emit every individual character / escape sequence as it completes.
    /// Best for interactive editing where low latency matters.
    Character,
    /// Accumulate data until a newline (`\n`) is seen, then emit the full
    /// line. Useful for command-line style input.
    Line,
}

// ---------------------------------------------------------------------------
// Output events
// ---------------------------------------------------------------------------

/// Events emitted by [`StdinBuffer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinBufferEvent {
    /// A complete input sequence (key press, escape sequence, etc.).
    Data(Vec<u8>),
    /// Bracketed paste content.
    Paste(Vec<u8>),
}

// ---------------------------------------------------------------------------
// StdinBuffer
// ---------------------------------------------------------------------------

/// Configuration options for [`StdinBuffer`].
#[derive(Debug, Clone)]
pub struct StdinBufferOptions {
    /// Maximum time to wait for an incomplete escape sequence before flushing
    /// it anyway. Defaults to 10 ms.
    pub timeout: Duration,
    /// Initial buffering mode. Defaults to [`BufferingMode::Character`].
    pub mode: BufferingMode,
    /// Internal read buffer capacity in bytes. Defaults to 4096.
    pub read_capacity: usize,
}

impl Default for StdinBufferOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(10),
            mode: BufferingMode::Character,
            read_capacity: 4096,
        }
    }
}

/// Buffered async stdin reader that coalesces partial escape sequences.
///
/// # Overview
///
/// The `StdinBuffer` wraps any [`Read`] source (e.g. `std::io::stdin()`) and
/// provides:
///
/// * **Escape sequence completion** – partial chunks are accumulated until a
///   full CSI / OSC / DCS / APC / SS3 sequence is recognised.
/// * **Bracketed paste** – `\x1b[200~ … \x1b[201~` pairs are detected and
///   emitted as a single [`StdinBufferEvent::Paste`].
/// * **Drain optimisation** – call [`StdinBuffer::drain`] in a tight loop
///   until the OS read buffer is empty. Especially helpful over SSH where the
///   terminal may drain slowly.
/// * **Kitty deduplication** – when a terminal sends both the legacy key
///   event and a Kitty "unmodified printable" sequence, the duplicate is
///   suppressed.
///
/// # Usage
///
/// ```ignore
/// use std::io::stdin;
/// use oxi_tui::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinBufferEvent};
///
/// let mut buf = StdinBuffer::new(stdin(), StdinBufferOptions::default());
///
/// while let Some(event) = buf.next_event().unwrap() {
///     match event {
///         StdinBufferEvent::Data(seq) => { /* handle key / sequence */ }
///         StdinBufferEvent::Paste(text) => { /* handle paste */ }
///     }
/// }
/// ```
pub struct StdinBuffer<R: Read> {
    /// Underlying reader (typically stdin).
    reader: R,
    /// Internal read buffer.
    read_buf: Vec<u8>,
    /// Accumulator for incomplete escape sequences.
    seq_buf: Vec<u8>,
    /// Options.
    opts: StdinBufferOptions,
    /// Bracketed-paste state.
    paste_mode: bool,
    paste_buf: Vec<u8>,
    /// Kitty deduplication: pending printable codepoint to suppress.
    pending_kitty_cp: Option<u32>,
    /// Line-mode accumulator.
    line_buf: Vec<u8>,
}

impl<R: Read> StdinBuffer<R> {
    /// Create a new `StdinBuffer` wrapping `reader` with the given options.
    pub fn new(reader: R, opts: StdinBufferOptions) -> Self {
        Self {
            reader,
            read_buf: vec![0u8; opts.read_capacity],
            seq_buf: Vec::with_capacity(256),
            opts,
            paste_mode: false,
            paste_buf: Vec::new(),
            pending_kitty_cp: None,
            line_buf: Vec::new(),
        }
    }

    /// Create with sensible defaults.
    pub fn with_reader(reader: R) -> Self {
        Self::new(reader, StdinBufferOptions::default())
    }

    // -- Accessors ---------------------------------------------------------

    /// Current buffering mode.
    pub fn mode(&self) -> BufferingMode {
        self.opts.mode
    }

    /// Switch buffering mode at runtime.
    pub fn set_mode(&mut self, mode: BufferingMode) {
        self.opts.mode = mode;
    }

    /// Number of bytes currently in the incomplete-sequence accumulator.
    pub fn buffered_len(&self) -> usize {
        self.seq_buf.len()
    }

    /// Whether we are currently inside a bracketed-paste.
    pub fn is_pasting(&self) -> bool {
        self.paste_mode
    }

    // -- Reading -----------------------------------------------------------

    /// Perform a single non-blocking read from the underlying reader and
    /// process the bytes. Returns the list of events produced.
    ///
    /// If no bytes are available the result is an empty `Vec`.
    pub fn poll(&mut self) -> Result<Vec<StdinBufferEvent>> {
        let mut events = Vec::new();

        // Non-blocking read – use the reader directly. The caller should
        // set the source to non-blocking or use `poll` inside an async
        // event loop that has signalled stdin readability.
        match self.reader.read(&mut self.read_buf) {
            Ok(0) | Err(_) => {
                // EOF or error: flush whatever we have.
                events.append(&mut self.flush_inner());
            }
            Ok(n) => {
                let data = self.read_buf[..n].to_vec();
                self.process_bytes(&data, &mut events);
            }
        }

        Ok(events)
    }

    /// Drain the OS read buffer by reading repeatedly until no more bytes
    /// are immediately available. Returns all events produced.
    ///
    /// This is the primary optimisation for SSH connections where the
    /// terminal may deliver data slowly: rather than waking up for every
    /// tiny chunk, we pull everything that's available in one go.
    pub fn drain(&mut self) -> Result<Vec<StdinBufferEvent>> {
        let mut all_events = Vec::new();

        loop {
            match self.reader.read(&mut self.read_buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = self.read_buf[..n].to_vec();
                    self.process_bytes(&data, &mut all_events);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }

        // Flush any trailing incomplete sequence
        all_events.append(&mut self.flush_inner());
        Ok(all_events)
    }

    /// Force-read up to `max_bytes` from the reader and process them.
    /// Useful to prime the buffer or flush the OS side.
    pub fn read_into_buffer(&mut self, max_bytes: usize) -> Result<Vec<StdinBufferEvent>> {
        let mut tmp = vec![0u8; max_bytes];
        let mut events = Vec::new();
        match self.reader.read(&mut tmp) {
            Ok(0) | Err(_) => {}
            Ok(n) => {
                self.process_bytes(&tmp[..n], &mut events);
            }
        }
        Ok(events)
    }

    // -- Processing --------------------------------------------------------

    /// Core processing: feed raw bytes, detect bracketed paste boundaries,
    /// extract complete sequences, and push events.
    fn process_bytes(&mut self, data: &[u8], events: &mut Vec<StdinBufferEvent>) {
        if data.is_empty() && self.seq_buf.is_empty() {
            return;
        }

        // Append to accumulator
        self.seq_buf.extend_from_slice(data);

        // -- Bracketed paste handling --
        if self.paste_mode {
            self.paste_buf.extend_from_slice(&self.seq_buf);
            self.seq_buf.clear();

            if let Some(idx) = find_subsequence(&self.paste_buf, BRACKETED_PASTE_END) {
                let pasted = self.paste_buf[..idx].to_vec();
                let remaining = self.paste_buf[idx + BRACKETED_PASTE_END.len()..].to_vec();
                self.paste_mode = false;
                self.paste_buf.clear();
                self.pending_kitty_cp = None;
                events.push(StdinBufferEvent::Paste(pasted));
                if !remaining.is_empty() {
                    self.process_bytes(&remaining, events);
                }
            }
            return;
        }

        // Check for bracketed paste start
        if let Some(idx) = find_subsequence(&self.seq_buf, BRACKETED_PASTE_START) {
            // Emit any complete sequences before the paste marker
            if idx > 0 {
                let (seqs, _rem) = extract_complete_sequences(&self.seq_buf[..idx]);
                for seq in seqs {
                    self.emit_data_sequence(&seq, events);
                }
            }

            self.pending_kitty_cp = None;
            let after_start = idx + BRACKETED_PASTE_START.len();
            let remaining = self.seq_buf[after_start..].to_vec();
            self.seq_buf.clear();
            self.paste_mode = true;

            if !remaining.is_empty() {
                self.paste_buf = remaining;
                if let Some(end_idx) = find_subsequence(&self.paste_buf, BRACKETED_PASTE_END) {
                    let pasted = self.paste_buf[..end_idx].to_vec();
                    let after = self.paste_buf[end_idx + BRACKETED_PASTE_END.len()..].to_vec();
                    self.paste_mode = false;
                    self.paste_buf.clear();
                    self.pending_kitty_cp = None;
                    events.push(StdinBufferEvent::Paste(pasted));
                    if !after.is_empty() {
                        self.process_bytes(&after, events);
                    }
                }
            }
            return;
        }

        // Extract complete sequences
        let (seqs, remainder) = extract_complete_sequences(&self.seq_buf);
        self.seq_buf = remainder;

        for seq in seqs {
            self.emit_data_sequence(&seq, events);
        }

        // If there is still data in seq_buf it's an incomplete escape
        // sequence. In a real async event loop the caller would set a
        // timeout and call flush(). For simplicity we leave it buffered.
    }

    /// Emit a single data sequence, applying Kitty deduplication and
    /// line-mode buffering.
    fn emit_data_sequence(&mut self, seq: &[u8], events: &mut Vec<StdinBufferEvent>) {
        // Kitty deduplication: suppress the raw codepoint event when it
        // matches the pending Kitty printable sequence.
        if seq.len() == 1 {
            if Some(seq[0] as u32) == self.pending_kitty_cp {
                self.pending_kitty_cp = None;
                return;
            }
        }

        // Check if this sequence itself is a Kitty printable sequence
        self.pending_kitty_cp = parse_kitty_printable_codepoint(seq);

        // Buffering mode dispatch
        match self.opts.mode {
            BufferingMode::Character => {
                events.push(StdinBufferEvent::Data(seq.to_vec()));
            }
            BufferingMode::Line => {
                // Check if the sequence contains a newline
                if seq.contains(&b'\n') {
                    // Flush whatever we have accumulated plus this sequence
                    let mut combined = std::mem::take(&mut self.line_buf);
                    combined.extend_from_slice(seq);
                    events.push(StdinBufferEvent::Data(combined));
                } else {
                    self.line_buf.extend_from_slice(seq);
                }
            }
        }
    }

    // -- Flush / clear -----------------------------------------------------

    /// Flush any incomplete buffered data as-is (forced completion).
    /// Returns events produced.
    pub fn flush(&mut self) -> Vec<StdinBufferEvent> {
        self.flush_inner()
    }

    fn flush_inner(&mut self) -> Vec<StdinBufferEvent> {
        let mut events = Vec::new();

        // Flush paste buffer
        if self.paste_mode && !self.paste_buf.is_empty() {
            let pasted = std::mem::take(&mut self.paste_buf);
            self.paste_mode = false;
            self.pending_kitty_cp = None;
            events.push(StdinBufferEvent::Paste(pasted));
        }

        // Flush incomplete escape sequence
        if !self.seq_buf.is_empty() {
            let data = std::mem::take(&mut self.seq_buf);
            self.emit_data_sequence(&data, &mut events);
        }

        // Flush line buffer
        if !self.line_buf.is_empty() {
            let line = std::mem::take(&mut self.line_buf);
            events.push(StdinBufferEvent::Data(line));
        }

        self.pending_kitty_cp = None;
        events
    }

    /// Discard all buffered state without emitting events.
    pub fn clear(&mut self) {
        self.seq_buf.clear();
        self.paste_mode = false;
        self.paste_buf.clear();
        self.line_buf.clear();
        self.pending_kitty_cp = None;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the first occurrence of `needle` in `haystack`.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Helper: create a buffer fed from a static byte slice.
    fn buffer_from(data: &[u8], mode: BufferingMode) -> StdinBuffer<Cursor<Vec<u8>>> {
        let cursor = Cursor::new(data.to_vec());
        let mut opts = StdinBufferOptions::default();
        opts.mode = mode;
        StdinBuffer::new(cursor, opts)
    }

    // -- Test 1: plain characters are emitted immediately ------------------

    #[test]
    fn test_plain_characters_emitted_immediately() {
        let mut buf = buffer_from(b"abc", BufferingMode::Character);
        let events = buf.drain().unwrap();

        let data_events: Vec<&Vec<u8>> = events
            .iter()
            .filter_map(|e| match e {
                StdinBufferEvent::Data(d) => Some(d),
                _ => None,
            })
            .collect();

        assert_eq!(data_events.len(), 3);
        assert_eq!(data_events[0], &vec![b'a']);
        assert_eq!(data_events[1], &vec![b'b']);
        assert_eq!(data_events[2], &vec![b'c']);
    }

    // -- Test 2: complete CSI escape sequence is emitted as one piece ------

    #[test]
    fn test_complete_csi_sequence() {
        // CSI A = arrow up
        let seq = b"\x1b[A";
        let mut buf = buffer_from(seq, BufferingMode::Character);
        let events = buf.drain().unwrap();

        let data_events: Vec<&Vec<u8>> = events
            .iter()
            .filter_map(|e| match e {
                StdinBufferEvent::Data(d) => Some(d),
                _ => None,
            })
            .collect();

        assert_eq!(data_events.len(), 1);
        assert_eq!(data_events[0], &vec![0x1b, b'[', b'A']);
    }

    // -- Test 3: partial escape sequences are reassembled across reads -----

    #[test]
    fn test_partial_escape_sequence_across_reads() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        // Feed ESC, then "[A" separately
        let mut events = Vec::new();
        buf.process_bytes(b"\x1b", &mut events);
        assert!(events.is_empty(), "should wait for more data");

        buf.process_bytes(b"[A", &mut events);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StdinBufferEvent::Data(d) => assert_eq!(d, &vec![0x1b, b'[', b'A']),
            _ => panic!("expected Data event"),
        }
    }

    // -- Test 4: bracketed paste detection ---------------------------------

    #[test]
    fn test_bracketed_paste() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        let input = b"\x1b[200~hello world\x1b[201~";
        buf.process_bytes(input, &mut events);

        // Should get a single Paste event
        assert_eq!(events.len(), 1);
        match &events[0] {
            StdinBufferEvent::Paste(content) => {
                assert_eq!(content, b"hello world");
            }
            _ => panic!("expected Paste event"),
        }
    }

    // -- Test 5: line buffering mode ---------------------------------------

    #[test]
    fn test_line_buffering_mode() {
        let cursor = Cursor::new(b"".to_vec());
        let mut opts = StdinBufferOptions::default();
        opts.mode = BufferingMode::Line;
        let mut buf = StdinBuffer::new(cursor, opts);

        let mut events = Vec::new();

        // Feed characters without newline
        buf.process_bytes(b"hello ", &mut events);
        assert!(events.is_empty(), "line mode should buffer until newline");

        buf.process_bytes(b"world\n", &mut events);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StdinBufferEvent::Data(d) => {
                assert_eq!(&String::from_utf8_lossy(d), "hello world\n");
            }
            _ => panic!("expected Data event"),
        }
    }

    // -- Test 6: SGR mouse sequence ----------------------------------------

    #[test]
    fn test_sgr_mouse_sequence() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        buf.process_bytes(b"\x1b[<35;20;5m", &mut events);

        assert_eq!(events.len(), 1);
        match &events[0] {
            StdinBufferEvent::Data(d) => {
                assert_eq!(&String::from_utf8_lossy(d), "\x1b[<35;20;5m");
            }
            _ => panic!("expected Data event"),
        }
    }

    // -- Test 7: flush forces incomplete data out --------------------------

    #[test]
    fn test_flush_incomplete_sequence() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        buf.process_bytes(b"\x1b[", &mut events);
        assert!(events.is_empty(), "incomplete CSI should not emit");

        let flushed = buf.flush();
        assert_eq!(flushed.len(), 1);
        match &flushed[0] {
            StdinBufferEvent::Data(d) => {
                assert_eq!(d, &vec![0x1b, b'[']);
            }
            _ => panic!("expected Data from flush"),
        }
    }

    // -- Test 8: clear discards everything ---------------------------------

    #[test]
    fn test_clear_discards_buffered_data() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        buf.process_bytes(b"\x1b[", &mut events);
        assert!(events.is_empty());
        assert!(buf.buffered_len() > 0);

        buf.clear();
        assert_eq!(buf.buffered_len(), 0);

        let flushed = buf.flush();
        assert!(flushed.is_empty());
    }

    // -- Test 9: mixed plain text and escape sequences ---------------------

    #[test]
    fn test_mixed_text_and_escapes() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        buf.process_bytes(b"a\x1b[Ab", &mut events);

        let data_events: Vec<Vec<u8>> = events
            .into_iter()
            .filter_map(|e| match e {
                StdinBufferEvent::Data(d) => Some(d),
                _ => None,
            })
            .collect();

        assert_eq!(data_events.len(), 3);
        assert_eq!(data_events[0], vec![b'a']);
        assert_eq!(data_events[1], vec![0x1b, b'[', b'A']);
        assert_eq!(data_events[2], vec![b'b']);
    }

    // -- Test 10: Kitty printable codepoint deduplication ------------------

    #[test]
    fn test_kitty_deduplication() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        // Kitty sends: ESC[97u (for 'a'), then raw 'a'
        buf.process_bytes(b"\x1b[97ua", &mut events);

        // The raw 'a' should be suppressed because it matches the Kitty codepoint
        let data_events: Vec<Vec<u8>> = events
            .into_iter()
            .filter_map(|e| match e {
                StdinBufferEvent::Data(d) => Some(d),
                _ => None,
            })
            .collect();

        assert_eq!(data_events.len(), 1, "raw char should be suppressed");
        assert_eq!(data_events[0], vec![0x1b, b'[', b'9', b'7', b'u']);
    }

    // -- Test 11: drain reads all available data ---------------------------

    #[test]
    fn test_drain_reads_all() {
        let data = b"hello\x1b[Aworld";
        let cursor = Cursor::new(data.to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let events = buf.drain().unwrap();
        let data_events: Vec<Vec<u8>> = events
            .into_iter()
            .filter_map(|e| match e {
                StdinBufferEvent::Data(d) => Some(d),
                _ => None,
            })
            .collect();

        // h, e, l, l, o, ESC[A, w, o, r, l, d
        assert_eq!(data_events.len(), 11);
    }

    // -- Test 12: SS3 sequence (ESC O P = F1) -----------------------------

    #[test]
    fn test_ss3_sequence() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        buf.process_bytes(b"\x1bOP", &mut events); // F1 key

        assert_eq!(events.len(), 1);
        match &events[0] {
            StdinBufferEvent::Data(d) => {
                assert_eq!(d, &vec![0x1b, b'O', b'P']);
            }
            _ => panic!("expected Data event"),
        }
    }

    // -- Test 13: OSC sequence with BEL terminator ------------------------

    #[test]
    fn test_osc_bel_terminated() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        buf.process_bytes(b"\x1b]0;title\x07", &mut events);

        assert_eq!(events.len(), 1);
        match &events[0] {
            StdinBufferEvent::Data(d) => {
                assert_eq!(&String::from_utf8_lossy(d), "\x1b]0;title\x07");
            }
            _ => panic!("expected Data event"),
        }
    }

    // -- Test 14: sequence completeness helpers ----------------------------

    #[test]
    fn test_is_complete_sequence_helpers() {
        // Plain char
        assert_eq!(is_complete_sequence(b"a"), SequenceStatus::NotEscape);

        // Bare ESC
        assert_eq!(is_complete_sequence(b"\x1b"), SequenceStatus::Incomplete);

        // Complete CSI
        assert_eq!(is_complete_sequence(b"\x1b[A"), SequenceStatus::Complete);

        // Incomplete CSI
        assert_eq!(is_complete_sequence(b"\x1b["), SequenceStatus::Incomplete);

        // SS3
        assert_eq!(is_complete_sequence(b"\x1bOP"), SequenceStatus::Complete);

        // Meta (ESC + char)
        assert_eq!(is_complete_sequence(b"\x1ba"), SequenceStatus::Complete);
    }

    // -- Test 15: find_subsequence -----------------------------------------

    #[test]
    fn test_find_subsequence() {
        assert_eq!(find_subsequence(b"hello world", b"world"), Some(6));
        assert_eq!(find_subsequence(b"hello", b"xyz"), None);
        assert_eq!(find_subsequence(b"abc", b"abc"), Some(0));
        assert_eq!(find_subsequence(b"", b"abc"), None);
        assert_eq!(find_subsequence(b"abc", b""), Some(0));
    }

    // -- Test 16: extract_complete_sequences -------------------------------

    #[test]
    fn test_extract_complete_sequences() {
        let (seqs, rem) = extract_complete_sequences(b"a\x1b[Ab");
        assert_eq!(seqs.len(), 3);
        assert_eq!(seqs[0], vec![b'a']);
        assert_eq!(seqs[1], vec![0x1b, b'[', b'A']);
        assert_eq!(seqs[2], vec![b'b']);
        assert!(rem.is_empty());

        // Incomplete escape at end
        let (seqs, rem) = extract_complete_sequences(b"a\x1b[");
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0], vec![b'a']);
        assert_eq!(rem, vec![0x1b, b'[']);
    }

    // -- Test 17: parse_kitty_printable_codepoint --------------------------

    #[test]
    fn test_parse_kitty_printable_codepoint() {
        assert_eq!(parse_kitty_printable_codepoint(b"\x1b[97u"), Some(97)); // 'a'
        assert_eq!(parse_kitty_printable_codepoint(b"\x1b[97:1u"), Some(97));
        assert_eq!(parse_kitty_printable_codepoint(b"\x1b[10u"), None); // codepoint < 32
        assert_eq!(parse_kitty_printable_codepoint(b"\x1b[A"), None); // not kitty
        assert_eq!(parse_kitty_printable_codepoint(b"\x1b[65;1u"), Some(65)); // 'A' with semicolon
    }

    // -- Test 18: old-style mouse sequence ---------------------------------

    #[test]
    fn test_old_style_mouse_sequence() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let mut events = Vec::new();
        // ESC[M + 3 bytes
        buf.process_bytes(b"\x1b[M \x20\x20", &mut events);

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            StdinBufferEvent::Data(vec![0x1b, b'[', b'M', b' ', 0x20, 0x20])
        );
    }

    // -- Test 19: read_into_buffer -----------------------------------------

    #[test]
    fn test_read_into_buffer() {
        let data = b"test data";
        let cursor = Cursor::new(data.to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        let events = buf.read_into_buffer(1024).unwrap();
        let combined: Vec<u8> = events
            .into_iter()
            .filter_map(|e| match e {
                StdinBufferEvent::Data(d) => Some(d),
                _ => None,
            })
            .flatten()
            .collect();

        assert_eq!(combined, b"test data");
    }

    // -- Test 20: set_mode switching ---------------------------------------

    #[test]
    fn test_set_mode_switching() {
        let cursor = Cursor::new(b"".to_vec());
        let mut buf = StdinBuffer::new(cursor, StdinBufferOptions::default());

        assert_eq!(buf.mode(), BufferingMode::Character);
        buf.set_mode(BufferingMode::Line);
        assert_eq!(buf.mode(), BufferingMode::Line);
        buf.set_mode(BufferingMode::Character);
        assert_eq!(buf.mode(), BufferingMode::Character);
    }
}
