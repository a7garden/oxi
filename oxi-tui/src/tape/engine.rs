//! TapeEngine — the core append-only native-scrollback rendering engine.
//!
//! Translates omp's `TUI` class (`packages/tui/src/tui.ts`). The engine
//! writes finalized rows to the terminal's native scrollback (immutable),
//! and repaints only the live region (mutable suffix) in-place.
//!
//! **Standalone module** — not wired into the default render path. Future
//! integration will be gated behind `OXI_TAPE_RENDER=1`.

use std::io::{self, Write};

use super::component::LiveRegion;
use super::line::{LINE_TERMINATOR, line_to_ansi};

// ── Escape sequences ─────────────────────────────────────────────────────

const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";
const SYNC_BEGIN: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";
const DISABLE_AUTOWRAP: &str = "\x1b[?7l";
const ENABLE_AUTOWRAP: &str = "\x1b[?7h";
const CURSOR_HOME: &str = "\x1b[H";
#[allow(dead_code)] // available for future screen-clear paths
const ED2_CLEAR_SCREEN: &str = "\x1b[2J";
const ED3_CLEAR_SCROLLBACK: &str = "\x1b[3J";
const CRLF: &str = "\r\n";

fn paint_begin(sync: bool) -> String {
    if sync {
        format!("{HIDE_CURSOR}{SYNC_BEGIN}{DISABLE_AUTOWRAP}")
    } else {
        format!("{HIDE_CURSOR}{DISABLE_AUTOWRAP}")
    }
}

fn paint_end(sync: bool) -> String {
    if sync {
        format!("{ENABLE_AUTOWRAP}{SYNC_END}")
    } else {
        ENABLE_AUTOWRAP.to_string()
    }
}
// ── Frame outcome ────────────────────────────────────────────────────────

/// The result of painting one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The engine wrote bytes to the terminal.
    Rendered {
        /// Number of bytes written to the terminal.
        bytes_written: usize,
    },
    /// Nothing changed — the frame is identical to the previous one.
    Idle,
}

// ── Paint options ─────────────────────────────────────────────────────────

/// Options for the next paint.
#[derive(Debug, Clone, Default)]
pub struct PaintOptions {
    /// Clear native scrollback before painting (session replace).
    pub clear_scrollback: bool,
}

// ── TapeEngine ───────────────────────────────────────────────────────────

/// Append-only tape rendering engine.
///
/// Writes to any `io::Write` implementor. The caller is responsible for
/// terminal mode management (raw mode, alt screen enter/exit).
///
/// **Not wired into the default render path.** Future integration gated
/// behind `OXI_TAPE_RENDER=1`.
pub struct TapeEngine<W: Write> {
    out: W,
    committed_rows: usize,
    window_top: usize,
    previous_window: Vec<String>,
    previous_width: u16,
    previous_height: u16,
    has_ever_rendered: bool,
    clear_scrollback_next: bool,
    synchronized_output: bool,
}

impl<W: Write> TapeEngine<W> {
    /// Create a new tape engine writing to `out`.
    #[must_use]
    pub fn new(out: W) -> Self {
        Self {
            out,
            committed_rows: 0,
            window_top: 0,
            previous_window: Vec::new(),
            previous_width: 0,
            previous_height: 0,
            has_ever_rendered: false,
            clear_scrollback_next: false,
            synchronized_output: true,
        }
    }

    /// Whether to wrap output in CSI 2026 synchronized-update mode.
    pub fn set_synchronized_output(&mut self, enabled: bool) {
        self.synchronized_output = enabled;
    }

    /// Request a scrollback clear on the next paint.
    pub fn clear_scrollback(&mut self) {
        self.clear_scrollback_next = true;
    }

    /// Get a reference to the underlying writer.
    #[must_use]
    pub fn writer(&self) -> &W {
        &self.out
    }

