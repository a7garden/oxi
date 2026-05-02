//! Edit file tool - make targeted edits to files

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::sync::oneshot;

pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }

    async fn apply_edit_impl(
        path: &str,
        old_text: &str,
        new_text: &str,
        dry_run: bool,
    ) -> Result<String, ToolError> {
        let path = Path::new(path);

        // Security: prevent path traversal
        if path.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }

        // Read current content
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| format!("Cannot read file: {}", e))?;

        // Find the text to replace
        if !content.contains(old_text) {
            return Err(format!(
                "Text to replace not found in file. Make sure to match the exact text including whitespace and newlines."
            ));
        }

        if dry_run {
            let new_content = content.replace(old_text, new_text);
            let diff = if new_content == content {
                "No changes needed".to_string()
            } else {
                format!(
                    "Would change {} bytes ({} -> {})",
                    content.len(),
                    old_text.len(),
                    new_text.len()
                )
            };
            return Ok(diff);
        }

        // Apply the edit
        let new_content = content.replace(old_text, new_text);

        fs::write(path, &new_content)
            .await
            .map_err(|e| format!("Cannot write file: {}", e))?;

        Ok(format!(
            "Applied edit: replaced {} bytes with {} bytes",
            old_text.len(),
            new_text.len()
        ))
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn label(&self) -> &str {
        "Edit File"
    }

    fn description(&self) -> &str {
        "Make a targeted edit to a file. Replace old_text with new_text. Make sure old_text is an exact match including whitespace. Use dry_run=true to preview without making changes."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to replace (must match exactly, including whitespace)"
                },
                "new_text": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, preview the change without applying it",
                    "default": false
                }
            },
            "required": ["path", "old_text", "new_text"]
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

        let old_text = params
            .get("old_text")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: old_text".to_string())?;

        let new_text = params
            .get("new_text")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: new_text".to_string())?;

        let dry_run = params
            .get("dry_run")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        match Self::apply_edit_impl(path, old_text, new_text, dry_run).await {
            Ok(msg) => Ok(AgentToolResult::success(msg)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}