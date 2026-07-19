//! Mouse scroll normalizer for cross-terminal consistent scroll feel.
//!
//! Different terminals emit different numbers of mouse events per physical
//! wheel notch:
//! - Apple Terminal: 3 events/notch
//! - Ghostty: 3 events/notch
//! - iTerm2: 1 event/notch
//! - VS Code: 1 event/notch
//! - tmux/screen/zellij: 1 event/notch (SGR mode strips extras)
//!
//! Without normalization, users perceive wildly different scroll speeds
//! depending on their terminal. This module groups events into streams
//! (80ms gap or direction change) and converts accumulated events into
//! a single normalized delta.
//!
//! ## Architecture
//!
//! ```
//! crossterm MouseEvent → ScrollNormalizer::push()
//!                              ↓
//!                       [gesture grouping]
//!                              ↓
//!                       ScrollNormalizer::flush()
//!                              ↓
//!                     NormalizedScroll { delta_lines }
//!                              ↓
//!                     App::scroll_up/down(delta_lines)
//! ```
//!
//! State is owned by `AppState` (oxi-cli). `oxi-tui` exposes this widget
//! library function but doesn't wire it to events itself.

use std::time::{Duration, Instant};

/// How long to wait for additional events before flushing a stream.
const DEFAULT_FLUSH_GAP: Duration = Duration::from_millis(80);

/// How many recent events to keep for device detection (trackpad vs wheel).
const HISTORY_CAPACITY: usize = 16;

/// Direction of a normalized scroll event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

/// Terminal brand detection. Drives the events-per-tick (EPT) correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    AppleTerminal,
    Ghostty,
    ITerm2,
    VSCode,
    Multiplexer, // tmux/screen/zellij — EPT forced to 1
    Unknown,
}

impl TerminalKind {
    /// Events per physical wheel notch for this terminal.
    /// Override via `OXI_SCROLL_EPT` env var.
    pub fn ept(self) -> u8 {
        // env override: OXI_SCROLL_EPT=1..10
        if let Ok(s) = std::env::var("OXI_SCROLL_EPT")
            && let Ok(n) = s.parse::<u8>()
            && (1..=10).contains(&n)
        {
            return n;
        }
        match self {
            TerminalKind::AppleTerminal | TerminalKind::Ghostty => 3,
            TerminalKind::ITerm2
            | TerminalKind::Multiplexer
            | TerminalKind::VSCode
            | TerminalKind::Unknown => 1,
        }
    }
}

/// Detect the current terminal from environment variables.
///
/// Detection order:
/// 1. `TMUX` / `STY` / `ZELLIJ` env → Multiplexer
/// 2. `TERM_PROGRAM` value → specific kind
/// 3. fallback: Unknown
pub fn detect_terminal() -> TerminalKind {
    if std::env::var("TMUX").is_ok()
        || std::env::var("STY").is_ok()
        || std::env::var("ZELLIJ").is_ok()
    {
        return TerminalKind::Multiplexer;
    }
    match std::env::var("TERM_PROGRAM").as_deref() {
        Ok("Apple_Terminal") => TerminalKind::AppleTerminal,
        Ok("ghostty") => TerminalKind::Ghostty,
        Ok("iTerm.app") => TerminalKind::ITerm2,
        Ok("vscode") => TerminalKind::VSCode,
        _ => TerminalKind::Unknown,
    }
}

/// Result of a flush — a single normalized scroll delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedScroll {
    /// Signed line count. Positive = down, negative = up.
    pub delta_lines: i32,
    /// Original direction (separate from sign so callers don't recompute).
    pub direction: ScrollDirection,
}

/// A wheel event waiting to be flushed.
///
/// `crossterm::event::MouseEvent` doesn't carry notch metadata, so we just
/// count consecutive same-direction events as a stream.
#[derive(Debug, Clone, Copy)]
struct PendingEvent {
    direction: ScrollDirection,
    at: Instant,
}

