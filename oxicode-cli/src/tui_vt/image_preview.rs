//! Inline image preview support for kitty / iTerm2 graphics protocols.
//!
//! `detect_image_support` looks at environment variables to pick a protocol.
//! `kitty_transmit_png` / `kitty_place` / `iterm_inline_png` / `text_fallback`
//! are pure escape-sequence builders — they return `String`s the caller can
//! emit through whatever terminal backend it owns. This module deliberately
//! does NOT touch I/O; rendering and stdout writes are the caller's job, so
//! the encoders stay unit-testable.
//!
//! Live viewport: transmit once (dedup by content-hash id) + place; rows
//! committed to scrollback use `text_fallback` only — image pixels are not
//! expected to survive terminal history (omp lesson).
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
//! and <https://iterm2.com/documentation-images.html>.

use base64::{Engine, engine::general_purpose};

/// Which inline-image protocol the host terminal supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSupport {
    /// Kitty graphics protocol (`xterm-kitty` or `KITTY_WINDOW_ID`).
    Kitty,
    /// iTerm2 OSC 1337 (`TERM_PROGRAM=iTerm.app`).
    Iterm2,
    /// No inline-image support — caller must use `text_fallback`.
    None,
}

/// Default maximum number of concurrently-transmitted images the terminal is
/// expected to keep alive. When a new transmission would push us over the
/// limit, the budget evicts the oldest id and returns its delete command so
/// the caller can emit it before the new transmit.
pub const IMAGE_BUDGET_LIMIT: usize = 8;

/// Decide which protocol the host terminal supports, from the live env.
///
/// `OXICODE_FORCE_IMAGE_TERM` (`kitty` | `iterm2` | `none`, aliases accepted)
/// overrides auto-detection — used by tests and misdetected terminals.
pub fn detect_image_support() -> ImageSupport {
    let force = std::env::var("OXICODE_FORCE_IMAGE_TERM").ok();
    let kitty_window = std::env::var("KITTY_WINDOW_ID").ok();
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    detect_image_support_from(
        force.as_deref(),
        kitty_window.as_deref(),
        &term,
        &term_program,
    )
}

/// Pure decision core — env values passed in so tests can build a matrix.
///
/// Priority: force override > `KITTY_WINDOW_ID` set > `TERM=xterm-kitty` >
/// `TERM_PROGRAM=iTerm.app` > `None`.
pub fn detect_image_support_from(
    force: Option<&str>,
    kitty_window: Option<&str>,
    term: &str,
    term_program: &str,
) -> ImageSupport {
    if let Some(v) = force {
        match v.trim().to_ascii_lowercase().as_str() {
            "kitty" | "xterm-kitty" => return ImageSupport::Kitty,
            "iterm" | "iterm2" | "iterm.app" => return ImageSupport::Iterm2,
            "none" | "off" | "disable" | "disabled" => return ImageSupport::None,
            _ => {}
        }
    }
    if kitty_window.is_some_and(|s| !s.is_empty()) {
        return ImageSupport::Kitty;
    }
    if term.eq_ignore_ascii_case("xterm-kitty") {
        return ImageSupport::Kitty;
    }
    if term_program.eq_ignore_ascii_case("iTerm.app") {
        return ImageSupport::Iterm2;
    }
    ImageSupport::None
}

/// Build a kitty transmit escape sequence (APC `G`) — transmit only, no
/// display (`a=t`).
///
/// Control data on the first chunk: `a=t,f=100,t=d,q=2,i=<id>`
/// - `f=100` selects PNG (dimensions come from the PNG itself);
/// - `t=d` is DIRECT transmission — the payload is inline base64, not a
///   file path;
/// - `q=2` suppresses ALL terminal responses (OK and failure) so no
///   unsolicited replies land on the TUI's input stream;
/// - `i=<id>` pins a stable id for later `a=p` placement.
///
/// The payload is base64 of the PNG bytes with `\` → `\\` and `,` → `\c`
/// escaped, then chunked per the spec: each APC carries at most 4096
/// payload bytes, every chunk except the last is a multiple of 4 base64
/// characters, non-final chunks are tagged `m=1`, the final chunk `m=0`,
/// and only the first chunk carries the control keys.
pub fn kitty_transmit_png(id: u32, png: &[u8]) -> String {
    let b64 = escape_kitty_payload(&general_purpose::STANDARD.encode(png));
    /// Spec maximum chunk size; a multiple of 4 so chunk boundaries stay
    /// on base64 quanta.
    const CHUNK: usize = 4096;
    let mut seq = String::new();
    let mut start = 0;
    while start < b64.len() {
        let mut end = (start + CHUNK).min(b64.len());
        if end < b64.len() {
            end -= (end - start) % 4;
        }
        let first = start == 0;
        let last = end == b64.len();
        seq.push_str("\x1b_G");
        if first {
            seq.push_str("a=t,f=100,t=d,q=2,i=");
            seq.push_str(&id.to_string());
        }
        if !first || !last {
            if first {
                seq.push(','); // separator after the control keys
            }
            seq.push_str(if last { "m=0" } else { "m=1" });
        }
        seq.push(';');
        seq.push_str(&b64[start..end]);
        seq.push_str("\x1b\\");
        start = end;
    }
    seq
}

