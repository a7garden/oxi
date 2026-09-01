//! Persistent shell session — the reference [`ShellSession`] implementation
//! behind the `coding-omp-v1` "Shell session" extension.
//!
//! Protocol: one long-lived `bash --noprofile --norc` child with piped
//! stdio. Each [`ShellSession::execute`] writes the command followed by an
//! exit-code marker line and reads stdout until the marker, so the working
//! directory and exported environment persist across calls.
//!
//! Cancellation: the child runs in its own process group and installs a
//! no-op `trap : INT`. [`ShellSession::cancel`] SIGINTs the whole group —
//! bash swallows the signal (trap) and survives to run the marker line,
//! while the foreground command inherits the DEFAULT disposition and dies,
//! surfacing as exit code 130. Output is bounded; the bound is reported
//! via `ShellOutput::truncated`.
//!
//! Known edge (documented, same class as OMP): a command that consumes
//! stdin itself will swallow the marker line — such commands should read
//! from files/args, not the session's stdin.

use super::ShellOutput;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

const MARKER: &str = "__OXI_SH_DONE__";
const DEFAULT_MAX_OUTPUT: usize = 512 * 1024;

struct ShellProc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Whether the session's `trap : INT` init line has been sent.
    initialized: bool,
}

/// Persistent bash session. Executes are serialized by an internal lock,
/// matching a single terminal.
pub struct PersistentShellSession {
    workspace_root: PathBuf,
    max_output: usize,
    proc: Mutex<Option<ShellProc>>,
    /// Process-group id of the bash child while a command runs (0 = idle).
    /// Tracked separately because the child handle is checked out of
    /// [`Self::proc`] for the duration of an execute.
    active_pgid: AtomicU64,
}

impl std::fmt::Debug for PersistentShellSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentShellSession")
            .field("workspace_root", &self.workspace_root)
            .field("alive", &self.proc.lock().is_some())
            .finish()
    }
}

impl PersistentShellSession {
    /// Session rooted at `workspace_root` (the reset working directory).
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            max_output: DEFAULT_MAX_OUTPUT,
            proc: Mutex::new(None),
            active_pgid: AtomicU64::new(0),
        }
    }

    /// Override the output bound (bytes).
    pub fn with_max_output(mut self, max: usize) -> Self {
        self.max_output = max;
        self
    }

    fn spawn(&self) -> std::io::Result<ShellProc> {
        use tokio::process::Command;
        let mut cmd = Command::new("bash");
        cmd.args(["--noprofile", "--norc"])
            .current_dir(&self.workspace_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            // Own process group so cancel() can SIGINT the foreground command.
            cmd.process_group(0);
        }
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no stdout"))?;
        // Stderr is drained (bounded) and discarded — commands that need it
        // redirect explicitly.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                let mut kept: usize = 0;
                loop {
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            kept = kept.saturating_add(line.len());
                            line.clear();
                            if kept >= DEFAULT_MAX_OUTPUT {
                                break; // stop draining; pipe backpressure accepted
                            }
                        }
                    }
                }
            });
        }
        Ok(ShellProc {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            initialized: false,
        })
    }

    /// Take the live proc out (guard never held across `.await`).
    fn take_proc(&self) -> std::io::Result<ShellProc> {
        let existing = self.proc.lock().take();
        match existing {
            Some(mut p) => {
                let alive = p.child.try_wait().map_or(true, |s| s.is_none());
                if alive { Ok(p) } else { self.spawn() }
            }
            None => self.spawn(),
        }
    }

    fn put_proc(&self, proc: ShellProc) {
        self.active_pgid.store(0, Ordering::SeqCst);
        *self.proc.lock() = Some(proc);
    }
}

/// SIGINT the bash process group to abort the foreground command; the
/// `trap : INT` init makes bash itself survive so the marker line still
/// runs. No-op when idle.
fn interrupt_active(pgid: u64) {
    #[cfg(unix)]
    if pgid != 0 {
        // process_group(0) made the child its own group leader, so a
        // negative pid targets the whole group (bash + the foreground
        // command).
        unsafe {
            libc::kill(-(pgid as i32), libc::SIGINT);
        }
    }
}