/// ScrollNormalizer — accumulates mouse scroll events, groups them into
/// streams, and converts to a single normalized delta on flush.
///
/// Designed to be:
/// - Cheap when idle (no allocation per push)
/// - Deterministic (same input → same output, modulo timing)
/// - Override-able via env vars (EPT, flush gap, multipliers)
#[derive(Debug)]
pub struct ScrollNormalizer {
    terminal: TerminalKind,
    flush_gap: Duration,
    history: Vec<PendingEvent>,
    current_stream: Option<PendingEvent>,
}

impl ScrollNormalizer {
    /// Create with explicit terminal kind (used by tests).
    pub fn with_terminal(terminal: TerminalKind) -> Self {
        let flush_gap = std::env::var("OXI_SCROLL_FLUSH_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_FLUSH_GAP);
        Self {
            terminal,
            flush_gap,
            history: Vec::with_capacity(HISTORY_CAPACITY),
            current_stream: None,
        }
    }

    /// Detect terminal from env and create.
    pub fn new() -> Self {
        Self::with_terminal(detect_terminal())
    }

    /// Push a raw wheel event. Returns Some(NormalizedScroll) if this
    /// event closes a stream and a delta is ready. Returns None if the
    /// event is added to the current stream (caller should call flush()
    /// later to retrieve the delta).
    pub fn push(&mut self, direction: ScrollDirection) -> Option<NormalizedScroll> {
        let now = Instant::now();
        // Decide if this is a new stream or extends the current one.
        let is_new_stream = match self.current_stream {
            None => true,
            Some(stream) => {
                stream.direction != direction || now.duration_since(stream.at) > self.flush_gap
            }
        };

        if is_new_stream {
            // Flush the previous stream (if any) before starting new.
            let flushed = self.flush_internal();
            self.current_stream = Some(PendingEvent { direction, at: now });
            // Add to history.
            self.push_history(PendingEvent { direction, at: now });
            flushed
        } else {
            // Extend current stream.
            self.current_stream = Some(PendingEvent { direction, at: now });
            self.push_history(PendingEvent { direction, at: now });
            None
        }
    }

    /// Force-flush the current stream. Returns the accumulated delta or
    /// None if there's nothing to flush.
    ///
    /// Call this from the event loop "when no new event has arrived for
    /// flush_gap" — typically via a deferred timer.
    pub fn flush(&mut self) -> Option<NormalizedScroll> {
        self.flush_internal()
    }

    fn flush_internal(&mut self) -> Option<NormalizedScroll> {
        let stream = self.current_stream.take()?;
        // Count events matching stream.direction with at <= stream.at
        // (i.e. events that "belong" to this stream).
        let events_in_stream: usize = self
            .history
            .iter()
            .rev()
            .take_while(|e| e.direction == stream.direction && e.at <= stream.at)
            .count();
        let ept = self.terminal.ept() as i32;
        // Two cases produce a delta:
        //   1. Events ≥ EPT → physical notch boundary reached, emit immediately.
        //   2. Gap-elapsed flush (caller forced flush after stream timeout)
        //      → emit even single events as a "best guess" delta.
        // A fresh stream with < EPT events and no gap doesn't emit.
        let is_gap_flush = Instant::now().duration_since(stream.at) > self.flush_gap;
        if (events_in_stream as i32) < ept && !is_gap_flush {
            // Restore the stream — caller might push more events.
            self.current_stream = Some(stream);
            return None;
        }
        let notches = ((events_in_stream as i32) + ept - 1) / ept;
        let lines_per_notch = 3;
        let signed = match stream.direction {
            ScrollDirection::Up => -(notches * lines_per_notch),
            ScrollDirection::Down => notches * lines_per_notch,
        };
        Some(NormalizedScroll {
            delta_lines: signed,
            direction: stream.direction,
        })
    }
    fn push_history(&mut self, event: PendingEvent) {
        if self.history.len() >= HISTORY_CAPACITY {
            self.history.remove(0);
        }
        self.history.push(event);
    }

    /// Total events in history (for diagnostics).
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Active terminal kind (for diagnostics).
    pub fn terminal(&self) -> TerminalKind {
        self.terminal
    }

    /// Reset all state (used after a long pause or overlay close).
    pub fn reset(&mut self) {
        self.history.clear();
        self.current_stream = None;
    }
}

impl Default for ScrollNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Env-mutating tests must run serially — they share process-global state.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Helper: lock the env mutex for serial env mutation.
    fn with_env_lock<R>(f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        f()
    }

