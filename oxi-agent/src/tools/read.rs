//! Read file tool

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }

    async fn read_file_impl(path: &str) -> Result<String, ToolError> {
        let path = Path::new(path);

        // Security: prevent path traversal
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }

        // Check if file exists
        match fs::metadata(path).await {
            Ok(meta) if meta.is_dir() => {
                return Err("Cannot read a directory, use read_dir instead".to_string());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("File not found: {}", path.display()));
            }
            Err(e) => {
                return Err(format!("Cannot access file: {}", e));
            }
            _ => {}
        }

        // Read file content
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| format!("Cannot open file: {}", e))?;

        let mut content = String::new();
        file.read_to_string(&mut content)
            .await
            .map_err(|e| format!("Cannot read file: {}", e))?;

        Ok(content)
    }
}

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn label(&self) -> &str {
        "Read File"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the full file content as a string. Use this for reading source code, configs, or any text files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                }
            },
            "required": ["path"]
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

        match Self::read_file_impl(path).await {
            Ok(content) => Ok(AgentToolResult::success(content)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}