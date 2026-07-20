//! Runtime XTVERSION probe (`CSI > 0 q` → `DCS > | text ST`), run when
//! env-based brand detection yields Unknown (SSH, plain xterm) or a
//! headfully-validated allowlisted brand (see [`gate_allows_probe`]).
//!
//! Fire-and-forget, parser-integrated model (as in helix and similar TUIs):
//! the query is written once at startup with no timed read; the reply is
//! recognized and swallowed by the event loop's `XtversionFilter` whenever
//! it arrives.
//!
//! Safety invariants:
//! - Query write must happen after `enable_raw_mode()` and before the
//!   `EventStream` filter is constructed.
//! - Accepted residuals: SSH *from* JediTerm still probes (its env marker
//!   doesn't cross SSH) and leaks the query there; a reply whose first
//!   event arrives only after the filter's 5s arm window types as
//!   Alt+Shift+P + literal text; on a silent terminal with a fully idle
//!   session the `OnceLock` stays unset (`record_no_reply` only runs from
//!   the filter, which only runs on input) — `detected()` is None either
//!   way, so both consumers are unaffected.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Startup probe outcome.
#[derive(Debug)]
enum ProbeResult {
    Skipped,
    NoReply,
    Identified(String),
}

/// Unset while the query is in flight (or never sent).
static XTVERSION: OnceLock<ProbeResult> = OnceLock::new();

/// True once the query bytes were written to the terminal.
static QUERY_SENT: AtomicBool = AtomicBool::new(false);

/// XTVERSION query alone — no DA1 sentinel: nothing waits on reply
/// ordering here, and a stale unsolicited DA1 reply could mis-answer a
/// future crossterm DA1-waiting probe.
#[cfg(unix)]
const QUERY: &[u8] = b"\x1b[>0q";

/// Returns the terminal's self-reported name/version, if the terminal
/// answered (e.g. `"kitty 0.35.2"`, `"foot(1.22.0)"`).
pub fn detected() -> Option<&'static str> {
    match XTVERSION.get() {
        Some(ProbeResult::Identified(v)) => Some(v),
        _ => None,
    }
}

/// True when the query was sent and no reply has been recorded yet — the
/// event loop arms its response filter on this.
pub fn reply_pending() -> bool {
    QUERY_SENT.load(Ordering::Relaxed) && XTVERSION.get().is_none()
}

/// Record the DCS payload recognized by the event-loop filter.
pub fn record_reply(payload: &str) {
    let result = match sanitize_payload(payload) {
        Some(v) => ProbeResult::Identified(v),
        None => ProbeResult::NoReply,
    };
    tracing::info!(?result, "XTVERSION probe");
    let _ = XTVERSION.set(result);
}

/// Record that the filter disarmed without seeing a reply. Only invoked
/// from the filter on input, so a fully idle session can leave the
/// `OnceLock` unset (benign — see module doc).
pub fn record_no_reply() {
    if XTVERSION.set(ProbeResult::NoReply).is_ok() {
        tracing::info!("XTVERSION probe: no reply");
    }
}

/// Send the XTVERSION query once at startup (fire-and-forget); no-ops when
/// the gate rejects the brand/multiplexer or stdin is not a TTY.
pub fn probe_at_startup() {
    use std::io::IsTerminal;

    if XTVERSION.get().is_some() || QUERY_SENT.load(Ordering::Relaxed) {
        return;
    }
    let ctx = super::terminal_context();
    if !gate_allows_probe(ctx) || !std::io::stdin().is_terminal() {
        let _ = XTVERSION.set(ProbeResult::Skipped);
        return;
    }
    send_query();
}

/// Crush-style brand allowlist: Unknown plus brands headfully validated as
/// clean XTVERSION responders (version fidelity is the payoff there).
/// CSI-intercepting multiplexers skip — the innermost layer answers as
/// itself, which the `multiplexer` field already records. Transparent muxes
/// (e.g. cmux) need no special case.
fn gate_allows_probe(ctx: &super::TerminalContext) -> bool {
    use super::TerminalName::*;
    matches!(
        ctx.brand,
        Unknown | Kitty | WezTerm | Ghostty | Iterm2 | Rio
    ) && !ctx.multiplexer.intercepts_csi_queries()
}

#[cfg(unix)]
fn send_query() {
    if super::probe::write_query(QUERY) {
        QUERY_SENT.store(true, Ordering::Relaxed);
    } else {
        // Brand-Unknown TTY whose query can't reach the terminal is a
        // feedback-triage signal worth tracing.
        tracing::debug!("XTVERSION probe skipped: query write failed or output is not a TTY");
        let _ = XTVERSION.set(ProbeResult::Skipped);
    }
}

#[cfg(not(unix))]
fn send_query() {
    // ConPTY does not implement XTVERSION.
    let _ = XTVERSION.set(ProbeResult::Skipped);
}

/// Strip controls and trim; `None` for an empty payload.
fn sanitize_payload(payload: &str) -> Option<String> {
    let cleaned: String = payload.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().to_owned();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