/// Build a kitty placement command for a previously-transmitted image id.
///
/// Format: `ESC _G a=p,i=<id>,r=<rows>,C=1 ESC \`
///
/// `a=p` displays the already-transmitted image at the CURRENT cursor
/// position — the emit step parks the cursor on the tool box's top row
/// with CUP first. Only `r` (rows) is given so the width follows the
/// image's aspect ratio (a `c` of 1 would squeeze it to one column).
/// `C=1` stops the terminal from moving the cursor after the placement
/// (the caller restores it with DECRC anyway, but per the spec the
/// default cursor move can otherwise land outside the scroll area).
pub fn kitty_place(id: u32, rows: u16) -> String {
    format!("\x1b_Ga=p,i={id},r={rows},C=1\x1b\\")
}

/// Build a kitty delete command for budget demotion — `d=I` (capital)
/// deletes the image's placements AND frees its stored data, provided
/// nothing else (e.g. scrollback) still references it.
pub fn kitty_delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id}\x1b\\")
}

/// Build an iTerm2 inline-image escape sequence (OSC 1337).
///
/// Format: `ESC ]1337;File=inline=1;preserveAspectRatio=1;base64=<b64> BEL`
///
/// iTerm2 sizes the image by its own cell metrics; the width/height
/// arguments are optional and omitted here so the terminal picks defaults.
pub fn iterm_inline_png(png: &[u8]) -> String {
    let b64 = general_purpose::STANDARD.encode(png);
    format!("\x1b]1337;File=inline=1;preserveAspectRatio=1;base64={b64}\x07")
}

/// Plain-text fallback used when no graphics protocol is supported or the
/// `inline_images` kill-switch is off. `path` identifies the image.
pub fn text_fallback(path: &str) -> String {
    format!("[image: {path}]")
}

/// Escape a base64 string per the kitty graphics protocol: `\` → `\\`
/// first, then `,` → `\c`. Processing char-by-char makes the order safe.
fn escape_kitty_payload(b64: &str) -> String {
    let mut out = String::with_capacity(b64.len());
    for ch in b64.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\c"),
            other => out.push(other),
        }
    }
    out
}

/// Tracks transmitted image ids so the renderer stays under the kitty
/// working-set limit ([`IMAGE_BUDGET_LIMIT`]), emitting delete escapes for
/// evicted ids. Ids should be content hashes — re-rendering the same image
/// refreshes its position instead of re-transmitting.
#[derive(Debug, Default)]
pub struct ImageBudget {
    /// Ids in insertion order: `ids[0]` is the oldest, last is newest.
    ids: Vec<u32>,
}

impl ImageBudget {
    /// Construct an empty budget.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide what to emit for a new transmission of `id`. Returns:
    /// - `Some(delete_cmd)` when the budget was full and the oldest id had
    ///   to be evicted — the caller emits this BEFORE the new transmit;
    /// - `None` when no delete is required (budget had room, or the id was
    ///   already tracked — its position is refreshed, no re-transmit).
    pub fn record(&mut self, id: u32) -> Option<String> {
        // Dedup: a known id just refreshes its recency.
        if let Some(pos) = self.ids.iter().position(|x| *x == id) {
            self.ids.remove(pos);
            self.ids.push(id);
            return None;
        }
        let mut to_evict = None;
        if self.ids.len() >= IMAGE_BUDGET_LIMIT {
            let oldest = self.ids.remove(0);
            to_evict = Some(kitty_delete(oldest));
        }
        self.ids.push(id);
        to_evict
    }