    /// Get a mutable reference to the underlying writer.
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.out
    }

    /// Paint one frame.
    pub fn paint(
        &mut self,
        frame: &[String],
        live_region: LiveRegion,
        width: u16,
        height: u16,
    ) -> io::Result<FrameOutcome> {
        self.paint_with_options(frame, live_region, width, height, &PaintOptions::default())
    }

    /// Paint one frame with explicit options.
    pub fn paint_with_options(
        &mut self,
        frame: &[String],
        live_region: LiveRegion,
        width: u16,
        height: u16,
        options: &PaintOptions,
    ) -> io::Result<FrameOutcome> {
        let frame_len = frame.len();
        let live_start = live_region.start().unwrap_or(frame_len);
        let final_boundary = live_start.min(frame_len);

        let width_changed = self.previous_width > 0 && self.previous_width != width;
        let height_changed = self.previous_height > 0 && self.previous_height != height;
        let geometry_changed = width_changed || height_changed;
        let first_paint = !self.has_ever_rendered;
        let clear_requested = self.clear_scrollback_next || options.clear_scrollback;
        let full_paint = first_paint || clear_requested || geometry_changed;

        let window_top = frame_len.saturating_sub(height as usize);
        let visible_end = frame_len.min(window_top + height as usize);
        let window: Vec<&String> = frame[window_top..visible_end].iter().collect();

        // Idle check: no commits, no window change, no geometry change
        if !full_paint && !clear_requested {
            let chunk_to = final_boundary.min(window_top);
            let new_commits = chunk_to.saturating_sub(self.committed_rows);
            if new_commits == 0
                && window_top == self.window_top
                && window.len() == self.previous_window.len()
                && window
                    .iter()
                    .zip(self.previous_window.iter())
                    .all(|(c, p)| *c == p)
            {
                self.clear_scrollback_next = false;
                return Ok(FrameOutcome::Idle);
            }
        }

        let bytes = if full_paint {
            self.emit_full_paint(
                frame,
                &window,
                width,
                height,
                window_top,
                final_boundary,
                clear_requested,
            )?
        } else {
            self.emit_update(frame, &window, width, height, window_top, final_boundary)?
        };

        self.previous_window = window.iter().map(|s| (*s).clone()).collect();
        self.previous_width = width;
        self.previous_height = height;
        self.has_ever_rendered = true;
        self.clear_scrollback_next = false;
        self.window_top = window_top;
        self.committed_rows = final_boundary.min(window_top);

        Ok(FrameOutcome::Rendered {
            bytes_written: bytes,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_full_paint(
        &mut self,
        frame: &[String],
        window: &[&String],
        width: u16,
        height: u16,
        _window_top: usize,
        chunk_to: usize,
        clear_scrollback: bool,
    ) -> io::Result<usize> {
        let mut buf = String::new();
        buf.push_str(&paint_begin(self.synchronized_output));

        if clear_scrollback {
            buf.push_str(CURSOR_HOME);
            buf.push_str(ED3_CLEAR_SCROLLBACK);
        } else {
            buf.push_str(CURSOR_HOME);
        }

        let prefix_end = chunk_to.min(frame.len());
        for (i, line) in frame[..prefix_end].iter().enumerate() {
            if i > 0 {
                buf.push_str(CRLF);
            }
            buf.push_str(&line_to_ansi(line, width));
            buf.push_str(LINE_TERMINATOR);
        }

        for (screen_row, line) in window.iter().enumerate() {
            if prefix_end > 0 || screen_row > 0 {
                buf.push_str(CRLF);
            }
            buf.push_str(&line_to_ansi(line, width));
            buf.push_str(LINE_TERMINATOR);
        }

        for _ in window.len()..height as usize {
            buf.push_str(CRLF);
            buf.push_str(LINE_TERMINATOR);
        }

        buf.push_str(&paint_end(self.synchronized_output));
        self.out.write_all(buf.as_bytes())?;
        Ok(buf.len())
    }

    fn emit_update(
        &mut self,
        frame: &[String],
        window: &[&String],
        width: u16,
        height: u16,
        window_top: usize,
        final_boundary: usize,
    ) -> io::Result<usize> {
        let chunk_from = self.committed_rows;
        let chunk_to = final_boundary.min(window_top);
        let chunk_length = chunk_to.saturating_sub(chunk_from);
        let scroll = window_top.saturating_sub(self.window_top);

        // Scroll-append fast path
        if chunk_length > 0
            && chunk_length == scroll
            && scroll < height as usize
            && chunk_from == self.window_top
            && self.previous_window.len() == height as usize
            && (0..chunk_length).all(|i| {
                self.previous_window
                    .get(i)
                    .zip(frame.get(chunk_from + i))
                    .is_some_and(|(p, c)| p == c)
            })
        {
            return self.emit_scroll_append(frame, window, width, height, scroll);
        }

        // Chunk commit path
        if chunk_length > 0 {
            return self.emit_chunk_commit(frame, window, width, height, chunk_from, chunk_length);
        }

        // Pure in-window diff
        self.emit_in_window_diff(window, width)
    }

    fn emit_scroll_append(
        &mut self,
        _frame: &[String],
        window: &[&String],
        width: u16,
        height: u16,
        scroll: usize,
    ) -> io::Result<usize> {
        let mut buf = String::new();
        buf.push_str(&paint_begin(self.synchronized_output));

        // Move to bottom
        buf.push_str(&format!("\x1b[{}B", height as usize - 1));

        // Write new bottom rows
        for r in (height as usize - scroll)..(height as usize) {
            buf.push_str(CRLF);
            if let Some(line) = window.get(r) {
                buf.push_str(&line_to_ansi(line, width));
                buf.push_str(LINE_TERMINATOR);
            }
        }

        // Rewrite changed rows after the shift
        let mut first_changed: Option<usize> = None;
        let mut last_changed = 0usize;
        for r in 0..(height as usize - scroll) {
            let prev = self.previous_window.get(r + scroll);
            let curr = window.get(r);
            if prev.zip(curr).is_none_or(|(p, c)| p != *c) {
                if first_changed.is_none() {
                    first_changed = Some(r);
                }
                last_changed = r;
            }
        }

        if let Some(fc) = first_changed {
            let up = (height as usize - 1).saturating_sub(fc);
            if up > 0 {
                buf.push_str(&format!("\x1b[{up}A"));
            }
            buf.push('\r');
            for r in fc..=last_changed {
                if r > fc {
                    buf.push_str(CRLF);
                }
                if let Some(line) = window.get(r) {
                    buf.push_str(&line_to_ansi(line, width));
                    buf.push_str(LINE_TERMINATOR);
                }
            }
        }

        buf.push_str(&paint_end(self.synchronized_output));
        self.out.write_all(buf.as_bytes())?;
        Ok(buf.len())
    }

    fn emit_chunk_commit(
        &mut self,
        frame: &[String],
        window: &[&String],
        width: u16,
        height: u16,
        chunk_from: usize,
        chunk_length: usize,
    ) -> io::Result<usize> {
        let mut buf = String::new();
        buf.push_str(&paint_begin(self.synchronized_output));

        // Write committed rows (they scroll into native scrollback)
        for i in 0..chunk_length {
            if i > 0 || self.has_ever_rendered {
                buf.push_str(CRLF);
            }
            if let Some(line) = frame.get(chunk_from + i) {
                buf.push_str(&line_to_ansi(line, width));
                buf.push_str(LINE_TERMINATOR);
            }
        }

        // Rewrite visible window
        for (r, line) in window.iter().enumerate() {
            if r > 0 || chunk_length > 0 {
                buf.push_str(CRLF);
            }
            buf.push_str(&line_to_ansi(line, width));
            buf.push_str(LINE_TERMINATOR);
        }

        for _ in window.len()..height as usize {
            buf.push_str(CRLF);
            buf.push_str(LINE_TERMINATOR);
        }

        buf.push_str(&paint_end(self.synchronized_output));
        self.out.write_all(buf.as_bytes())?;
        Ok(buf.len())
    }

    fn emit_in_window_diff(&mut self, window: &[&String], width: u16) -> io::Result<usize> {
        let curr_len = window.len();
        let prev_len = self.previous_window.len();

        let mut first_changed: Option<usize> = None;
        let mut last_changed = 0usize;

        for r in 0..curr_len.max(prev_len) {
            let prev: Option<&str> = self.previous_window.get(r).map(|s| s.as_str());
            let curr: Option<&str> = window.get(r).map(|s| s.as_str());
            if prev != curr {
                if first_changed.is_none() {
                    first_changed = Some(r);
                }
                last_changed = r;
            }
        }

        let Some(fc) = first_changed else {
            return Ok(0);
        };
        let mut buf = String::new();
        buf.push_str(&paint_begin(self.synchronized_output));
        buf.push_str(CURSOR_HOME);
        if fc > 0 {
            buf.push_str(&format!("\x1b[{fc}B"));
        }
        buf.push('\r');

        for r in fc..=last_changed {
            if r > fc {
                buf.push_str(CRLF);
            }
            if let Some(line) = window.get(r) {
                buf.push_str(&line_to_ansi(line, width));
                buf.push_str(LINE_TERMINATOR);
            } else {
                buf.push_str(LINE_TERMINATOR);
            }
        }

        buf.push_str(&paint_end(self.synchronized_output));
        self.out.write_all(buf.as_bytes())?;
        Ok(buf.len())
    }

    /// Show the hardware cursor.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.out.write_all(SHOW_CURSOR.as_bytes())
    }

    /// Hide the hardware cursor.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.out.write_all(HIDE_CURSOR.as_bytes())
    }

    /// Flush the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// Consume the engine and return the underlying writer.
    #[must_use]
    pub fn into_inner(self) -> W {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn first_paint_emits_full_paint() {
        let mut buf = Vec::new();
        {
            let mut engine = TapeEngine::new(&mut buf);
            engine.set_synchronized_output(false);
            let f = frame(&["line1", "line2", "line3"]);
            let r = engine.paint(&f, LiveRegion::None, 80, 24).unwrap();
            assert!(matches!(r, FrameOutcome::Rendered { .. }));
        }
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("line1"));
        assert!(out.contains("line3"));
        assert!(out.contains(HIDE_CURSOR));
    }

    #[test]
    fn identical_frame_is_idle() {
        let mut buf = Vec::new();
        let f = frame(&["a", "b"]);
        let mut engine = TapeEngine::new(&mut buf);
        engine.set_synchronized_output(false);
        engine.paint(&f, LiveRegion::None, 80, 24).unwrap();
        let r2 = engine.paint(&f, LiveRegion::None, 80, 24).unwrap();
        assert_eq!(r2, FrameOutcome::Idle);
    }

    #[test]
    fn changed_frame_writes_diff() {
        let mut buf = Vec::new();
        let f1 = frame(&["a", "b", "c"]);
        let f2 = frame(&["a", "X", "c"]);
        let mut engine = TapeEngine::new(&mut buf);
        engine.set_synchronized_output(false);
        engine.paint(&f1, LiveRegion::None, 80, 24).unwrap();
        let r2 = engine.paint(&f2, LiveRegion::None, 80, 24).unwrap();
        assert!(matches!(r2, FrameOutcome::Rendered { .. }));
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("X"));
    }

    #[test]
    fn clear_scrollback_emits_ed3() {
        let mut buf = Vec::new();
        let mut engine = TapeEngine::new(&mut buf);
        engine.set_synchronized_output(false);
        engine.clear_scrollback();
        engine
            .paint(&frame(&["x"]), LiveRegion::None, 80, 24)
            .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains(ED3_CLEAR_SCROLLBACK));
    }

    #[test]
    fn resize_triggers_full_paint() {
        let f = frame(&["a", "b"]);
        let mut engine = TapeEngine::new(Vec::new());
        engine.set_synchronized_output(false);
        engine.paint(&f, LiveRegion::None, 80, 24).unwrap();
        engine.paint(&f, LiveRegion::None, 100, 30).unwrap();
        let out = String::from_utf8_lossy(engine.writer());
        assert!(out.contains(CURSOR_HOME));
    }

    #[test]
    fn synchronized_output_wraps_frame() {
        let mut buf = Vec::new();
        let mut engine = TapeEngine::new(&mut buf);
        let f = frame(&["test"]);
        engine.paint(&f, LiveRegion::None, 80, 24).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains(SYNC_BEGIN));
        assert!(out.contains(SYNC_END));
    }

    #[test]
    fn large_frame_commits_overflow() {
        let mut buf = Vec::new();
        let f: Vec<String> = (0..30).map(|i| format!("row{i}")).collect();
        let mut engine = TapeEngine::new(&mut buf);
        engine.set_synchronized_output(false);
        engine.paint(&f, LiveRegion::None, 80, 10).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("row0"));
        assert!(out.contains("row29"));
    }

    #[test]
    fn streaming_updates_only_write_changed() {
        let f1: Vec<String> = (0..5).map(|i| format!("line{i}")).collect();
        let mut f2 = f1.clone();
        f2[4] = "line4-updated".into();

        let mut engine = TapeEngine::new(Vec::new());
        engine.set_synchronized_output(false);
        engine
            .paint(&f1, LiveRegion::Mutable { start: 4 }, 80, 24)
            .unwrap();
        let first_len = engine.writer().len();
        let r = engine
            .paint(&f2, LiveRegion::Mutable { start: 4 }, 80, 24)
            .unwrap();
        assert!(matches!(r, FrameOutcome::Rendered { .. }));
        let all_out = String::from_utf8_lossy(engine.writer());
        let second_out = &all_out[first_len..];
        assert!(
            second_out.contains("line4-updated"),
            "changed row should be in diff"
        );
        assert!(
            !second_out.contains("line0"),
            "unchanged rows should not be in diff"
        );
    }

    #[test]
    fn pinned_suffix_never_advances_committed_rows() {
        let frame: Vec<String> = (0..30).map(|i| format!("row{i}")).collect();
        let mut engine = TapeEngine::new(Vec::new());
        engine.set_synchronized_output(false);
        engine
            .paint(&frame, LiveRegion::Pinned { start: 0 }, 80, 10)
            .unwrap();
        assert_eq!(engine.committed_rows, 0);
    }

    #[test]
    fn finalized_commits_are_monotonic_until_replay_boundary() {
        let mut engine = TapeEngine::new(Vec::new());
        engine.set_synchronized_output(false);
        let first: Vec<String> = (0..15).map(|i| format!("row{i}")).collect();
        engine.paint(&first, LiveRegion::None, 80, 10).unwrap();
        let committed = engine.committed_rows;
        let second: Vec<String> = (0..20).map(|i| format!("row{i}")).collect();
        engine.paint(&second, LiveRegion::None, 80, 10).unwrap();
        assert!(engine.committed_rows >= committed);

        engine.clear_scrollback();
        engine
            .paint(&frame(&["replacement"]), LiveRegion::None, 80, 10)
            .unwrap();
        assert_eq!(engine.committed_rows, 0, "replay starts a new commit epoch");
    }

    #[test]
    fn synchronized_update_sequences_are_balanced() {
        let mut engine = TapeEngine::new(Vec::new());
        engine
            .paint(&frame(&["one"]), LiveRegion::None, 80, 24)
            .unwrap();
        engine
            .paint(&frame(&["changed"]), LiveRegion::None, 80, 24)
            .unwrap();
        let output = String::from_utf8_lossy(engine.writer());
        assert_eq!(
            output.matches(SYNC_BEGIN).count(),
            output.matches(SYNC_END).count()
        );
    }
}
