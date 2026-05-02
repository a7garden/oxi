//! Ls tool - list directory contents

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::sync::oneshot;

pub struct LsTool;

impl LsTool {
    pub fn new() -> Self {
        Self
    }

    async fn ls_impl(
        path: &str,
        all: bool,
        long_format: bool,
    ) -> Result<String, ToolError> {
        let dir_path = Path::new(path);

        // Security: prevent path traversal
        if dir_path.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }

        if !dir_path.exists() {
            return Err(format!("Path not found: {}", path));
        }

        if !dir_path.is_dir() {
            // If it's a file, just return its info
            let meta = fs::metadata(dir_path)
                .await
                .map_err(|e| format!("Cannot read metadata: {}", e))?;
            let size = meta.len();
            let name = dir_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            return Ok(if long_format {
                format!("{:<10} {}", size, name)
            } else {
                name
            });
        }

        let mut entries: Vec<(String, bool, u64)> = Vec::new(); // (name, is_dir, size)
        let mut dir = fs::read_dir(dir_path)
            .await
            .map_err(|e| format!("Cannot read directory: {}", e))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| format!("Error reading entry: {}", e))?
        {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless --all
            if !all && file_name.starts_with('.') {
                continue;
            }

            let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
            let is_dir = metadata.is_dir();
            let size = metadata.len();

            entries.push((file_name, is_dir, size));
        }

        // Sort: directories first, then alphabetically
        entries.sort_by(|a, b| {
            match (a.1, b.1) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
            }
        });

        if long_format {
            let mut lines: Vec<String> = entries
                .iter()
                .map(|(name, is_dir, size)| {
                    let type_indicator = if *is_dir { "/" } else { "" };
                    format!("{:<10} {}{}", size, name, type_indicator)
                })
                .collect();

            let dir_count = entries.iter().filter(|e| e.1).count();
            let file_count = entries.len() - dir_count;
            lines.push(format!(
                "\n{} director{}, {} file{}",
                dir_count,
                if dir_count == 1 { "y" } else { "ies" },
                file_count,
                if file_count == 1 { "" } else { "s" }
            ));

            Ok(lines.join("\n"))
        } else {
            let lines: Vec<String> = entries
                .iter()
                .map(|(name, is_dir, _)| {
                    if *is_dir {
                        format!("{}/", name)
                    } else {
                        name.clone()
                    }
                })
                .collect();

            Ok(lines.join("\n"))
        }
    }
}

impl Default for LsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn label(&self) -> &str {
        "Ls"
    }

    fn description(&self) -> &str {
        "List directory contents. Shows files and subdirectories with optional details."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory to list",
                    "default": "."
                },
                "all": {
                    "type": "boolean",
                    "description": "If true, show hidden files (starting with .)",
                    "default": false
                },
                "long": {
                    "type": "boolean",
                    "description": "If true, show detailed listing with file sizes",
                    "default": false
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
            .unwrap_or(".");

        let all = params
            .get("all")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        let long_format = params
            .get("long")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        match Self::ls_impl(path, all, long_format).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}