    /// How many ids are currently tracked.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// True when no ids have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// True when `id` is still live in the budget (already transmitted,
    /// not evicted). Used by the emit step to skip re-transmitting a
    /// known image.
    pub fn contains(&self, id: u32) -> bool {
        self.ids.contains(&id)
    }
}

/// An image waiting for its first live placement, captured when a
/// generate_image tool result lands in the transcript.
pub struct PendingImage {
    /// Content-hash id (see [`content_hash_id`]).
    pub id: u32,
    /// Decoded PNG bytes.
    pub png: std::sync::Arc<Vec<u8>>,
    /// Marker embedded in the fallback row (`generate_image:<id>`). The
    /// render pass resolves it to a transcript row index — the row only
    /// exists after the append command flows through the harness channel,
    /// so the index cannot be known at enqueue time.
    pub label: String,
}

/// Screen position of a pending image's tool box, recorded by the live
/// render pass and consumed by the post-draw emit step.
pub struct ImageAnchor {
    pub id: u32,
    /// Column of the box (0-based screen cell).
    pub x: u16,
    /// Row of the box top (0-based screen cell).
    pub y: u16,
    /// Visual height of the box in cell rows — the placement height.
    pub rows: u16,
    /// Transcript row carrying the fallback text — the liveness check
    /// (`>= committed_entries`) runs against this at emit time.
    pub transcript_index: usize,
}

/// Render-state owner for inline image previews: protocol detection, the
/// settings kill-switch, the transmit budget, the pending queue, and the
/// per-frame anchors shared between the render pass (recorder) and the
/// post-draw emit step (consumer).
pub struct ImagePreviews {
    support: ImageSupport,
    enabled: bool,
    budget: ImageBudget,
    pending: Vec<PendingImage>,
    anchors: std::sync::Arc<parking_lot::Mutex<Vec<ImageAnchor>>>,
}

impl ImagePreviews {
    pub fn new(support: ImageSupport) -> Self {
        Self {
            support,
            enabled: true,
            budget: ImageBudget::new(),
            pending: Vec::new(),
            anchors: std::sync::Arc::new(parking_lot::Mutex::new(Vec::new())),
        }
    }

    /// Settings kill-switch (`inline_images`, default ON).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Queue a decoded image for its first live placement. Called by the
    /// generate_image result hook with the label embedded in the
    /// fallback row. The queue is capped: rows that never render live
    /// (immediately committed) would otherwise pile up forever.
    pub fn enqueue(&mut self, id: u32, png: std::sync::Arc<Vec<u8>>, label: String) {
        const MAX_PENDING: usize = 32;
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push(PendingImage { id, png, label });
    }

    /// Pending queue (inspected by tests and the render pre-pass).
    pub fn pending(&self) -> &[PendingImage] {
        &self.pending
    }

    /// Record where a pending image's tool box was painted this frame.
    /// Called from the live render pass; consumed by [`Self::emit_live`]
    /// right after the frame flushes.
    pub fn record_anchor(&self, id: u32, x: u16, y: u16, rows: u16, transcript_index: usize) {
        self.anchors.lock().push(ImageAnchor {
            id,
            x,
            y,
            rows,
            transcript_index,
        });
    }

    /// Pending queue length.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Build the image escape stream for anchors recorded during this
    /// frame's LIVE render and return it — the caller writes the string
    /// through the terminal backend. Rows already committed to
    /// scrollback never emit — the
    /// transcript's fallback text is all history keeps (omp lesson:
    /// image pixels must not be expected to survive history).
    ///
    /// Every write is wrapped in DECSC/DECRC (save/restore cursor) and
    /// CUP to the anchor so the sequences land on the tool box without
    /// disturbing the frame's cursor state.
    pub fn emit_live(&mut self, committed_entries: usize) -> String {
        let anchors = std::mem::take(&mut *self.anchors.lock());
        if self.pending.is_empty() && anchors.is_empty() {
            return String::new();
        }
        if !self.enabled || self.support == ImageSupport::None {
            // Nothing will ever be emitted for these — drop the queue so
            // it cannot grow unbounded across a long session.
            self.pending.clear();
            return String::new();
        }
        let mut seq = String::new();
        for anchor in anchors {
            if anchor.transcript_index < committed_entries {
                // The row was committed between render and emit — its
                // fallback text is already in scrollback and the image
                // can never be placed. Drop the pending.
                if let Some(pos) = self.pending.iter().position(|p| p.id == anchor.id) {
                    self.pending.remove(pos);
                }
                continue;
            }
            let Some(pos) = self.pending.iter().position(|p| p.id == anchor.id) else {
                continue;
            };
            let pending = self.pending.remove(pos);
            seq.push_str("\x1b7");
            seq.push_str(&format!(
                "\x1b[{};{}H",
                anchor.y.saturating_add(1),
                anchor.x.saturating_add(1)
            ));
            match self.support {
                ImageSupport::Kitty => {
                    // Transmit once per unique id; re-arrival after an
                    // eviction re-transmits (the budget dropped the data).
                    let known = self.budget.contains(pending.id);
                    if let Some(delete) = self.budget.record(pending.id) {
                        seq.push_str(&delete);
                    }
                    if !known {
                        seq.push_str(&kitty_transmit_png(pending.id, &pending.png));
                    }
                    seq.push_str(&kitty_place(pending.id, anchor.rows));
                }
                ImageSupport::Iterm2 => {
                    // OSC 1337 uploads and displays in one sequence —
                    // there is no transmit/place split to dedup.
                    seq.push_str(&iterm_inline_png(&pending.png));
                }
                ImageSupport::None => unreachable!("gated above"),
            }
            seq.push_str("\x1b8");
        }
        seq
    }
}

