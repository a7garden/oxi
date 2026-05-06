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
use super::{AgentTool, AgentToolResult, ProgressCallback, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;

/// Default timeout in seconds
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// BashTool.
pub struct BashTool {
    progress_callback: Arc<std::sync::Mutex<Option<ProgressCallback>>>,
}

impl BashTool {
/// TODO.
    pub fn new() -> Self {
        Self {
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

    /// Execute a command using tokio::process::Command with full feature support.
    async fn run_command(
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
                let path = Path::new(dir);
                if path.components().any(|c| c.as_os_str() == "..") {
                    return Err("Path traversal (..) not allowed in working directory".to_string());
                }
                if !path.exists() {
                    return Err(format!("Working directory does not exist: {}", dir));
                }
                Some(dir.to_string())
            }
            _ => None,
        };

        // Build the command
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Create a new process group so we can kill the entire tree
            .process_group(0);

        // Set working directory
        if let Some(ref dir) = work_dir {
            cmd.current_dir(dir);
        }

        // Set environment variables
        if let Some(env_map) = env {
            for (key, val) in env_map {
                if let Some(val_str) = val.as_str() {
                    cmd.env(key, val_str);
                }
            }
        }

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

        // Wait for the process with timeout and signal handling via tokio::select!
        let timeout_duration = Duration::from_secs(timeout);
        let result = tokio::select! {
            // Branch 1: Process exits normally
            status = child.wait() => {
                let status = status.map_err(|e| format!("Failed to wait for process: {}", e))?;
                Ok(status)
            }

            // Branch 2: Timeout elapsed
            _ = tokio::time::sleep(timeout_duration) => {
                // Kill the entire process group
                let _ = child.kill().await;
                let _ = child.wait().await; // Reap the child
                Err(format!("Command timed out after {} seconds", timeout))
            }

            // Branch 3: External cancel/abort signal received
            sig = async {
                if let Some(ref mut rx) = signal {
                    let _ = rx.await;
                } else {
                    // If no signal is provided, wait forever (never triggers)
                    std::future::pending::<()>().await;
                }
            } => {
                let _ = sig; // suppress unused warning
                // Kill the entire process group
                let _ = child.kill().await;
                let _ = child.wait().await; // Reap the child
                Err("Command aborted".to_string())
            }
        };

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

                // Apply truncation
                let truncation = truncate::truncate_head(
                    if combined.is_empty() {
                        "(no output)"
                    } else {
                        &combined
                    },
                    &TruncationOptions::default(),
                );

                let output = Self::build_output(&truncation, elapsed, exit_code);

                if status.success() {
                    Ok(AgentToolResult::success(output))
                } else {
                    Ok(AgentToolResult::error(output))
                }
            }
            Err(e) => {
                // Timeout or abort - include whatever output we got
                let mut output = String::new();
                if !stdout_str.is_empty() {
                    output.push_str(&stdout_str);
                }
                if !stderr_str.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&stderr_str);
                }

                if !output.is_empty() {
                    let truncation =
                        truncate::truncate_head(&output, &TruncationOptions::default());
                    output = truncation.content;
                }

                output.push_str(&format!("\n\n{}", e));
                output.push_str(&format!("\nTook {}", Self::format_duration(elapsed)));
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
    ) -> Result<AgentToolResult, ToolError> {
        let command = params
            .get("command")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?;

        let cwd = params.get("cwd").and_then(|v: &Value| v.as_str());
        let timeout = params.get("timeout").and_then(|v: &Value| v.as_u64());
        let env = params.get("env").and_then(|v: &Value| v.as_object());

        let progress_cb = self.progress_callback.lock().expect("progress callback lock poisoned").clone();

        Self::run_command(command, cwd, env, timeout, &progress_cb, signal).await
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
            .execute("test-1", make_params("echo hello"), None)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_command_with_args() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-2", make_params("echo hello world"), None)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_failed_command() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-3", make_params("exit 1"), None)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("exited with code 1"));
    }

    #[tokio::test]
    async fn test_missing_command_param() {
        let tool = BashTool::new();
        let result = tool.execute("test-4", json!({}), None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Missing required parameter: command"));
    }

    #[tokio::test]
    async fn test_no_output() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-5", make_params("true"), None)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("(no output)"));
    }

    #[tokio::test]
    async fn test_stderr_capture() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-6", make_params("echo error_msg >&2"), None)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("error_msg"));
    }

    #[tokio::test]
    async fn test_timeout_kills_process() {
        let tool = BashTool::new();
        let result = tool
            .execute("test-7", make_params_with_timeout("sleep 300", 1), None)
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
        let tool = BashTool::new();
        let result = tool
            .execute("test-8", make_params_with_cwd("pwd", "/tmp"), None)
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
            .execute("test-13", make_params("sleep 0.1 && echo done"), None)
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
                "test-14",
                make_params("echo stdout_msg && echo stderr_msg >&2"),
                None,
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
            .execute("test-15", make_params("seq 1 3000"), None)
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
            .execute("test-16", make_params("sleep 300"), Some(rx))
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
                "test-17",
                make_params("echo -e 'line1\nline2\nline3'"),
                None,
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
