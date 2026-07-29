//! PTY-based e2e test scenarios for oxi-cli.
//!
//! These tests spawn the actual `oxi` binary in a PTY and verify
//! the byte-level terminal output. They complement the unit tests
//! in oxi-tui (which use ratatui's TestBackend).
//!
//! Run with: cargo nextest run -p oxi-cli --test pty_e2e

mod pty_harness;

use std::time::Duration;

use pty_harness::{PtySession, assert_output_contains, oxi_binary_available};

/// Boot the oxi binary, verify it starts up and emits recognizable UI output.
///
/// Skips if the oxi binary is not built or not in PATH.
#[test]
fn test_pty_minimal_boot() {
    if !oxi_binary_available() {
        eprintln!("skipping: oxi binary not in PATH");
        return;
    }

    let mut session = match PtySession::spawn(&["--version"]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: failed to spawn oxi: {e}");
            return;
        }
    };

    // --version should print and exit. Read for the version prefix.
    let output = session
        .read_until("oxi", Duration::from_secs(5))
        .expect("read should not error");

    // The --version output should contain "oxi" and a version number pattern.
    assert_output_contains(&output, "oxi");

    // The process should exit cleanly within 5 seconds.
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(code)) = session.try_wait() {
            assert_eq!(code, 0, "oxi --version should exit 0");
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            // Force kill and fail.
            let _ = session.kill();
            panic!("oxi --version did not exit within 5 seconds");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Boot the oxi TUI in a PTY and verify the production tape path.
///
/// Ordinary chat renders on the main screen: startup must hide the cursor and
/// produce tape output without entering alternate screen. Alternate screen is
/// reserved for transient overlays.
#[test]
fn test_pty_tui_renders_and_exits() {
    if !oxi_binary_available() {
        eprintln!("skipping: oxi binary not in PATH");
        return;
    }

    // `-i` forces interactive TUI mode. No prompt needed.
    let mut session = match PtySession::spawn(&["-i"]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: failed to spawn oxi: {e}");
            return;
        }
    };

    // Main-screen tape emits cursor-hide and synchronized output, but never
    // alternate-screen enter during ordinary startup.
    let output = session
        .read_until("\x1b[?2026l", Duration::from_secs(5))
        .expect("read should not error");

    if !output.contains("\x1b[?2026l") {
        let _ = session.kill();
        panic!("TUI did not paint a synchronized tape frame within 5s. Output:\n{output}");
    }
    assert_output_contains(&output, "\x1b[?25l");
    assert!(
        !output.contains("\x1b[?1049h"),
        "ordinary tape rendering must stay on main screen"
    );

    // Send Ctrl+C twice: first cancels, second quits.
    session.send_raw(&[0x03]).expect("send first ctrl-c");
    std::thread::sleep(Duration::from_millis(200));
    session.send_raw(&[0x03]).expect("send second ctrl-c");

    // Wait for clean exit.
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(_code)) = session.try_wait() {
            // Exit code may be non-zero (sigint), that's fine.
            // The key assertion is that it DID exit, not that it exited 0.
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = session.kill();
            panic!("TUI did not exit within 5s of Ctrl+C — TUI is hung");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let final_output = session
        .read_until("\x1b[?25h", Duration::from_secs(2))
        .unwrap_or_default();
    let all_output = format!("{output}\n{final_output}");
    assert!(all_output.contains("\x1b[?25h"), "exit must restore cursor");
    assert!(!all_output.contains("\x1b[?1049h"));
}

/// Verify the PTY harness itself can spawn any binary and read its output.
///
/// This is a smoke test for the harness — it doesn't depend on oxi.
#[test]
fn test_pty_harness_spawns_echo() {
    // Use echo via portable-pty's lower-level API directly (since PtySession::spawn
    // hardcodes "oxi"). This validates the PTY plumbing itself.
    let pty_system = portable_pty::native_pty_system();
    let pty_pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = portable_pty::CommandBuilder::new("echo");
    cmd.arg("hello-pty");

    let mut child = pty_pair.slave.spawn_command(cmd).expect("spawn echo");

    let mut reader = pty_pair
        .master
        .try_clone_reader()
        .expect("try_clone_reader");
    drop(pty_pair.slave);

    let mut buf = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut byte_buf = [0u8; 1024];

    while std::time::Instant::now() < deadline {
        if let Ok(n) = reader.read(&mut byte_buf)
            && n > 0
        {
            buf.push_str(&String::from_utf8_lossy(&byte_buf[..n]));
        }
        if buf.contains("hello-pty") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_output_contains(&buf, "hello-pty");

    // Wait for child to exit.
    let _ = child.wait();
}

/// Open the Agent Hub overlay via `/agents`, verify the header, and close with `q`.
#[test]
fn test_pty_hub_opens_and_closes() {
    if !oxi_binary_available() {
        eprintln!("skipping: oxi binary not in PATH");
        return;
    }

    let mut session = match PtySession::spawn(&["-i"]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: failed to spawn oxi: {e}");
            return;
        }
    };

    // Wait for TUI to paint its first frame.
    session
        .read_until("\x1b[?2026l", Duration::from_secs(5))
        .expect("read should not error");

    // Send /agents to open the hub overlay.
    session.send_line("/agents").expect("send /agents");

    let hub_output = session
        .read_until("Agent Hub", Duration::from_secs(3))
        .expect("read should not error");

    // The hub table header must appear in the output.
    assert_output_contains(&hub_output, "Agent Hub");

    // Close with q, then send Ctrl+C twice to exit.
    session.send_raw(b"q").expect("send q to close hub");
    std::thread::sleep(Duration::from_millis(200));
    session.send_raw(&[0x03]).expect("send first ctrl-c");
    std::thread::sleep(Duration::from_millis(200));
    session.send_raw(&[0x03]).expect("send second ctrl-c");

    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(_code)) = session.try_wait() {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = session.kill();
            panic!("TUI did not exit within 5s after hub test");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
