//! PTY-based e2e test harness for oxicode-cli.
//!
//! Spawns the `oxicode` binary in a real PTY and reads/writes through it.
//! Tests using this harness can verify actual ANSI byte output, escape
//! sequences (OSC8, CSI 2026 sync, etc.), and interaction patterns
//! that ratatui's TestBackend cannot exercise.
//!
//! ## Architecture
//!
//! `PtySession` stores only the `master` (not the full `PtyPair`) to ensure
//! the slave fd is dropped — otherwise the child never receives SIGHUP when
//! the test exits.
//!
//! Reads happen on a background thread feeding an `mpsc::Receiver`, so
//! `read_until()` can poll with timeouts without blocking on the PTY reader
//! (which is inherently blocking I/O).
//!
//! This file is a helper module — actual test scenarios live in `pty_e2e.rs`.

use std::io::{self, Read, Write};
use std::process::Command;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// A spawned oxicode process in its own PTY.
/// A spawned oxicode process in its own PTY.
#[allow(dead_code)]
pub struct PtySession {
    // Store only master (not the full PtyPair). Dropping the slave fd in
    // spawn() ensures the child gets SIGHUP when we exit.
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn Write + Send>,
    // Background reader thread feeds this channel; read_until polls it.
    reader_rx: mpsc::Receiver<Vec<u8>>,
}

#[allow(dead_code)]
impl PtySession {}

impl PtySession {
    /// Spawn the `oxicode` binary with the given args in a new PTY.
    ///
    /// Assumes the `oxicode` binary is in PATH. Tests should call
    /// `oxicode_binary_available()` first and skip if false.
    pub fn spawn(args: &[&str]) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(pty_err_to_io)?;

        let mut cmd = CommandBuilder::new("oxicode");
        cmd.args(args);
        // Disable config file loading to isolate tests from user config.
        cmd.env("OXICODE_NO_USER_CONFIG", "1");
        cmd.cwd(".");

        let child = pty_pair.slave.spawn_command(cmd).map_err(pty_err_to_io)?;

        let reader = pty_pair.master.try_clone_reader().map_err(pty_err_to_io)?;
        let writer = pty_pair.master.take_writer().map_err(pty_err_to_io)?;

        // CRITICAL: drop the slave fd so the child receives SIGHUP when
        // the master closes. If we kept the full PtyPair, the slave fd
        // would stay alive and the child would never see the parent exit.
        // Also, `Box<dyn Slave + Send>` does not implement Clone, so we
        // can't keep a copy anyway.
        drop(pty_pair.slave);

        // Background thread: blocking read from PTY → mpsc channel.
        // This lets `read_until` poll with timeouts instead of blocking.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            // Receiver dropped — test is done.
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            master: pty_pair.master,
            child,
            writer,
            reader_rx: rx,
        })
    }

    /// Read PTY output until `pattern` appears or `timeout` elapses.
    ///
    /// Returns the accumulated output (which may or may not contain the
    /// pattern — caller should check). Uses a background reader thread so
    /// this method never blocks on the PTY itself.
    pub fn read_until(&mut self, pattern: &str, timeout: Duration) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        let mut buf = String::new();
        // Terminal emulation: answer cursor-position reports (CSI 6n).
        // The inline viewport queries the cursor at startup; a harness
        // that never replies makes crossterm time out and the TUI
        // degrade to fullscreen.
        let mut answered_queries = 0usize;

        loop {
            match self.reader_rx.try_recv() {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    let queries = buf.matches("\x1b[6n").count();
                    if queries > answered_queries {
                        for _ in answered_queries..queries {
                            self.writer.write_all(b"\x1b[24;1R")?;
                        }
                        self.writer.flush()?;
                        answered_queries = queries;
                    }
                    if buf.contains(pattern) {
                        return Ok(buf);
                    }
                }
                Err(TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return Ok(buf);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(TryRecvError::Disconnected) => {
                    // Reader thread exited (child closed stdout).
                    return Ok(buf);
                }
            }
        }
    }

    /// Write text followed by Enter to the PTY.
    #[allow(dead_code)]
    pub fn send_line(&mut self, text: &str) -> io::Result<()> {
        self.writer.write_all(text.as_bytes())?;
        self.writer.write_all(b"\r")?; // PTY expects \r, not \n
        self.writer.flush()
    }

    #[allow(dead_code)]
    pub fn send_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    #[allow(dead_code)]
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(pty_err_to_io)
    }

    /// Check if the child process has exited. Returns `Some(code)` if exited.
    /// Exit code is `u32` (matches portable_pty::ExitStatus::exit_code).
    #[allow(dead_code)] // used by some test binaries (pty_e2e) but not others (sessions_resume)
    pub fn try_wait(&mut self) -> io::Result<Option<u32>> {
        self.child
            .try_wait()
            .map(|opt| opt.map(|status| status.exit_code()))
    }

    /// Kill the child process if still running.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pty_err_to_io(e: anyhow::Error) -> io::Error {
    io::Error::other(e)
}

/// Assert that `haystack` contains `needle`. Panics with both strings on miss.
#[track_caller]
pub fn assert_output_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected PTY output to contain {:?}\n\nactual output:\n---\n{}\n---",
        needle,
        haystack
    );
}

/// Check if the `oxicode` binary is available (built and in PATH).
pub fn oxicode_binary_available() -> bool {
    Command::new("oxicode")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_output_contains_pass() {
        // Does not panic when needle is present.
        assert_output_contains("hello world", "hello");
    }

    #[test]
    #[should_panic(expected = "expected PTY output to contain")]
    fn test_assert_output_contains_fail() {
        assert_output_contains("hello world", "missing");
    }

    // Note: spawn() tests live in pty_e2e.rs, not here — they need the
    // actual `oxicode` binary which may not be in PATH during unit test runs.
}