    // ── EPT table ────────────────────────────────────────────────────────

    #[test]
    fn ept_apple_terminal_is_3() {
        assert_eq!(TerminalKind::AppleTerminal.ept(), 3);
    }

    #[test]
    fn ept_ghostty_is_3() {
        assert_eq!(TerminalKind::Ghostty.ept(), 3);
    }

    #[test]
    fn ept_iterm2_is_1() {
        assert_eq!(TerminalKind::ITerm2.ept(), 1);
    }

    #[test]
    fn ept_vscode_is_1() {
        assert_eq!(TerminalKind::VSCode.ept(), 1);
    }
    #[test]
    fn detect_term_program_ghostty() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("TMUX");
                std::env::remove_var("STY");
                std::env::remove_var("ZELLIJ");
                std::env::set_var("TERM_PROGRAM", "ghostty");
            }
            assert_eq!(detect_terminal(), TerminalKind::Ghostty);
            unsafe {
                std::env::remove_var("TERM_PROGRAM");
            }
        });
    }

    #[test]
    fn ept_unknown_is_1() {
        assert_eq!(TerminalKind::Unknown.ept(), 1);
    }

    #[test]
    fn detect_tmux_returns_multiplexer() {
        with_env_lock(|| {
            // SAFETY: env access in tests is acceptable (single-threaded test runtime).
            unsafe {
                std::env::set_var("TMUX", "/tmp/tmux-1000/default,12345,0");
                std::env::remove_var("STY");
                std::env::remove_var("ZELLIJ");
                std::env::remove_var("TERM_PROGRAM");
            }
            assert_eq!(detect_terminal(), TerminalKind::Multiplexer);
        });
    }

    #[test]
    fn detect_sty_returns_multiplexer() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("TMUX");
                std::env::set_var("STY", "12345.pts-0.host");
                std::env::remove_var("ZELLIJ");
            }
            assert_eq!(detect_terminal(), TerminalKind::Multiplexer);
        });
    }
    #[test]
    fn detect_zellij_returns_multiplexer() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("TMUX");
                std::env::remove_var("STY");
                std::env::set_var("ZELLIJ", "0");
            }
            assert_eq!(detect_terminal(), TerminalKind::Multiplexer);
        });
    }

    #[test]
    fn detect_term_program_apple_terminal() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("TMUX");
                std::env::remove_var("STY");
                std::env::remove_var("ZELLIJ");
                std::env::set_var("TERM_PROGRAM", "Apple_Terminal");
            }
            assert_eq!(detect_terminal(), TerminalKind::AppleTerminal);
        });
    }

    #[test]
    fn detect_unknown_when_no_env() {
        with_env_lock(|| {
            unsafe {
                std::env::remove_var("TMUX");
                std::env::remove_var("STY");
                std::env::remove_var("ZELLIJ");
                std::env::remove_var("TERM_PROGRAM");
            }
            assert_eq!(detect_terminal(), TerminalKind::Unknown);
        });
    }

    // ── EPT env override ─────────────────────────────────────────────────

    #[test]
    fn ept_env_override_takes_precedence() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "5");
            }
            // AppleTerminal normally is 3, but override forces 5.
            assert_eq!(TerminalKind::AppleTerminal.ept(), 5);
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
        });
    }

    #[test]
    fn ept_env_override_invalid_falls_back_to_table() {
        with_env_lock(|| {
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "0"); // invalid (out of 1..=10)
            }
            assert_eq!(TerminalKind::AppleTerminal.ept(), 3);
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "abc"); // invalid parse
            }
            assert_eq!(TerminalKind::AppleTerminal.ept(), 3);
            unsafe {
                std::env::set_var("OXI_SCROLL_EPT", "100"); // out of range
            }
            assert_eq!(TerminalKind::AppleTerminal.ept(), 3);
            unsafe {
                std::env::remove_var("OXI_SCROLL_EPT");
            }
        });
    }

    // ── Stream grouping ──────────────────────────────────────────────────

    #[test]
    fn single_event_produces_single_scroll() {
        let mut norm = ScrollNormalizer::with_terminal(TerminalKind::ITerm2);
        // Single event flushes immediately (new stream starts).
        let r = norm.push(ScrollDirection::Down);
        // First push returns None (stream just started; caller flushes later).
        assert!(r.is_none());
        // Flush → 1 event / EPT=1 = 1 notch * 3 lines = +3.
        let r = norm.flush().expect("flush should produce");
        assert_eq!(r.delta_lines, 3);
        assert_eq!(r.direction, ScrollDirection::Down);
    }

    #[test]
    fn multiple_events_same_direction_combine_into_stream() {
        let mut norm = ScrollNormalizer::with_terminal(TerminalKind::AppleTerminal);
        // AppleTerminal: 3 events per physical notch.
        // Push 3 events in quick succession → flush should yield 1 notch * 3 lines.
        assert!(norm.push(ScrollDirection::Down).is_none());
        assert!(norm.push(ScrollDirection::Down).is_none());
        assert!(norm.push(ScrollDirection::Down).is_none());
        let r = norm.flush().expect("flush should produce");
        // 3 events / 3 EPT = 1 notch, * 3 lines = 3.
        assert_eq!(r.delta_lines, 3);
    }

    #[test]
    fn direction_change_starts_new_stream() {
        let mut norm = ScrollNormalizer::with_terminal(TerminalKind::ITerm2);
        norm.push(ScrollDirection::Down);
        // Direction change → previous stream flushed automatically.
        let r = norm.push(ScrollDirection::Up);
        assert!(r.is_some(), "direction change should flush previous stream");
        assert_eq!(r.unwrap().delta_lines, 3); // down=+3
        // New stream has 1 event. iTerm2 EPT=1 → 1 event IS a notch →
        // flush emits delta immediately (no extra confirmation needed).
        let r2 = norm
            .flush()
            .expect("single iTerm2 event should produce delta");
        assert_eq!(r2.delta_lines, -3); // 1 event Up = -3 (negative for up)
    }

    #[test]
    fn flush_gap_separates_streams() {
        let mut norm = ScrollNormalizer::with_terminal(TerminalKind::ITerm2);
        norm.push(ScrollDirection::Down);
        // Simulate gap > flush_gap by sleeping briefly.
        std::thread::sleep(Duration::from_millis(120));
        let r = norm.push(ScrollDirection::Down);
        // Gap > 80ms → new stream → previous flushed.
        assert!(r.is_some(), "long gap should flush previous stream");
    }

    // ── Realistic cross-terminal scenarios ────────────────────────────────

    #[test]
    fn apple_terminal_one_notch_matches_iterm2_one_notch() {
        // Simulate one physical notch in AppleTerminal (3 events).
        let mut apple = ScrollNormalizer::with_terminal(TerminalKind::AppleTerminal);
        apple.push(ScrollDirection::Down);
        apple.push(ScrollDirection::Down);
        apple.push(ScrollDirection::Down);
        let apple_delta = apple.flush().unwrap().delta_lines;

        // Simulate one physical notch in iTerm2 (1 event).
        let mut iterm = ScrollNormalizer::with_terminal(TerminalKind::ITerm2);
        iterm.push(ScrollDirection::Down);
        let iterm_delta = iterm.flush().unwrap().delta_lines;

        // Both should produce the same delta — that's the whole point.
        assert_eq!(apple_delta, iterm_delta);
    }

    #[test]
    fn history_capped_at_16() {
        let mut norm = ScrollNormalizer::with_terminal(TerminalKind::ITerm2);
        for _ in 0..30 {
            norm.push(ScrollDirection::Down);
        }
        assert_eq!(norm.history_len(), 16);
    }

    #[test]
    fn reset_clears_state() {
        let mut norm = ScrollNormalizer::with_terminal(TerminalKind::ITerm2);
        norm.push(ScrollDirection::Down);
        norm.reset();
        assert_eq!(norm.history_len(), 0);
        assert!(norm.flush().is_none());
    }
}
