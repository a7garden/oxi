//! Write file tool

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::sync::oneshot;

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }

    async fn write_file_impl(path: &str, content: &str, append: bool) -> Result<String, ToolError> {
        let path = Path::new(path);

        // Security: prevent path traversal
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Cannot create parent directory: {}", e))?;
        }

        let bytes_written = if append {
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
                .map_err(|e| format!("Cannot open file: {}", e))?;
            use tokio::io::AsyncWriteExt;
            file.write_all(content.as_bytes())
                .await
                .map_err(|e| format!("Cannot write file: {}", e))?;
            content.len()
        } else {
            fs::write(path, content)
                .await
                .map_err(|e| format!("Cannot write file: {}", e))?;
            content.len()
        };

        Ok(format!("Wrote {} bytes to {}", bytes_written, path.display()))
    }
}

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn label(&self) -> &str {
        "Write File"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories as needed. Existing files will be overwritten. Use append=true to append to existing files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                },
                "append": {
                    "type": "boolean",
                    "description": "If true, append to existing file instead of overwriting",
                    "default": false
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let path = params
            .get("path")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let content = params
            .get("content")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let append = params
            .get("append")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        match Self::write_file_impl(path, content, append).await {
            Ok(msg) => Ok(AgentToolResult::success(msg)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}