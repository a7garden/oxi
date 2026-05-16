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
use serde_json::{json, Value};
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
fn is_dangerous_command(command: &str) -> Option<String> {
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
fn validate_cwd(dir: &str, workspace: Option<&Path>) -> Result<PathBuf, String> {
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
    fn format_duration(duration: Duration) -> String {
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
    fn build_output(
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
        if let Some(code) = exit_code {
            if code != 0 {
                output.push_str(&format!("\n\nCommand exited with code {}", code));
            }
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

        if let Some(ref dir) = work_dir {
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
                if let Some(code) = exit_code {
                    if let Some(cb) = progress_cb {
                        cb(format!("Process exited with code {}", code));
                    }
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

        let cwd = params.get("cwd").and_then(|v: &Value| v.as_str());
        let timeout = params.get("timeout").and_then(|v: &Value| v.as_u64());
        let env = params.get("env").and_then(|v: &Value| v.as_object());

        let progress_cb = self
            .progress_callback
            .lock()
            .expect("progress callback lock poisoned")
            .clone();

        // Use root_dir if set, else ctx.root()
        let root = self.root_dir.as_deref().unwrap_or(ctx.root());

        Self::run_command(root, command, cwd, env, timeout, &progress_cb, signal).await
    }

    fn on_progress(&self, callback: ProgressCallback) {
        let cb = self.progress_callback.clone();
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
        assert!(result
            .unwrap_err()
            .contains("Missing required parameter: command"));
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
                    "echo $OXI_TEST_VAR",
                    json!({ "OXI_TEST_VAR": "hello_from_env" }),
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
                    "echo $OXI_A $OXI_B",
                    json!({ "OXI_A": "first", "OXI_B": "second" }),
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
}
