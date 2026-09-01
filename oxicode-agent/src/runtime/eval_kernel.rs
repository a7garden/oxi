//! Persistent eval kernels — the reference [`EvalKernel`] implementations
//! behind the `coding-omp-v1` "Eval kernel" extension.
//!
//! - [`PythonEvalKernel`]: one long-lived `python3 -q -u -i` process. Each
//!   cell ships inline — triple-quoted with backslash escaping, wrapped in
//!   `exec(compile(...))` — so multi-line and indented cells work verbatim;
//!   namespace state persists across cells.
//! - [`JavaScriptEvalKernel`]: one long-lived `node -i --no-warnings` (or
//!   `bun -i`) REPL. Cells are fed through stdin; top-level declarations
//!   persist in the REPL context. Cells must be complete statements (an
//!   unclosed brace would swallow the trailing marker line).
//!
//! Both delimit a cell with a stdout marker line, bound output, surface
//! interpreter diagnostics on error, and support explicit reset (child
//! killed; respawned lazily on the next cell).

use super::{EvalKernel, EvalLanguage, EvalOutput};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

const MARKER: &str = "__OXI_EVAL_END__";
const DEFAULT_MAX_OUTPUT: usize = 256 * 1024;
const STDERR_TAIL_LIMIT: usize = 16 * 1024;

struct KernelProc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Shared per-kernel session state: the live interpreter process plus a
/// bounded stderr tail used for error diagnostics.
struct KernelShared {
    proc: Mutex<Option<KernelProc>>,
    stderr_tail: Arc<Mutex<String>>,
    program: &'static str,
    args: &'static [&'static str],
}

impl std::fmt::Debug for KernelShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KernelShared")
            .field("program", &self.program)
            .field("alive", &self.proc.lock().is_some())
            .finish()
    }
}

impl KernelShared {
    fn new(program: &'static str, args: &'static [&'static str]) -> Self {
        Self {
            proc: Mutex::new(None),
            stderr_tail: Arc::new(Mutex::new(String::new())),
            program,
            args,
        }
    }

    /// Take the live interpreter out, respawning if dead or absent. The
    /// lock is never held across `.await`.
    async fn take(&self) -> Result<KernelProc, String> {
        let existing = self.proc.lock().take();
        match existing {
            Some(p) => {
                let mut p = p;
                let alive = p.child.try_wait().map_or(true, |s| s.is_none());
                if alive { Ok(p) } else { self.spawn().await }
            }
            None => self.spawn().await,
        }
    }

    async fn spawn(&self) -> Result<KernelProc, String> {
        use tokio::process::Command;
        let mut cmd = Command::new(self.program);
        cmd.args(self.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.program))?;
        let stdin = child.stdin.take().ok_or("interpreter: no stdin")?;
        let stdout = child.stdout.take().ok_or("interpreter: no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            drain_stderr(Arc::clone(&self.stderr_tail), stderr);
        }
        Ok(KernelProc {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn clear_tail(&self) {
        self.stderr_tail.lock().clear();
    }

    fn snapshot_tail(&self) -> String {
        self.stderr_tail.lock().clone()
    }
}

/// Forward interpreter stderr into the bounded shared tail. The task ends
/// when the child's stderr closes (child exit).
fn drain_stderr(tail: Arc<Mutex<String>>, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let mut buf = tail.lock();
                    if buf.len() + line.len() > STDERR_TAIL_LIMIT {
                        buf.clear();
                    }
                    buf.push_str(&line);
                    drop(buf);
                    line.clear();
                }
            }
        }
    });
}

