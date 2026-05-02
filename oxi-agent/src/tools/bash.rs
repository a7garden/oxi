//! Bash tool - execute shell commands

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::oneshot;

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }

    async fn run_command_impl(
        command: &str,
        working_dir: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<String, ToolError> {
        // Build the command
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set working directory if specified
        if let Some(dir) = working_dir {
            let path = Path::new(dir);
            // Security: prevent path traversal
            if dir.contains("..") {
                return Err("Path traversal not allowed in working directory".to_string());
            }
            if path.exists() {
                cmd.current_dir(dir);
            }
        }

        // Set timeout if specified
        let timeout = timeout_secs.map(Duration::from_secs);

        // Execute with timeout
        let output = if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, cmd.output())
                .await
                .map_err(|_| "Command timed out")?
                .map_err(|e| format!("Command execution failed: {}", e))?
        } else {
            cmd.output()
                .await
                .map_err(|e| format!("Command execution failed: {}", e))?
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            if stdout.is_empty() && stderr.is_empty() {
                Ok("(no output)".to_string())
            } else {
                Ok(stdout.to_string())
            }
        } else {
            let exit_code = output.status.code().unwrap_or(-1);
            Err(format!(
                "Command failed with exit code {}: {}",
                exit_code,
                if stderr.is_empty() { &stdout } else { &stderr }
            ))
        }
    }

    async fn read_dir_impl(path: &str) -> Result<String, ToolError> {
        let path = Path::new(path);

        // Security: prevent path traversal
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
                    "description": "Optional timeout in seconds (default: 60)"
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

        match Self::run_command_impl(command, working_dir, timeout).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}