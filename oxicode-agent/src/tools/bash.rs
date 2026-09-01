/// Bash tool - execute shell commands
/// Features:
/// - Timeout support with process group kill
/// - Working directory (cwd) parameter
/// - Environment variables support
/// - Duration timing reporting
/// - Output truncation (2000 lines / 50KB defaults via truncate module)
/// - Separate stdout/stderr capture combined at end
/// - Process tree kill on abort/cancel via signal
use super::truncate::{self, TruncationOptions, TruncationResult};
use super::{AgentTool, AgentToolResult, ProgressCallback, ToolContext, ToolError};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;

/// Environment variables that are blocked from injection via the LLM.
/// These can be used for privilege escalation, library injection, or path manipulation.
const BLOCKED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "PATH",
    "HOME",
    "IFS",
    "SHELL",
    "USER",
    "LOGNAME",
    "PYTHONPATH",
    "NODE_PATH",
    "RUBYLIB",
    "PERL5LIB",
    "CLASSPATH",
    "JAVA_TOOL_OPTIONS",
    "MallocNanoZone",
    "MallocSpaceEfficient",
];

/// Check if a command contains dangerous patterns.
/// Returns a warning string if dangerous patterns are detected, or None if safe.
/// This does NOT block execution - it only emits a warning.
pub(crate) fn is_dangerous_command(command: &str) -> Option<String> {
    let cmd_lower = command.to_lowercase();
    let mut warnings: Vec<String> = Vec::new();

    // Pipe to shell
    if cmd_lower.contains("| sh") || cmd_lower.contains("| bash") || cmd_lower.contains("| zsh") {
        warnings.push("pipe to shell".to_string());
    }

    // Sensitive file access via command substitution
    if command.contains("/etc/passwd") || command.contains("/etc/shadow") {
        warnings.push("access to sensitive authentication files".to_string());
    }
    if command.contains("id_rsa") || command.contains("id_ed25519") || command.contains(".ssh/") {
        warnings.push("access to SSH private keys/directory".to_string());
    }

    // Network exfiltration patterns
    if (cmd_lower.contains("curl") || cmd_lower.contains("wget")) && cmd_lower.contains("| nc") {
        warnings.push("possible network exfiltration (pipe to netcat)".to_string());
    }
    if command.contains("/dev/tcp/") || command.contains("/dev/udp/") {
        warnings.push("possible network exfiltration via /dev/tcp|udp".to_string());
    }

    // Privilege escalation
    if cmd_lower.starts_with("sudo ")
        || cmd_lower.contains("\nsudo ")
        || cmd_lower.contains("&&sudo ")
    {
        warnings.push("sudo detected (privilege escalation)".to_string());
    }
    if cmd_lower.contains("su -") || cmd_lower.contains("su root") {
        warnings.push("user switch to privileged account".to_string());
    }

    // Fork bomb patterns
    if cmd_lower.contains(":(){ :|:& };") || cmd_lower.contains("fork bomb") {
        warnings.push("fork bomb pattern detected".to_string());
    }
    // Also detect the common `:(){ :|:& };:` pattern (without spaces)
    if command.contains(":(){") && command.contains(":|:&") {
        warnings.push("fork bomb pattern detected".to_string());
    }

    // Write to system directories
    let system_write_patterns: &[(&str, &str)] = &[
        ("> /etc/", "/etc/"),
        (">> /etc/", "/etc/"),
        ("> /boot/", "/boot/"),
        (">> /boot/", "/boot/"),
        ("> /sys/", "/sys/"),
        (">> /sys/", "/sys/"),
        ("> /proc/", "/proc/"),
        (">> /proc/", "/proc/"),
    ];
    for (pattern, dir) in system_write_patterns {
        if cmd_lower.contains(pattern) {
            warnings.push(format!("write to system directory {}", dir));
            break;
        }
    }

    if warnings.is_empty() {
        None
    } else {
        Some(format!(
            "⚠️  SECURITY WARNING: {}",
            warnings
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Validate that a working directory is within the allowed workspace.
/// Returns an error message if the path is invalid/escapes, or the resolved path on success.
pub(crate) fn validate_cwd(dir: &str, workspace: Option<&Path>) -> Result<PathBuf, String> {
    let path = Path::new(dir);

    // Reject path traversal
    if path.components().any(|c| c.as_os_str() == "..") {
        return Err("Path traversal (..) not allowed in working directory".to_string());
    }

    if !path.exists() {
        return Err(format!("Working directory does not exist: {}", dir));
    }

    // If we have a workspace root, validate the cwd is within it
    if let Some(workspace_root) = workspace {
        // Canonicalize both paths to resolve symlinks and normalize
        let canonical_cwd = path
            .canonicalize()
            .map_err(|e| format!("Failed to resolve working directory: {}", e))?;
        let canonical_workspace = workspace_root
            .canonicalize()
            .map_err(|e| format!("Failed to resolve workspace directory: {}", e))?;

        if !canonical_cwd.starts_with(&canonical_workspace) {
            return Err(format!(
                "Working directory '{}' is outside the allowed workspace '{}'",
                canonical_cwd.display(),
                canonical_workspace.display()
            ));
        }

        return Ok(canonical_cwd);
    }

    // No workspace constraint - just return the original path
    Ok(path.to_path_buf())
}

// ── F-9: PTY-backed bash execution preserves ANSI SGR, drops other CSI/OSC ─

/// Outcome of a PTY-backed command run. The output is filtered to keep SGR
/// color escapes (`ESC [ ... m`) and drop cursor-motion / screen-control
/// sequences so the bytes are safe to embed in the agent transcript.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub struct PtyOutcome {
    /// Filtered PTY output (SGR kept, other CSI / OSC dropped).
    pub output: String,
    /// Exit status of the child, if available (None for signal kills).
    pub exit_code: Option<i32>,
}

/// Filter PTY bytes: keep SGR (`ESC [ ... m`) sequences, drop all other CSI
/// (`ESC [ ... <letter>`) and OSC (`ESC ] ... BEL|ST`) escapes. Non-control
/// bytes pass through unchanged.
///
/// Downstream renderers (TUI transcript, logs) treat `ESC [ 31m` red text as
/// meaningful, but `ESC [ 2J` clear-screen and `ESC [ H` cursor-home are noise
/// that would corrupt transcript diffs.
#[cfg(unix)]
fn ansi_filter(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != 0x1b {
            out.push(b);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            out.push(b);
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            b'[' => {
                // CSI: ESC [ <params> <final byte 0x40..=0x7e>
                let mut j = i + 2;
                while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                    j += 1;
                }
                if j < bytes.len() {
                    if bytes[j] == b'm' {
                        out.extend_from_slice(&bytes[i..=j]);
                    }
                    i = j + 1;
                } else {
                    i += 1;
                }
            }
            b']' => {
                // OSC: ESC ] <payload> terminated by BEL (0x07) or ST (ESC \).
                let mut j = i + 2;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        j += 1;
                        break;
                    }
                    if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            _ => {
                // ESC followed by something we don't recognize. Drop the
                // ESC; the next byte is kept verbatim.
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run a command inside a real PTY (portable-pty). Output is merged (stderr
/// is folded into stdout via `exec 2>&1`) and filtered. Returns the filtered
/// output and the child's exit code.
///
/// Low-level helper used by the bash tool when `OXICODE_BASH_PTY=1` is set.
/// Does NOT enforce `BLOCKED_ENV_VARS` or workspace CWD policy — those
/// concerns live in the calling tool.
#[cfg(unix)]
pub async fn run_in_pty(
    cmd: &str,
    cwd: &Path,
    timeout: Duration,
    abort: oneshot::Receiver<()>,
) -> Result<PtyOutcome, ToolError> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to open PTY: {e}"))?;

    // Single bash invocation: merge stderr into stdout inside the shell,
    // then run the user's command. This avoids juggling two PTY readers.
    // portable-pty already calls setsid() in the spawned child, so the
    // bash process becomes its own session / process-group leader —
    // signalling the whole group (negative pid) reaches bash AND any
    // descendants, leaving no orphans holding the PTY slave fd open
    // (which would hang read_thread.join()).
    let wrapped = format!("exec 2>&1; {cmd}");
    let mut builder = CommandBuilder::new("bash");
    builder.arg("-c");
    builder.arg(&wrapped);
    builder.cwd(cwd);

    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| format!("failed to spawn PTY child: {e}"))?;

    // Clone a kill handle so the timeout / abort path can stop the child
    // without owning the Child.
    let mut killer = child.clone_killer();
    let pid = child.process_id();

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to clone PTY reader: {e}"))?;
    drop(pair.slave);

    // Reader thread: blocking std::io::Read until EOF or error.
    let (read_tx, read_rx) = std::sync::mpsc::channel::<String>();
    let read_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.push_str(&String::from_utf8_lossy(&chunk[..n])),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    // EIO on Linux when the slave side closes — treat as EOF.
                    break;
                }
            }
        }
        let _ = read_tx.send(buf);
    });

    // Wait thread: blocking wait for the child.
    let wait_thread = std::thread::spawn(move || child.wait());

    // Bridge the oneshot abort signal into a polled AtomicBool so we can
    // race it against the wait timeout without spawning a tokio task. The
    // bridge thread blocks on `try_recv` until either the sender fires
    // or the channel is dropped — whichever comes first.
    let abort_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let abort_flag = abort_flag.clone();
        std::thread::spawn(move || {
            let mut abort = abort;
            loop {
                match abort.try_recv() {
                    Ok(_) => {
                        abort_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                    Err(oneshot::error::TryRecvError::Closed) => return,
                    Err(oneshot::error::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        });
    }

    let timeout_at = std::time::Instant::now() + timeout;
    let wait_handle = wait_thread;
    let (status_opt, timed_out, aborted): (Option<portable_pty::ExitStatus>, bool, bool) = loop {
        if wait_handle.is_finished() {
            let join_result = wait_handle.join();
            let status = match join_result {
                Ok(Ok(s)) => Some(s),
                _ => None,
            };
            break (status, false, false);
        }
        if std::time::Instant::now() >= timeout_at {
            pty_kill_process_group(pid, &mut killer);
            break (None, true, false);
        }
        if abort_flag.load(std::sync::atomic::Ordering::SeqCst) {
            pty_kill_process_group(pid, &mut killer);
            break (None, true, true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    // Drain any partial output, then join the reader thread with a bounded
    // timeout. Killing the process group closes the slave fd, so the
    // reader's blocking read returns EOF and the thread terminates — but
    // cap the join wait so we never hang the agent loop if something is
    // wrong.
    let raw = read_rx
        .recv_timeout(Duration::from_millis(500))
        .unwrap_or_default();
    let join_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !read_thread.is_finished() && std::time::Instant::now() < join_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // If the reader thread is still alive after 2s, leak it rather than
    // hanging the agent loop. The OS reclaims it when the group is torn
    // down.
    drop(read_thread);

    if aborted {
        let filtered = ansi_filter(&raw);
        return Err(format!("Command aborted; partial output:\n{filtered}"));
    }
    if timed_out {
        let filtered = ansi_filter(&raw);
        return Err(format!(
            "Command timed out after {} seconds; partial output:\n{filtered}",
            timeout.as_secs(),
        ));
    }

    let filtered = ansi_filter(&raw);
    Ok(PtyOutcome {
        output: filtered,
        exit_code: status_opt.map(|s| s.exit_code() as i32),
    })
}

/// Send SIGKILL to the bash process group (negative pid → group) when a
/// pid is available, falling back to the portable-pty kill trait. Used by
/// both timeout and abort paths.
#[cfg(unix)]
fn pty_kill_process_group(
    pid: Option<u32>,
    killer: &mut Box<dyn portable_pty::ChildKiller + Send + Sync>,
) {
    if let Some(pid) = pid {
        // SAFETY: pid is a live owned child process at the time of the call.
        // portable-pty calls setsid() in the spawned child, so a negative
        // pid targets bash + its descendants (the process group).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    } else {
        let _ = killer.kill();
    }
}

/// Default timeout in seconds
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// BashTool.
pub struct BashTool {
    root_dir: Option<PathBuf>,
    progress_callback: Arc<std::sync::Mutex<Option<ProgressCallback>>>,
}

impl BashTool {
    /// Create with no explicit root (uses ToolContext.workspace_dir at runtime).
    pub fn new() -> Self {
        Self {
            root_dir: None,
            progress_callback: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Create with a specific working directory (overrides ToolContext).
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            root_dir: Some(cwd),
            progress_callback: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Format a duration for human-readable display
    pub(crate) fn format_duration(duration: Duration) -> String {
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();
        if secs >= 60 {
            let mins = secs / 60;
            let remain_secs = secs % 60;
            format!(
                "{}m {:.1}s",
                mins,
                remain_secs as f64 + millis as f64 / 1000.0
            )
        } else {
            format!("{:.1}s", secs as f64 + millis as f64 / 1000.0)
        }
    }

    /// Build the output string with optional truncation notice and timing.
    pub(crate) fn build_output(
        truncation: &TruncationResult,
        elapsed: Duration,
        exit_code: Option<i32>,
    ) -> String {
        let mut output = truncation.content.clone();

        // Append truncation notice if output was truncated
        if truncation.truncated {
            let notice = match truncation.truncated_by {
                truncate::TruncatedBy::Lines => format!(
                    "\n\n[Truncated: showing {} of {} lines. {} bytes remaining]",
                    truncation.output_lines,
                    truncation.total_lines,
                    truncate::format_bytes(
                        truncation
                            .total_bytes
                            .saturating_sub(truncation.output_bytes)
                    )
                ),
                truncate::TruncatedBy::Bytes => format!(
                    "\n\n[Truncated: {} lines shown ({} byte limit). Total was {} lines, {}]",
                    truncation.output_lines,
                    truncate::format_bytes(truncate::DEFAULT_MAX_BYTES),
                    truncation.total_lines,
                    truncate::format_bytes(truncation.total_bytes)
                ),
                truncate::TruncatedBy::None => String::new(),
            };
            output.push_str(&notice);
        }

        // Append exit code for non-zero
        if let Some(code) = exit_code
            && code != 0
        {
            output.push_str(&format!("\n\nCommand exited with code {}", code));
        }

        // Append timing
        output.push_str(&format!("\n\nTook {}", Self::format_duration(elapsed)));

        output
    }

    /// Wait for a child process with timeout and optional abort signal.
    async fn wait_with_timeout_and_signal(
        child: &mut tokio::process::Child,
        timeout: u64,
        signal: &mut Option<oneshot::Receiver<()>>,
    ) -> Result<std::process::ExitStatus, String> {
        let timeout_duration = Duration::from_secs(timeout);

        tokio::select! {
            status = child.wait() => {
                status.map_err(|e| format!("Failed to wait for process: {}", e))
            }
            _ = tokio::time::sleep(timeout_duration) => {
                Self::kill_process_group(child).await;
                Err(format!("Command timed out after {} seconds", timeout))
            }
            _ = async {
                match signal {
                    Some(rx) => { let _ = rx.await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                Self::kill_process_group(child).await;
                Err("Command aborted".to_string())
            }
        }
    }

    /// Build the shell command with working directory and environment variables.
    fn build_shell_command(
        command: &str,
        work_dir: &Option<String>,
        env: Option<&serde_json::Map<String, Value>>,
    ) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0);

        if let Some(dir) = work_dir {
            cmd.current_dir(dir);
        }

        if let Some(env_map) = env {
            for (key, val) in env_map {
                if BLOCKED_ENV_VARS
                    .iter()
                    .any(|blocked| blocked.eq_ignore_ascii_case(key))
                {
                    continue;
                }
                if let Some(val_str) = val.as_str() {
                    cmd.env(key, val_str);
                }
            }
        }

        cmd
    }

    /// Kill a process group (Unix) or fall back to child.kill().
    async fn kill_process_group(child: &mut tokio::process::Child) {
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                let pgid = -(pid as i32);
                // SAFETY: libc::kill sends SIGKILL to the process group. The negative PID
                // targets the entire group (shell + child processes). PID comes from
                // child.id() which is a valid running process owned by this process.
                unsafe {
                    libc::kill(pgid, libc::SIGKILL);
                }
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    /// Format error output for timeout/abort cases.
    fn format_error_output(
        stdout_str: &str,
        stderr_str: &str,
        error_msg: &str,
        elapsed: Duration,
    ) -> String {
        let mut output = String::new();
        if !stdout_str.is_empty() {
            output.push_str(stdout_str);
        }
        if !stderr_str.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(stderr_str);
        }

        if !output.is_empty() {
            let truncation = truncate::truncate_head(&output, &TruncationOptions::default());
            output = truncation.content;
        }

        output.push_str(&format!("\n\n{}", error_msg));
        output.push_str(&format!("\nTook {}", Self::format_duration(elapsed)));
        output
    }

    /// Execute a command using tokio::process::Command with full feature support.
    async fn run_command(
        root_dir: &Path,
        command: &str,
        cwd: Option<&str>,
        env: Option<&serde_json::Map<String, Value>>,
        timeout_secs: Option<u64>,
        progress_cb: &Option<ProgressCallback>,
        mut signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        if let Some(cb) = progress_cb {
            cb(format!("Executing: {}", command));
        }

        let timeout = timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let start = Instant::now();

        // Resolve working directory
        let work_dir = match cwd {
            Some(dir) if !dir.is_empty() => {
                let validated = validate_cwd(dir, Some(root_dir))?;
                Some(validated.to_string_lossy().to_string())
            }
            _ => Some(root_dir.to_string_lossy().to_string()),
        };

        // Build the command
        let mut cmd = Self::build_shell_command(command, &work_dir, env);

        // Spawn the child process
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        // Take stdout and stderr handles for separate capture
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture stdout".to_string())?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| "Failed to capture stderr".to_string())?;

        // Read stdout and stderr concurrently
        let stdout_handle = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf).await;
            buf
        });
        let stderr_handle = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf).await;
            buf
        });

        // Wait for the process with timeout and signal handling
        let result = Self::wait_with_timeout_and_signal(&mut child, timeout, &mut signal).await;

        let elapsed = start.elapsed();

        // Collect stdout and stderr
        let stdout_bytes = stdout_handle.await.unwrap_or_default();
        let stderr_bytes = stderr_handle.await.unwrap_or_default();

        let stdout_str = String::from_utf8_lossy(&stdout_bytes).to_string();
        let stderr_str = String::from_utf8_lossy(&stderr_bytes).to_string();

        if let Some(cb) = progress_cb {
            cb(format!(
                "Process completed in {}",
                Self::format_duration(elapsed)
            ));
        }

        match result {
            Ok(status) => {
                let exit_code = status.code();
                if let Some(code) = exit_code
                    && let Some(cb) = progress_cb
                {
                    cb(format!("Process exited with code {}", code));
                }
                let combined = if stderr_str.is_empty() {
                    stdout_str.clone()
                } else if stdout_str.is_empty() {
                    stderr_str.clone()
                } else {
                    format!("{}\n{}", stdout_str, stderr_str)
                };

                let security_warning = is_dangerous_command(command);

                let truncation = truncate::truncate_head(
                    if combined.is_empty() {
                        "(no output)"
                    } else {
                        &combined
                    },
                    &TruncationOptions::default(),
                );

                let mut output = Self::build_output(&truncation, elapsed, exit_code);

                if let Some(ref warning) = security_warning {
                    output.push_str(&format!("\n{}", warning));
                }

                if status.success() {
                    Ok(AgentToolResult::success(output))
                } else {
                    Ok(AgentToolResult::error(output))
                }
            }
            Err(e) => {
                let output = Self::format_error_output(&stdout_str, &stderr_str, &e, elapsed);
                Ok(AgentToolResult::error(output))
            }
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn label(&self) -> &str {
        "Bash"
    }

    fn essential(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Execute a bash command in a shell. Returns stdout and stderr. \
         Output is truncated to 2000 lines or 50KB (whichever is hit first). \
         Set timeout to limit execution time."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)",
                    "default": 120
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command (optional)"
                },
                "env": {
                    "type": "object",
                    "description": "Environment variables as key-value pairs (optional)",
                    "additionalProperties": {
                        "type": "string"
                    }
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let command = params
            .get("command")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?;

        // F-10 (audit 2026-06-21): in strict mode (OXICODE_STRICT_BASH=1) refuse
        // commands that match `is_dangerous_command` patterns outright
        // instead of merely appending a warning after the fact. Without
        // strict mode, the original "warn after execution" behavior is
        // preserved (so existing users see no change). Strict mode is
        // opt-in because some legitimate operations match the heuristic
        // (e.g. `cat /etc/passwd` is sometimes needed to inspect user
        // identity); making it opt-in preserves backward compatibility
        // while giving security-conscious deployments a hard block.
        // F-10 (audit 2026-06-21): collapsed `if` so the two predicates
        // (`OXICODE_STRICT_BASH=1` AND a dangerous-pattern match) live on a
        // single line — clippy::collapsible_if flagged the original form.
        if std::env::var_os("OXICODE_STRICT_BASH").as_deref() == Some(std::ffi::OsStr::new("1"))
            && let Some(reason) = is_dangerous_command(command)
        {
            return Err(format!(
                "OXICODE_STRICT_BASH=1 blocked dangerous command: {reason}"
            ));
        }

        let cwd = params.get("cwd").and_then(|v: &Value| v.as_str());
        let timeout = params.get("timeout").and_then(|v: &Value| v.as_u64());
        let env = params.get("env").and_then(|v: &Value| v.as_object());

        // SAFETY: a poisoned lock means the previous holder panicked while
        // holding it — a real bug that must surface, not be swallowed. The
        // tool fails closed rather than proceeding with inconsistent state.
        #[allow(clippy::expect_used)]
        let progress_cb = self
            .progress_callback
            .lock()
            .expect("progress callback lock poisoned")
            .clone();

        // Use root_dir if set, else ctx.root()
        let root = self.root_dir.as_deref().unwrap_or(ctx.root());

        // F-9 (audit 2026-08-24): PTY-backed bash execution preserves ANSI
        // SGR color sequences in command output. Off by default; opt-in via
        // `OXICODE_BASH_PTY=1` (the matching `bash_pty` setting in
        // oxicode-cli is informational — the agent tool cannot see cli
        // settings today; the env var is the only live override).
        //
        // Stderr is merged into stdout inside the PTY command, so the
        // existing combined-output / truncation / timing pipeline still
        // works unchanged.
        #[cfg(unix)]
        if std::env::var_os("OXICODE_BASH_PTY").as_deref() == Some(std::ffi::OsStr::new("1")) {
            let work_dir = match cwd {
                Some(dir) if !dir.is_empty() => validate_cwd(dir, Some(root))?,
                _ => root.to_path_buf(),
            };
            let timeout_secs = timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);
            let start = Instant::now();
            if let Some(cb) = &progress_cb {
                cb(format!("Executing (pty): {}", command));
            }
            // Convert optional abort into a concrete oneshot::Receiver<()>.
            // If the caller didn't provide one, create a fresh channel and
            // immediately drop the sender so the receiver reports Closed
            // (treated as "never aborts" by the bridge thread).
            let abort_rx = match signal {
                Some(rx) => rx,
                None => {
                    let (tx, rx) = oneshot::channel();
                    drop(tx);
                    rx
                }
            };
            let outcome = run_in_pty(
                command,
                &work_dir,
                Duration::from_secs(timeout_secs),
                abort_rx,
            )
            .await;
            let elapsed = start.elapsed();
            if let Some(cb) = &progress_cb {
                cb(format!(
                    "Process (pty) completed in {}",
                    Self::format_duration(elapsed)
                ));
            }
            return match outcome {
                Ok(o) => {
                    let combined = if o.output.is_empty() {
                        "(no output)".to_string()
                    } else {
                        o.output
                    };
                    let truncation =
                        truncate::truncate_head(&combined, &TruncationOptions::default());
                    let mut output = Self::build_output(&truncation, elapsed, o.exit_code);
                    if let Some(reason) = is_dangerous_command(command) {
                        output.push_str(&format!("\n{}", reason));
                    }
                    if o.exit_code == Some(0) {
                        Ok(AgentToolResult::success(output))
                    } else {
                        Ok(AgentToolResult::error(output))
                    }
                }
                Err(e) => {
                    let mut output = format!("\n\n{}", e);
                    output.push_str(&format!("\nTook {}", Self::format_duration(elapsed)));
                    Ok(AgentToolResult::error(output))
                }
            };
        }

        Self::run_command(root, command, cwd, env, timeout, &progress_cb, signal).await
    }

    fn on_progress(&self, callback: ProgressCallback) {
        let cb = self.progress_callback.clone();
        // SAFETY: a poisoned lock means the previous holder panicked while
        // holding it — a real bug that must surface, not be swallowed.
        #[allow(clippy::expect_used)]
        let mut guard = cb.lock().expect("progress callback lock poisoned");
        *guard = Some(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(command: &str) -> Value {
        json!({ "command": command })
    }

    fn make_params_with_timeout(command: &str, timeout: u64) -> Value {
        json!({ "command": command, "timeout": timeout })
    }

    fn make_params_with_cwd(command: &str, cwd: &str) -> Value {
        json!({ "command": command, "cwd": cwd })
    }

    fn make_params_with_env(command: &str, env: serde_json::Value) -> Value {
        json!({ "command": command, "env": env })
    }

    #[tokio::test]
    async fn test_simple_command() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-1",
                make_params("echo hello"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_command_with_args() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-2",
                make_params("echo hello world"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_failed_command() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-3",
                make_params("exit 1"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("exited with code 1"));
    }

    #[tokio::test]
    async fn test_missing_command_param() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-4", json!({}), None, &ToolContext::default())
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Missing required parameter: command")
        );
    }

    #[tokio::test]
    async fn test_no_output() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-5", make_params("true"), None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("(no output)"));
    }

    #[tokio::test]
    async fn test_stderr_capture() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-6",
                make_params("echo error_msg >&2"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("error_msg"));
    }

    #[tokio::test]
    async fn test_timeout_kills_process() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-7",
                make_params_with_timeout("sleep 300", 1),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("timed out"));
    }

    #[tokio::test]
    async fn test_timeout_default() {
        // Verify default timeout is 120 seconds by checking the parameter schema
        let tool = BashTool::new();
        let schema = tool.parameters_schema();
        assert_eq!(schema["properties"]["timeout"]["default"], 120);
    }

    #[tokio::test]
    async fn test_working_directory() {
        let tool = BashTool::with_cwd(PathBuf::from("/tmp"));
        let result = tool
            .execute(
                "test-8",
                make_params_with_cwd("pwd", "/tmp"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("/tmp") || result.output.contains("/private/tmp"));
    }

    #[tokio::test]
    async fn test_working_directory_nonexistent() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-9",
                make_params_with_cwd("echo hi", "/nonexistent/dir/xyz"),
                None,
                &ToolContext::default(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_working_directory_traversal() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-10",
                make_params_with_cwd("echo hi", "/tmp/../etc"),
                None,
                &ToolContext::default(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Path traversal"));
    }

    #[tokio::test]
    async fn test_env_variables() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-11",
                make_params_with_env(
                    "echo $OXICODE_TEST_VAR",
                    json!({ "OXICODE_TEST_VAR": "hello_from_env" }),
                ),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello_from_env"));
    }

    #[tokio::test]
    async fn test_env_variables_multiple() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-12",
                make_params_with_env(
                    "echo $OXICODE_A $OXICODE_B",
                    json!({ "OXICODE_A": "first", "OXICODE_B": "second" }),
                ),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("first second"));
    }

    #[tokio::test]
    async fn test_duration_timing() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-13",
                make_params("sleep 0.1 && echo done"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Took "));
        assert!(result.output.contains("s")); // Should contain seconds
    }

    #[tokio::test]
    async fn test_combined_stdout_stderr() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test",
                make_params("echo stdout_msg; echo stderr_msg >&2"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("stdout_msg"));
        assert!(result.output.contains("stderr_msg"));
    }

    #[tokio::test]
    async fn test_output_truncation() {
        let tool = BashTool::new();
        // Generate more than 2000 lines to trigger truncation
        let result = tool
            .execute(
                "test-15",
                make_params("seq 1 3000"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("truncated") || result.output.contains("Truncated"));
    }

    #[tokio::test]
    async fn test_signal_aborts_process() {
        let tool = BashTool::new();
        let (tx, rx) = oneshot::channel();

        // Spawn a task that will send the abort signal after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = tx.send(());
        });

        let result = tool
            .execute(
                "test-16",
                make_params("sleep 300"),
                Some(rx),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("aborted"));
    }

    #[tokio::test]
    async fn test_parameters_schema() {
        let tool = BashTool::new();
        let schema = tool.parameters_schema();

        // Check required fields
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("command")));

        // Check all expected properties exist
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("command"));
        assert!(props.contains_key("timeout"));
        assert!(props.contains_key("cwd"));
        assert!(props.contains_key("env"));

        // Check types
        assert_eq!(props["command"]["type"], "string");
        assert_eq!(props["timeout"]["type"], "integer");
        assert_eq!(props["cwd"]["type"], "string");
        assert_eq!(props["env"]["type"], "object");
    }

    #[tokio::test]
    async fn test_multiline_output() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test",
                make_params("echo line1 && echo line2 && echo line3"),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("line1"));
        assert!(result.output.contains("line2"));
        assert!(result.output.contains("line3"));
    }

    #[tokio::test]
    async fn test_format_duration() {
        assert_eq!(
            BashTool::format_duration(Duration::from_millis(500)),
            "0.5s"
        );
        assert_eq!(BashTool::format_duration(Duration::from_secs(1)), "1.0s");
        assert_eq!(
            BashTool::format_duration(Duration::from_secs(65)),
            "1m 5.0s"
        );
        assert_eq!(
            BashTool::format_duration(Duration::from_secs(120)),
            "2m 0.0s"
        );
    }

    // ── F-10 regression: OXICODE_STRICT_BASH=1 blocks dangerous commands ─────

    /// With `OXICODE_STRICT_BASH=1`, a command matching `is_dangerous_command`
    /// must be refused BEFORE `sh -c` runs (audit finding F-10). Without
    /// the gate, the agent receives an `AgentToolResult::success` with a
    /// trailing warning — i.e. the dangerous command ran and only the
    /// post-hoc warning tried to discourage it.
    #[tokio::test]
    async fn test_strict_bash_blocks_pipe_to_shell() {
        // SAFETY: env mutation under test (Rust 2024 makes this unsafe);
        // acceptable for a single isolated #[tokio::test].
        unsafe {
            std::env::set_var("OXICODE_STRICT_BASH", "1");
        }
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-strict",
                make_params("echo hi | sh"),
                None,
                &ToolContext::default(),
            )
            .await;
        unsafe {
            std::env::remove_var("OXICODE_STRICT_BASH");
        }
        // The command should be refused — `Err(...)` with the audit reason.
        let err = result.expect_err("strict mode must refuse `| sh` commands");
        assert!(
            err.contains("OXICODE_STRICT_BASH") && err.contains("pipe to shell"),
            "unexpected error: {err}"
        );
    }

    /// Without `OXICODE_STRICT_BASH`, the legacy "warn after execution"
    /// behavior is preserved (backward compatible).
    #[tokio::test]
    async fn test_strict_bash_off_preserves_warning_behavior() {
        // Ensure strict mode is off.
        unsafe {
            std::env::remove_var("OXICODE_STRICT_BASH");
        }
        let tool = BashTool::new();
        let result = tool
            .execute(
                "test-lenient",
                make_params("echo hi"),
                None,
                &ToolContext::default(),
            )
            .await;
        let r = result.expect("non-dangerous command must succeed when strict is off");
        assert!(r.success, "echo hi must succeed: {}", r.output);
        assert!(!r.output.contains("OXICODE_STRICT_BASH"));
    }

    // ── F-9: PTY-backed bash execution preserves ANSI SGR, drop CSI/OSC ─

    /// Run a command through a real PTY and assert that the SGR color sequence
    /// `\x1b[31m` survives the capture. Pipe-based execution strips these bytes;
    /// the PTY path is the only way to keep colorized output for downstream
    /// renderers that respect SGR.
    #[cfg(unix)]
    #[tokio::test]
    async fn pty_preserves_color_codes() {
        let cwd = std::env::current_dir().expect("current_dir");
        let (_tx, rx) = oneshot::channel::<()>();
        let outcome = run_in_pty(
            "printf '\\x1b[31mred\\x1b[0m'",
            &cwd,
            Duration::from_secs(10),
            rx,
        )
        .await
        .expect("run_in_pty");
        assert!(
            outcome.output.contains("\x1b[31m"),
            "PTY output must preserve the \\x1b[31m SGR escape; got: {:?}",
            outcome.output
        );
        assert!(
            outcome.output.contains("red"),
            "PTY output must contain the printed text; got: {:?}",
            outcome.output
        );
    }

    /// Run a command that emits a CSI screen-clear escape and assert that the
    /// filter strips the control sequence while preserving the text. The
    /// filter keeps SGR (`ESC [ ... m`) and drops other CSI / OSC sequences
    /// so that cursor motion and screen-control bytes don't leak into the
    /// agent transcript.
    #[cfg(unix)]
    #[tokio::test]
    async fn pty_strips_cursor_motion() {
        let cwd = std::env::current_dir().expect("current_dir");
        let (_tx, rx) = oneshot::channel::<()>();
        let outcome = run_in_pty(
            "printf '\\x1b[2Jhello\\x1b[H'",
            &cwd,
            Duration::from_secs(10),
            rx,
        )
        .await
        .expect("run_in_pty");
        assert!(
            !outcome.output.contains("\x1b[2J"),
            "PTY output must drop the screen-clear CSI; got: {:?}",
            outcome.output
        );
        assert!(
            !outcome.output.contains("\x1b[H"),
            "PTY output must drop the cursor-home CSI; got: {:?}",
            outcome.output
        );
        assert!(
            outcome.output.contains("hello"),
            "PTY output must contain the printed text; got: {:?}",
            outcome.output
        );
    }

    // ── F-9 round 2: process-group kill on timeout ─────────────────

    /// Assert that run_in_pty respects a deadline. When the user command
    /// outlives the deadline the function returns Err(...) AND returns
    /// promptly — a hung child holding the PTY slave fd would hang
    /// read_thread.join() indefinitely.
    #[cfg(unix)]
    #[tokio::test]
    async fn pty_timeout_kills_process_group_promptly() {
        let cwd = std::env::current_dir().expect("current_dir");
        let (_tx, rx) = oneshot::channel::<()>();
        let start = std::time::Instant::now();
        let result = run_in_pty("sleep 30", &cwd, Duration::from_millis(200), rx).await;
        let elapsed = start.elapsed();
        let err = result.expect_err("sleep 30 must time out within 200ms budget");
        // Bounded: must return within 5 seconds even if the process group
        // kill fails for any reason.
        assert!(
            elapsed < Duration::from_secs(5),
            "run_in_pty took {elapsed:?} after timeout — likely a hang on join"
        );
        assert!(
            err.contains("timed out") || err.contains("Timeout"),
            "error must mention the timeout: {err}"
        );
    }

    /// Assert the abort signal path tears the PTY child down. The abort
    /// signal fires immediately, the PTY group is killed, run_in_pty
    /// returns Err(...) promptly with an `aborted` message.
    #[cfg(unix)]
    #[tokio::test]
    async fn pty_abort_signal_tears_down_promptly() {
        let cwd = std::env::current_dir().expect("current_dir");
        let (tx, rx) = oneshot::channel::<()>();
        let start = std::time::Instant::now();
        // Fire the abort on a std::thread (not tokio::spawn) — run_in_pty
        // is async but its body is purely blocking, so a tokio::spawn-ed
        // task wouldn't be polled until run_in_pty returns to an await
        // point. A real OS thread sends independently.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = tx.send(());
        });
        let result = run_in_pty("sleep 30", &cwd, Duration::from_secs(30), rx).await;
        let elapsed = start.elapsed();
        let err = result.expect_err("sleep 30 must be aborted by signal");
        assert!(
            elapsed < Duration::from_secs(5),
            "abort took {elapsed:?} — likely a hang on join"
        );
        assert!(
            err.contains("aborted") || err.contains("Aborted"),
            "error must mention the abort: {err}"
        );
    }
}