fn last_nonempty(s: &str) -> &str {
    s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// Write the cell, read stdout until the marker. Returns
/// `(stdout, truncated, completed)`; kills the child on I/O failure.
async fn write_and_read(
    shared: &KernelShared,
    proc: &mut KernelProc,
    cell: &str,
    timeout: Duration,
    max_output: usize,
) -> Result<(String, bool, bool), String> {
    let deadline = Instant::now() + timeout;
    if let Err(e) = proc.stdin.write_all(cell.as_bytes()).await {
        let msg = format!("{} stdin write: {e}", shared.program);
        let _ = proc.child.kill().await;
        return Err(msg);
    }
    if let Err(e) = proc.stdin.flush().await {
        let msg = format!("{} stdin flush: {e}", shared.program);
        let _ = proc.child.kill().await;
        return Err(msg);
    }
    let mut stdout = String::new();
    let mut truncated = false;
    loop {
        if Instant::now() >= deadline {
            return Ok((stdout, truncated, false));
        }
        let mut line = String::new();
        let read = tokio::time::timeout_at(
            tokio::time::Instant::from(deadline),
            proc.stdout.read_line(&mut line),
        )
        .await;
        match read {
            Err(_elapsed) => return Ok((stdout, truncated, false)),
            Ok(Err(e)) => {
                let msg = format!("{} stdout read: {e}", shared.program);
                let _ = proc.child.kill().await;
                return Err(msg);
            }
            Ok(Ok(0)) => {
                let msg = format!("{} exited mid-cell", shared.program);
                let _ = proc.child.kill().await;
                return Err(msg);
            }
            Ok(Ok(_)) => {
                if line.trim_end().ends_with(MARKER) {
                    return Ok((stdout, truncated, true));
                }
                if stdout.len() + line.len() > max_output {
                    truncated = true;
                } else {
                    stdout.push_str(&line);
                }
            }
        }
    }
}

// ── Python ───────────────────────────────────────────────────────────────

/// Persistent `python3 -q -u -i` kernel.
#[derive(Debug)]
pub struct PythonEvalKernel {
    shared: KernelShared,
    max_output: usize,
}

impl Default for PythonEvalKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonEvalKernel {
    /// Creates a kernel that lazily spawns `python3 -q -u -i` on first use.
    pub fn new() -> Self {
        Self {
            shared: KernelShared::new("python3", &["-q", "-u", "-i"]),
            max_output: DEFAULT_MAX_OUTPUT,
        }
    }

    /// Override the output bound (bytes).
    pub fn with_max_output(mut self, max: usize) -> Self {
        self.max_output = max;
        self
    }
}

#[async_trait]
impl EvalKernel for PythonEvalKernel {
    fn language(&self) -> EvalLanguage {
        EvalLanguage::Python
    }

    async fn execute(&self, code: &str, timeout: Duration) -> Result<EvalOutput, String> {
        // The cell ships inline (no temp file): triple-quoted with backslash
        // escaping, so arbitrary multi-line and indented code works verbatim
        // while the interactive namespace keeps state across cells.
        let escaped = code.replace('\\', "\\\\").replace("'''", "\\'\\'\\'");
        let cell = format!(
            "exec(compile('''{escaped}'''.encode('utf-8'), 'cell', 'exec'))\nprint(\"{MARKER}\")\n"
        );
        self.shared.clear_tail();

        let mut proc = self.shared.take().await?;
        let (stdout, truncated, completed) =
            write_and_read(&self.shared, &mut proc, &cell, timeout, self.max_output).await?;
        // Settle the stderr drain so tracebacks racing the stdout marker are
        // captured for diagnostics.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let stderr = self.shared.snapshot_tail();
        if !completed {
            let _ = proc.child.kill().await;
            *self.shared.proc.lock() = None;
            return Err(if stderr.trim().is_empty() {
                "python cell timed out before completing".to_string()
            } else {
                format!("python cell failed: {}", last_nonempty(&stderr))
            });
        }
        let stderr = self.shared.snapshot_tail();
        *self.shared.proc.lock() = Some(proc);
        let error = stderr
            .contains("Traceback")
            .then(|| last_nonempty(&stderr).to_string());
        Ok(EvalOutput {
            result: String::new(),
            stdout,
            stderr,
            error,
            truncated,
        })
    }

    async fn reset(&self) -> Result<(), String> {
        let taken = self.shared.proc.lock().take();
        if let Some(mut p) = taken {
            let _ = p.child.kill().await;
        }
        Ok(())
    }
}

// ── JavaScript ───────────────────────────────────────────────────────────

/// Persistent JavaScript REPL kernel. Prefers `node`, falls back to `bun`.
#[derive(Debug)]
pub struct JavaScriptEvalKernel {
    shared: KernelShared,
    max_output: usize,
}

impl Default for JavaScriptEvalKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaScriptEvalKernel {
    /// Detects `node`, then `bun` (first found on PATH wins).
    pub fn new() -> Self {
        let (program, args): (&'static str, &'static [&'static str]) = if runtime_present("node") {
            ("node", &["-i", "--no-warnings"])
        } else {
            ("bun", &["-i"])
        };
        Self {
            shared: KernelShared::new(program, args),
            max_output: DEFAULT_MAX_OUTPUT,
        }
    }

    /// Override the output bound (bytes).
    pub fn with_max_output(mut self, max: usize) -> Self {
        self.max_output = max;
        self
    }
}

fn runtime_present(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[async_trait]
impl EvalKernel for JavaScriptEvalKernel {
    fn language(&self) -> EvalLanguage {
        EvalLanguage::JavaScript
    }
    async fn execute(&self, code: &str, timeout: Duration) -> Result<EvalOutput, String> {
        let cell = format!("{code}\nconsole.log(\"{MARKER}\");\n");
        self.shared.clear_tail();

        let mut proc = self.shared.take().await?;
        let (stdout, truncated, completed) =
            write_and_read(&self.shared, &mut proc, &cell, timeout, self.max_output).await?;
        if !completed {
            let _ = proc.child.kill().await;
            *self.shared.proc.lock() = None;
            return Err(format!(
                "js cell timed out before completing ({})",
                self.shared.program
            ));
        }
        let stderr = self.shared.snapshot_tail();
        *self.shared.proc.lock() = Some(proc);
        // Node's REPL prints uncaught errors on stdout (unlike Python's
        // stderr tracebacks), so scan both streams.
        let error = if stderr.contains("Uncaught") {
            Some(last_nonempty(&stderr).to_string())
        } else if stdout.contains("Uncaught") {
            Some(
                stdout
                    .lines()
                    .rev()
                    .find(|l| l.contains("Uncaught"))
                    .unwrap_or_default()
                    .to_string(),
            )
        } else {
            None
        };
        Ok(EvalOutput {
            result: String::new(),
            stdout,
            stderr,
            error,
            truncated,
        })
    }

    async fn reset(&self) -> Result<(), String> {
        let taken = self.shared.proc.lock().take();
        if let Some(mut p) = taken {
            let _ = p.child.kill().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn python_state_persists_and_reports_errors() {
        let kernel = PythonEvalKernel::new();
        let first = kernel
            .execute("fixture_x = 41", Duration::from_secs(15))
            .await
            .unwrap();
        assert!(first.error.is_none(), "cell 1 errored: {:?}", first.stderr);
        let out = kernel
            .execute("print(fixture_x + 1)", Duration::from_secs(15))
            .await
            .unwrap();
        assert!(out.error.is_none(), "cell 2 errored: {:?}", out.stderr);
        assert!(
            out.stdout.contains("42"),
            "state must persist: {}",
            out.stdout
        );
        let err = kernel
            .execute("raise ValueError('oxi-boom')", Duration::from_secs(15))
            .await
            .unwrap();
        assert!(
            err.error
                .as_deref()
                .unwrap_or_default()
                .contains("oxi-boom")
        );
        kernel.reset().await.unwrap();
        let gone = kernel
            .execute("print(fixture_x)", Duration::from_secs(15))
            .await
            .unwrap();
        assert!(gone.error.is_some(), "reset must clear the namespace");
    }

    #[tokio::test]
    async fn javascript_state_persists() {
        let kernel = JavaScriptEvalKernel::new();
        kernel
            .execute("globalThis.fixture_y = 41", Duration::from_secs(15))
            .await
            .unwrap();
        let out = kernel
            .execute("console.log(fixture_y + 1)", Duration::from_secs(15))
            .await
            .unwrap();
        assert!(
            out.stdout.contains("42"),
            "state must persist: {}",
            out.stdout
        );
    }
}
