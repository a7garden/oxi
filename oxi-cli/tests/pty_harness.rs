//! PTY-based e2e test harness for oxi-cli.
//!
//! Spawns the `oxi` binary in a real PTY and reads/writes through it.
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

use portable_pty::{native_pty_system, CommandBuilder, Master, PtySize};

/// A spawned oxi process in its own PTY.
pub struct PtySession {
    // Store only master (not the full PtyPair). Dropping the slave fd in
    // spawn() ensures the child gets SIGHUP when we exit.
    master: Box<dyn Master + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn Write + Send>,
    // Background reader thread feeds this channel; read_until polls it.
    reader_rx: mpsc::Receiver<Vec<u8>>,
}

impl PtySession {
    /// Spawn the `oxi` binary with the given args in a new PTY.
    ///
    /// Assumes the `oxi` binary is in PATH. Tests should call
    /// `oxi_binary_available()` first and skip if false.
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

        let mut cmd = CommandBuilder::new("oxi");
        cmd.args(args);
        // Disable config file loading to isolate tests from user config.
        cmd.env("OXI_NO_USER_CONFIG", "1");
        cmd.cwd(".");

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(pty_err_to_io)?;

        let reader = pty_pair
            .master
            .take_reader()
            .map_err(pty_err_to_io)?;
        let writer = pty_pair
            .master
            .take_writer()
            .map_err(pty_err_to_io)?;

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

        loop {
            match self.reader_rx.try_recv() {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
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
    pub fn send_line(&mut self, text: &str) -> io::Result<()> {
        self.writer.write_all(text.as_bytes())?;
        self.writer.write_all(b"\r")?; // PTY expects \r, not \n
        self.writer.flush()
    }

    /// Write raw bytes (for sending control sequences like Ctrl+C).
    pub fn send_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the PTY (sends SIGWINCH on unix).
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
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        self.child
            .try_wait()
            .map(|opt| opt.map(|status| status.exit_code()))
            .map_err(pty_err_to_io)
    }

    /// Kill the child process if still running.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill().map_err(pty_err_to_io)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pty_err_to_io(e: anyhow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
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

/// Check if the `oxi` binary is available (built and in PATH).
pub fn oxi_binary_available() -> bool {
    Command::new("oxi")
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
    // actual `oxi` binary which may not be in PATH during unit test runs.
}