impl Default for ImagePreviews {
    /// Live-env detection + kill-switch default ON.
    fn default() -> Self {
        Self::new(detect_image_support())
    }
}

/// Stable content-hash id for dedup: first 32 bits of SHA-256.
pub fn content_hash_id(png: &[u8]) -> u32 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(png);
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]])
}
#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose};

    /// Env-var decision matrix — pins every detection branch.
    #[test]
    fn detect_kitty_from_env_matrix() {
        // Force override wins.
        assert_eq!(
            detect_image_support_from(Some("kitty"), None, "xterm", "iTerm.app"),
            ImageSupport::Kitty,
        );
        assert_eq!(
            detect_image_support_from(Some("iterm"), None, "xterm-kitty", "iTerm.app"),
            ImageSupport::Iterm2,
        );
        assert_eq!(
            detect_image_support_from(Some("none"), Some("42"), "xterm-kitty", "iTerm.app"),
            ImageSupport::None,
        );
        // KITTY_WINDOW_ID alone → Kitty, even when TERM is generic.
        assert_eq!(
            detect_image_support_from(None, Some("12345"), "xterm-256color", ""),
            ImageSupport::Kitty,
        );
        // TERM=xterm-kitty → Kitty.
        assert_eq!(
            detect_image_support_from(None, None, "xterm-kitty", ""),
            ImageSupport::Kitty,
        );
        // TERM_PROGRAM=iTerm.app → Iterm2.
        assert_eq!(
            detect_image_support_from(None, None, "xterm-256color", "iTerm.app"),
            ImageSupport::Iterm2,
        );
        // Anything else → None.
        assert_eq!(
            detect_image_support_from(None, None, "xterm-256color", "Apple_Terminal"),
            ImageSupport::None,
        );
        // Force override accepts aliases (case-insensitive).
        assert_eq!(
            detect_image_support_from(Some("ITERM2"), None, "xterm", ""),
            ImageSupport::Iterm2,
        );
        assert_eq!(
            detect_image_support_from(Some("disabled"), None, "xterm-kitty", ""),
            ImageSupport::None,
        );
    }

    /// Kitty payload must escape `,` (→ `\c`) and `\` (→ `\\`) in the base64
    /// stream, and the escaped payload must round-trip back to the PNG bytes.
    /// Transmission is DIRECT (`t=d` — the payload is inline base64, not a
    /// file path) and fully quiet (`q=2` — no OK/failure replies on stdin).
    #[test]
    fn kitty_transmit_contains_escaped_base64() {
        let png: &[u8] = &[0xff, 0x00, 0xff, 0x3b, 0xc3, 0x47];
        let s = kitty_transmit_png(7, png);
        assert!(s.starts_with("\x1b_G"), "must start with APC introducer");
        assert!(s.ends_with("\x1b\\"), "must end with ST terminator");
        assert!(s.contains("f=100"), "format=png");
        assert!(s.contains("t=d"), "direct inline transmission");
        assert!(s.contains("q=2"), "quiet: no terminal responses");
        assert!(s.contains("i=7"), "id carried");
        // No raw commas survive inside the payload portion.
        let payload = s
            .trim_start_matches("\x1b_G")
            .trim_end_matches("\x1b\\")
            .split_once(';')
            .map(|(_, p)| p)
            .expect("kv block ends with ';'");
        assert!(!payload.contains(','), "all commas must be escaped as \\c");
        // Escaping must round-trip: unescape → base64 → original bytes.
        let unescaped = unescape_kitty_payload(payload);
        let decoded = general_purpose::STANDARD.decode(&unescaped).unwrap();
        assert_eq!(decoded, png);
    }

    /// Payloads whose base64 exceeds the 4096-byte APC chunk limit must be
    /// transmitted as m=1/m=0 chunks: the first chunk carries the control
    /// keys, subsequent chunks carry only `m` (plus `q`), and every chunk
    /// except the last is a multiple of 4 base64 characters.
    #[test]
    fn kitty_transmit_chunks_large_payloads() {
        // 4096 base64 chars encode 3072 bytes; use 10000 bytes → ~13336
        // base64 chars → 4 chunks (4096 + 4096 + 4096 + ~1048).
        let png: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let s = kitty_transmit_png(11, &png);
        let apcs: Vec<&str> = s.split("\x1b_G").skip(1).collect();
        assert_eq!(apcs.len(), 4, "4 chunks for ~13.3k base64 chars");
        // First chunk: full control data + m=1.
        assert!(apcs[0].starts_with("a=t,f=100,t=d,q=2,i=11,m=1;"));
        // Middle chunks: only m=1 before the payload.
        for mid in &apcs[1..3] {
            assert!(
                mid.starts_with("m=1;"),
                "middle chunks carry only m: {mid:?}"
            );
        }
        // Last chunk: m=0.
        assert!(apcs[3].starts_with("m=0;"), "final chunk marks m=0");
        // Chunk sizes (payload between ';' and the ST): all but the last
        // are multiples of 4 base64 chars, none exceeds 4096.
        let payloads: Vec<&str> = apcs
            .iter()
            .map(|c| c.split(';').nth(1).unwrap_or("").trim_end_matches("\x1b\\"))
            .collect();
        for (i, pl) in payloads.iter().enumerate() {
            assert!(pl.len() <= 4096, "chunk {i} within the 4096 limit");
            if i < payloads.len() - 1 {
                assert!(pl.len() % 4 == 0, "chunk {i} multiple of 4");
            }
        }
        // Reassembling the payloads round-trips the PNG.
        let joined: String = payloads.concat();
        let unescaped = unescape_kitty_payload(&joined);
        let decoded = general_purpose::STANDARD.decode(&unescaped).unwrap();
        assert_eq!(decoded, png);
    }

    /// iTerm2 OSC 1337 wrapper layout and base64 round-trip.
    #[test]
    fn iterm_osc1337_wraps_base64() {
        let png: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let s = iterm_inline_png(png);
        assert!(s.starts_with("\x1b]1337;File=inline=1"));
        assert!(s.contains("preserveAspectRatio=1"));
        assert!(s.contains("base64="));
        assert!(s.ends_with('\x07'), "iTerm2 uses BEL as terminator");
        let b64 = s
            .split("base64=")
            .nth(1)
            .and_then(|t| t.strip_suffix('\x07'))
            .expect("base64= present");
        assert_eq!(general_purpose::STANDARD.decode(b64).unwrap(), png);
    }

    /// Plain-text fallback shape.
    #[test]
    fn fallback_format() {
        assert_eq!(text_fallback("/tmp/a.png"), "[image: /tmp/a.png]");
        assert_eq!(text_fallback(""), "[image: ]");
    }

    /// Budget keeps at most `IMAGE_BUDGET_LIMIT` live ids, evicting the
    /// oldest with a delete command when full, and refreshes position on
    /// re-record.
    #[test]
    fn image_budget_evicts_oldest() {
        let mut b = ImageBudget::new();
        for i in 0..IMAGE_BUDGET_LIMIT as u32 {
            assert!(b.record(i).is_none(), "no eviction for slot {i}");
        }
        assert_eq!(b.len(), IMAGE_BUDGET_LIMIT);
        // The 9th id forces eviction of the oldest (0).
        let evicted = b.record(99).expect("must emit delete for evicted id");
        assert!(
            evicted.starts_with("\x1b_Ga=d,d=I"),
            "demotion uses d=I so the terminal frees the image data"
        );
        assert!(evicted.contains("i=0"), "delete targets the evicted id");
        assert_eq!(b.len(), IMAGE_BUDGET_LIMIT, "budget stays capped");
        // Recording an existing id does NOT trigger eviction and refreshes
        // its position so it is no longer the oldest.
        assert!(b.record(99).is_none());
        let evicted = b.record(100).expect("eviction continues");
        assert!(evicted.contains("i=1"), "the new oldest is id=1, not 99");
    }

    /// Live emit (kitty): save-cursor + CUP at the anchor + budget-checked
    /// transmit-once + placement, then restore-cursor. The placed image
    /// leaves the pending queue.
    #[test]
    fn emit_live_kitty_writes_transmit_and_place_at_anchor() {
        let mut p = ImagePreviews::new(ImageSupport::Kitty);
        let png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let id = content_hash_id(&png);
        p.enqueue(
            id,
            std::sync::Arc::new(png),
            format!("generate_image:{id:08x}"),
        );
        p.record_anchor(id, 2, 10, 6, 3);
        let s = p.emit_live(0);
        assert!(s.contains("\x1b7"), "save cursor first");
        assert!(s.contains("\x1b[11;3H"), "CUP to anchor (1-based)");
        assert!(s.contains("\x1b_Ga=t,f=100"), "transmit present");
        assert!(s.contains(&format!("i={id}")), "stable content-hash id");
        assert!(s.contains("a=p"), "placement present");
        assert!(s.contains("r=6"), "placement sized to the box rows");
        assert!(s.contains("C=1"), "placement must not move the cursor");
        assert!(s.contains("\x1b8"), "restore cursor last");
        assert_eq!(p.pending_len(), 0, "placed image leaves the pending queue");
    }

    /// Live emit gates: rows already committed to scrollback are dropped
    /// without writing; the kill-switch and `ImageSupport::None` suppress
    /// every write (the transcript's fallback text is all the user gets).
    #[test]
    fn emit_live_skips_committed_rows_and_disabled() {
        let png = vec![1u8, 2, 3, 4];
        let id = content_hash_id(&png);

        // Committed row → dropped, nothing written.
        let mut p = ImagePreviews::new(ImageSupport::Kitty);
        p.enqueue(id, std::sync::Arc::new(png.clone()), String::new());
        p.record_anchor(id, 0, 0, 5, 3);
        assert!(p.emit_live(4).is_empty(), "committed rows never transmit");
        assert_eq!(p.pending_len(), 0, "committed pending dropped");

        // Kill-switch off → nothing written.
        let mut p = ImagePreviews::new(ImageSupport::Kitty);
        p.set_enabled(false);
        p.enqueue(id, std::sync::Arc::new(png.clone()), String::new());
        p.record_anchor(id, 0, 0, 5, 0);
        assert!(
            p.emit_live(0).is_empty(),
            "kill-switch suppresses all writes"
        );

        // No protocol support → nothing written.
        let mut p = ImagePreviews::new(ImageSupport::None);
        p.enqueue(id, std::sync::Arc::new(png), String::new());
        p.record_anchor(id, 0, 0, 5, 0);
        assert!(
            p.emit_live(0).is_empty(),
            "unsupported terminals get text only"
        );
    }

    /// Live emit (iTerm2): the OSC 1337 sequence both uploads and displays
    /// at the anchored cursor — no separate transmit/place split.
    #[test]
    fn emit_live_iterm_writes_osc1337_at_anchor() {
        let mut p = ImagePreviews::new(ImageSupport::Iterm2);
        let png = vec![0x89, 0x50, 0x4e, 0x47];
        let id = content_hash_id(&png);
        p.enqueue(id, std::sync::Arc::new(png), String::new());
        p.record_anchor(id, 0, 4, 5, 0);
        let s = p.emit_live(0);
        assert!(s.contains("\x1b[5;1H"), "cursor parked on the anchor row");
        assert!(s.contains("\x1b]1337;File=inline=1"), "inline upload");
    }

    /// Content-hash ids are stable for identical bytes and distinct for
    /// different bytes — the dedup key for transmit-once.
    #[test]
    fn content_hash_id_stable_and_distinct() {
        assert_eq!(
            content_hash_id(b"hello world"),
            content_hash_id(b"hello world")
        );
        assert_ne!(
            content_hash_id(b"hello world"),
            content_hash_id(b"hello worlD")
        );
    }

    /// Inverse of `escape_kitty_payload`, used to prove the escaping
    /// round-trips (kept in tests — production never needs to unescape).
    fn unescape_kitty_payload(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('\\') => out.push('\\'),
                    Some('c') => out.push(','),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
