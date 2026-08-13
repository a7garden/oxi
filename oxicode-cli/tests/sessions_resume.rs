//! End-to-end smoke for the `/sessions <id>` resume flow.
//!
//! Spawns the `oxicode` binary in a real PTY, types `/sessions <id>`,
//! then an empty Enter (which drains `pending_resume` on the next
//! Submit), and asserts:
//!
//! * the TUI paints a synchronized tape frame,
//! * the slash command prints `Resuming ...` synchronously (proves
//!   the direct-resume path did NOT open the picker),
//! * the resume worker fires and prints `Resumed session ...`, and
//! * the picker subtitle `Select a session` does NOT reappear.
//!
//! The stub session file is written into `~/.oxicode/sessions/` using
//! the crate's real `SessionHeader` / `FileEntry` types — the JSONL
//! shape is then guaranteed to round-trip through the real loader.
//! Written once per fixed id; if the file already exists it is left
//! alone (idempotent — never rewrites a user's existing session).
//!
//! Skips cleanly when the `oxicode` binary is not in PATH (the
//! `PtySession` harness's existing skip guard). Best-effort: when the
//! binary IS available, the assertions still rely on TTY timing; in
//! resource-constrained CI the test may flake and fall back to the
//! 908 unit tests for the same coverage.
//!
//! Run with:
//!
//! ```text
//! cargo nextest run -p oxicode-cli --no-fail-fast -- sessions_resume
//! ```
//!
//! or, if the binary isn't on PATH:
//!
//! ```text
//! PATH="$(pwd)/target/debug:$PATH" cargo nextest run -p oxicode-cli --no-fail-fast -- sessions_resume
//! ```
//!
//! IMPORTANT: the harness's `read_until` returns `Ok(buf)` whether or
//! not the pattern was found — it never returns `Err` (deadline,
//! pattern match, and child disconnect all funnel through `Ok`). So
//! every positive check MUST test `buf.contains(needle)` explicitly
//! to avoid passing vacuously.

mod pty_harness;

use std::time::Duration;

use pty_harness::PtySession;

/// Fixed id so the stub is deterministic and idempotent.
const STUB_SESSION_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Read PTY output until `needle` appears, then verify the buffer
/// actually contains it. Returns the buffer on success; on a missing
/// needle the PTY is killed and the function panics with the buffer
/// for diagnosis.
fn read_until_contains(
    pty: &mut PtySession,
    needle: &str,
    timeout: Duration,
    what: &str,
) -> String {
    let buf = pty.read_until(needle, timeout).unwrap_or_default();
    if buf.contains(needle) {
        buf
    } else {
        let _ = pty.kill();
        panic!("expected {what:?} within {timeout:?}; buffer:\n---\n{buf}\n---");
    }
}