#[async_trait]
impl super::ShellSession for PersistentShellSession {
    async fn execute(&self, command: &str, timeout: Duration) -> Result<ShellOutput, String> {
        let deadline = Instant::now() + timeout;
        let mut proc = self.take_proc().map_err(|e| format!("spawn bash: {e}"))?;
        if !proc.initialized {
            // No-op INT trap: bash survives group SIGINT (so the marker
            // runs) while child commands inherit the DEFAULT disposition
            // and die with exit code 130.
            proc.stdin
                .write_all(b"trap : INT\n")
                .await
                .map_err(|e| format!("bash init write: {e}"))?;
            proc.stdin
                .flush()
                .await
                .map_err(|e| format!("bash init flush: {e}"))?;
            proc.initialized = true;
        }
        self.active_pgid
            .store(proc.child.id().unwrap_or(0) as u64, Ordering::SeqCst);

        let payload = format!("{command}\nprintf '%s\\n' \"{MARKER}$?\"\n");
        if let Err(e) = proc.stdin.write_all(payload.as_bytes()).await {
            self.active_pgid.store(0, Ordering::SeqCst);
            let msg = format!("bash stdin write: {e}");
            let _ = proc.child.kill().await;
            return Err(msg);
        }
        if let Err(e) = proc.stdin.flush().await {
            self.active_pgid.store(0, Ordering::SeqCst);
            let msg = format!("bash stdin flush: {e}");
            let _ = proc.child.kill().await;
            return Err(msg);
        }

        let mut stdout = String::new();
        let mut truncated = false;
        let mut exit_code: Option<i32> = None;
        loop {
            if Instant::now() >= deadline {
                interrupt_active(self.active_pgid.load(Ordering::SeqCst));
                break;
            }
            let mut line = String::new();
            let read = tokio::time::timeout_at(
                tokio::time::Instant::from(deadline),
                proc.stdout.read_line(&mut line),
            )
            .await;
            match read {
                Err(_elapsed) => {
                    interrupt_active(self.active_pgid.load(Ordering::SeqCst));
                    break;
                }
                Ok(Err(e)) => {
                    let msg = format!("bash stdout read: {e}");
                    let _ = proc.child.kill().await;
                    return Err(msg);
                }
                Ok(Ok(0)) => {
                    let msg = "bash exited before the command completed".to_string();
                    let _ = proc.child.kill().await;
                    return Err(msg);
                }
                Ok(Ok(_)) => {
                    if let Some(rest) = line.trim_end().strip_prefix(MARKER) {
                        exit_code = rest.trim().parse::<i32>().ok();
                        break;
                    }
                    if stdout.len() + line.len() > self.max_output {
                        truncated = true;
                    } else {
                        stdout.push_str(&line);
                    }
                }
            }
        }
        self.put_proc(proc);
        Ok(ShellOutput {
            stdout,
            stderr: String::new(),
            exit_code: exit_code.unwrap_or(124),
            truncated: truncated || exit_code.is_none(),
        })
    }

    fn cancel(&self) {
        interrupt_active(self.active_pgid.load(Ordering::SeqCst));
    }

    async fn reset(&self) -> Result<(), String> {
        let taken = self.proc.lock().take();
        if let Some(mut p) = taken {
            let _ = p.child.kill().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShellSession as _;
    use std::sync::Arc;

    #[tokio::test]
    async fn cwd_and_env_persist() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let session = PersistentShellSession::new(dir.path().to_path_buf());
        let out = session
            .execute("cd sub && export OXI_FIXTURE=1", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        let out = session
            .execute("echo \"$PWD $OXI_FIXTURE\"", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(out.stdout.contains("sub"), "cwd must persist: {out:?}");
        assert!(
            out.stdout.trim_end().ends_with(" 1"),
            "env must persist: {out:?}"
        );
    }

    #[tokio::test]
    async fn output_bound_reports_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let session = PersistentShellSession::new(dir.path().to_path_buf()).with_max_output(4_096);
        // seq emits newline-terminated lines so the reader keeps flowing
        // past the bound and still sees the marker (fast, no deadline hit).
        let out = session
            .execute("seq 1 200000", Duration::from_secs(10))
            .await
            .unwrap();
        assert!(out.truncated);
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.len() <= 4_096 + 8);
    }

    #[tokio::test]
    async fn reset_returns_to_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let session = PersistentShellSession::new(dir.path().to_path_buf());
        session
            .execute("cd sub", Duration::from_secs(5))
            .await
            .unwrap();
        session.reset().await.unwrap();
        let out = session
            .execute("pwd", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(
            !out.stdout.contains("sub"),
            "reset must restore root: {out:?}"
        );
    }

    #[tokio::test]
    async fn cancel_aborts_long_command() {
        let dir = tempfile::tempdir().unwrap();
        let session = Arc::new(PersistentShellSession::new(dir.path().to_path_buf()));
        let worker = {
            let session = session.clone();
            tokio::spawn(async move { session.execute("sleep 30", Duration::from_secs(60)).await })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        session.cancel();
        let started = Instant::now();
        // SIGINT aborts the foreground `sleep`; the `trap : INT` init keeps
        // bash alive so the marker line runs and reports 130.
        let out = tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("execute must return after cancel")
            .expect("join")
            .expect("execute ok");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "cancel must be prompt"
        );
        assert_eq!(out.exit_code, 130, "SIGINT must surface as 130: {out:?}");
    }
}
