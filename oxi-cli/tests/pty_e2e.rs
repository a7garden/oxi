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

    let mut child = pty_pair
        .slave
        .spawn_command(cmd)
        .expect("spawn echo");

    let mut reader = pty_pair.master.try_clone_reader().expect("try_clone_reader");
    drop(pty_pair.slave);

    let mut buf = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut byte_buf = [0u8; 1024];

    while std::time::Instant::now() < deadline {
        if let Ok(n) = reader.read(&mut byte_buf) {
            if n > 0 {
                buf.push_str(&String::from_utf8_lossy(&byte_buf[..n]));
            }
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
