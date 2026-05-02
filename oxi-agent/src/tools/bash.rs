//! Bash tool - execute shell commands

use super::{AgentTool, AgentToolResult, ProgressCallback, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio as StdStdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::process::{Command, Stdio};
use tokio::sync::{Mutex, oneshot};

pub struct BashTool {
    progress_callback: Arc<Mutex<Option<ProgressCallback>>>,
}

impl BashTool {
    pub fn new() -> Self {
        Self {
            progress_callback: Arc::new(Mutex::new(None)),
        }
    }

    async fn run_command_impl(
        command: &str,
        working_dir: Option<&str>,
        timeout_secs: Option<u64>,
        progress_cb: &Option<ProgressCallback>,
    ) -> Result<String, ToolError> {
        if let Some(cb) = progress_cb {
            cb(format!("Executing: {}", command));
        }

        // Use blocking std Command for simplicity
        let mut cmd = StdCommand::new("sh");
        cmd.arg("-c").arg(command);
        cmd.stdout(StdStdio::piped());
        cmd.stderr(StdStdio::piped());

        if let Some(dir) = working_dir {
            if !dir.contains("..") && Path::new(dir).exists() {
                cmd.current_dir(dir);
            }
        }

        // Execute with optional timeout
        let output = if let Some(timeout) = timeout_secs {
            let result = std::thread::spawn(move || {
                std::time::timeout(Duration::from_secs(timeout), cmd.output())
            }).join().unwrap_or(Ok(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut, "timeout thread panicked"
            )))?}
            .map_err(|e| format!("Command timed out: {}", e))?
        } else {
            cmd.output().map_err(|e| format!("Command failed: {}", e))?
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if let Some(cb) = progress_cb {
            cb(format!("Process exited with code: {}", output.status.code().unwrap_or(-1)));
        }

        if output.status.success() {
            if stdout.is_empty() && stderr.is_empty() {
                Ok("(no output)".to_string())
            } else {
                Ok(stdout)
            }
        } else {
            Err(format!(
                "Command failed with exit code {}: {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { &stdout } else { &stderr }
            ))
        }
    }

    async fn read_dir_impl(path: &str) -> Result<String, ToolError> {
        let path = Path::new(path);

        if path.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }

        let mut entries = Vec::new();
        let mut dir = fs::read_dir(path)
            .await
            .map_err(|e| format!("Cannot read directory: {}", e))?;

        while let Some(entry) = dir.next_entry().await.map_err(|e| e.to_string())? {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().await.map_err(|e| e.to_string())?;
            let prefix = if file_type.is_dir() { "/" } else { "" };
            entries.push(format!("{}{}", file_name, prefix));
        }

        Ok(entries.join("\n"))
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
        "Execute a bash command in a shell. Returns stdout. Set timeout to limit execution time."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in seconds"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let command = params
            .get("command")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?;

        let working_dir = params.get("working_dir").and_then(|v: &Value| v.as_str());
        let timeout = params.get("timeout").and_then(|v: &Value| v.as_u64());

        let progress_cb = self.progress_callback.lock().await.clone();

        match Self::run_command_impl(command, working_dir, timeout, &progress_cb).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }

    fn on_progress(&self, callback: ProgressCallback) {
        let progress_cb = Arc::new(callback);
        let cb = self.progress_callback.clone();
        tokio::spawn(async move {
            let mut guard = cb.lock().await;
            *guard = Some(progress_cb);
        });
    }
}