/// Write a hermetic stub session file at `~/.oxicode/sessions/{id}.jsonl`
/// using the crate's real types — the JSONL form is then guaranteed to
/// round-trip through `SessionManager::open` and the resume worker.
///
/// The file is only written if absent. If a real session already lives
/// at that path, it is left untouched (we never rewrite a user's
/// session). The stub uses the user's home directory as `cwd` so the
/// resume loader's `assert_session_cwd_exists` check passes.
///
/// NOTE: the brief's draft used `FileEntry::Entry(SessionEntry)` — that
/// does NOT type-check. The real `FileEntry::Entry` wraps
/// `SessionEntryEnum`; we use `SessionEntryEnum::Message(...)` so the
/// serialized form matches what `SessionManager` writes.
fn write_stub_session() {
    use oxicode::store::session::{
        AgentMessage, ContentValue, FileEntry, SessionEntryBase, SessionEntryEnum, SessionHeader,
        SessionMessageEntry,
    };

    let dir = dirs::home_dir()
        .expect("home dir must be resolvable for stub write")
        .join(".oxicode")
        .join("sessions");
    let file = dir.join(format!("{STUB_SESSION_ID}.jsonl"));

    if file.exists() {
        // Idempotent: never rewrite an existing file (a real session
        // could already live here from a previous test run).
        return;
    }

    std::fs::create_dir_all(&dir).expect("create sessions dir");

    let cwd = dirs::home_dir()
        .expect("home dir must be resolvable for stub cwd")
        .to_string_lossy()
        .to_string();

    let header = SessionHeader::new(STUB_SESSION_ID.to_string(), cwd, None);

    let entry = SessionEntryEnum::Message(SessionMessageEntry {
        base: SessionEntryBase {
            entry_type: "message".to_string(),
            id: "stub-entry-1".to_string(),
            parent_id: None,
            // RFC3339 string, matching the on-disk format.
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
        message: AgentMessage::User {
            content: ContentValue::String("stub prior turn".to_string()),
        },
    });

    let mut s = String::new();
    s.push_str(&serde_json::to_string(&FileEntry::Header(header)).unwrap());
    s.push('\n');
    s.push_str(&serde_json::to_string(&FileEntry::Entry(entry)).unwrap());
    s.push('\n');

    std::fs::write(&file, s).expect("write stub session");
}

/// Drives the full `/sessions <id>` resume flow over a real PTY:
///
/// 1. Writes the hermetic stub (idempotent — see `write_stub_session`).
/// 2. Spawns the TUI in interactive mode (`-i`).
/// 3. Waits for the tape frame marker `\x1b[?2026l` (positive check).
/// 4. Sends `/sessions <id>` and reads `Resum` — the slash command's
///    synchronous `Resuming ...` reply (positive check). Reading this
///    BEFORE the picker could open proves the direct-resume path took
///    the fast lane (i.e. the old dead code that always opened the
///    picker is gone).
/// 5. Sends an empty Enter — the Submit arm drains `pending_resume`
///    and spawns the resume worker.
/// 6. Reads `Resumed session` — the resume worker's success line
///    (positive check).
/// 7. Negative-checks `Select a session` with a short timeout. A
///    timeout / empty buffer is the PASS condition; if the picker had
///    reopened, the subtitle would already be in the PTY buffer and
///    the contains() test would be true.
///
/// On any assertion failure the PTY is killed so the test process
/// exits cleanly.
#[test]
fn sessions_direct_resume_does_not_reopen_picker() {
    if !pty_harness::oxicode_binary_available() {
        eprintln!("oxicode binary not in PATH; skipping (build with `cargo build -p oxicode-cli`)");
        return;
    }

    write_stub_session();

    let mut pty = match PtySession::spawn(&["-i"]) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: failed to spawn oxicode: {e}");
            return;
        }
    };

    // (3) Wait for the TUI to paint its first frame.
    read_until_contains(
        &mut pty,
        "\x1b[?2026l",
        Duration::from_secs(10),
        "TUI synchronized tape frame marker",
    );

    // (4) Type `/sessions <id>` — the slash command replies
    //     `Resuming ...` synchronously and stashes the path in
    //     `pending_resume`. We read `Resum` (a strict prefix of the
    //     reply) — the picker title would also start with `S`, so
    //     `Resum` is the minimal needle that pins the synchronous
    //     reply without depending on the full Unicode `…` glyph.
    pty.send_line(&format!("/sessions {STUB_SESSION_ID}"))
        .expect("failed to send /sessions command");
    read_until_contains(
        &mut pty,
        "Resum",
        Duration::from_secs(10),
        "synchronous 'Resuming ...' reply from /sessions",
    );

    // (5) Send an empty Enter: the Submit arm drains `pending_resume`
    //     and spawns the resume worker.
    pty.send_line("").expect("failed to send empty enter");

    // (6) The resume worker prints `Resumed session <id> (<n> messages)`.
    read_until_contains(
        &mut pty,
        "Resumed session",
        Duration::from_secs(10),
        "resume worker 'Resumed session ...' line",
    );

    // (7) Negative check: the picker must NOT have reopened.
    //     `Select a session` is the picker's subtitle.
    let buf = pty
        .read_until("Select a session", Duration::from_millis(300))
        .unwrap_or_default();
    if buf.contains("Select a session") {
        let _ = pty.kill();
        panic!("picker reopened — got: {buf}");
    }

    // (8) Clean up.
    let _ = pty.kill();
}
