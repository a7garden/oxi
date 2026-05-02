//! Find tool - find files by name or pattern

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::sync::oneshot;

pub struct FindTool;

impl FindTool {
    pub fn new() -> Self {
        Self
    }

    /// Check if a filename matches a simple glob pattern
    fn matches_pattern(file_name: &str, pattern: &str) -> bool {
        if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            match parts.len() {
                1 => file_name == parts[0],
                2 => file_name.starts_with(parts[0]) && file_name.ends_with(parts[1]),
                _ => {
                    // Multi-wildcard: simple sequential matching
                    let mut idx = 0;
                    for (i, part) in parts.iter().enumerate() {
                        if part.is_empty() {
                            continue;
                        }
                        match file_name[idx..].find(part) {
                            Some(pos) => {
                                if i == 0 && pos != 0 {
                                    return false;
                                }
                                idx += pos + part.len();
                            }
                            None => return false,
                        }
                    }
                    if let Some(last) = parts.last() {
                        if !last.is_empty() {
                            file_name.ends_with(last)
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                }
            }
        } else {
            file_name == pattern
        }
    }

    async fn find_impl(
        path: &str,
        name: Option<&str>,
        file_type: Option<&str>,
        max_depth: Option<usize>,
        max_results: usize,
    ) -> Result<String, ToolError> {
        let root = Path::new(path);

        // Security: prevent path traversal
        if root.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }

        if !root.exists() {
            return Err(format!("Path not found: {}", path));
        }

        if !root.is_dir() {
            return Err(format!("Path is not a directory: {}", path));
        }

        let mut results: Vec<String> = Vec::new();
        Self::find_walk(root, root, name, file_type, max_depth, 0, &mut results, max_results)
            .await?;

        if results.is_empty() {
            Ok("No files found".to_string())
        } else {
            let header = format!("Found {} results:\n", results.len());
            Ok(header + &results.join("\n"))
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn find_walk(
        root: &Path,
        current: &Path,
        name: Option<&str>,
        file_type: Option<&str>,
        max_depth: Option<usize>,
        current_depth: usize,
        results: &mut Vec<String>,
        max_results: usize,
    ) -> Result<(), ToolError> {
        if results.len() >= max_results {
            return Ok(());
        }

        // Check depth limit
        if let Some(max) = max_depth {
            if current_depth > max {
                return Ok(());
            }
        }

        let mut entries = fs::read_dir(current)
            .await
            .map_err(|e| format!("Cannot read directory {}: {}", current.display(), e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Error reading entry: {}", e))?
        {
            if results.len() >= max_results {
                return Ok(());
            }

            let entry_path = entry.path();
            let file_name = entry
                .file_name()
                .to_string_lossy()
                .to_string();

            // Skip hidden entries
            if file_name.starts_with('.') {
                continue;
            }

            let metadata = entry
                .metadata()
                .await
                .map_err(|e| format!("Cannot read metadata: {}", e))?;

            let is_dir = metadata.is_dir();
            let is_file = metadata.is_file();

            // Apply type filter
            let type_match = match file_type {
                Some("file") => is_file,
                Some("dir" | "directory") => is_dir,
                _ => true, // "all" or None
            };

            // Apply name filter
            let name_match = match name {
                Some(pattern) => Self::matches_pattern(&file_name, pattern),
                None => true,
            };

            if type_match && name_match {
                let relative = entry_path
                    .strip_prefix(root)
                    .unwrap_or(&entry_path)
                    .display();
                let suffix = if is_dir { "/" } else { "" };
                results.push(format!("{}{}", relative, suffix));
            }

            // Recurse into directories
            if is_dir {
                // Skip common non-searchable dirs
                if matches!(
                    file_name.as_str(),
                    "node_modules"
                        | "target"
                        | ".git"
                        | "dist"
                        | "build"
                        | "__pycache__"
                        | ".venv"
                        | "venv"
                ) {
                    continue;
                }

                Box::pin(Self::find_walk(
                    root,
                    &entry_path,
                    name,
                    file_type,
                    max_depth,
                    current_depth + 1,
                    results,
                    max_results,
                ))
                .await?;
            }
        }

        Ok(())
    }
}

impl Default for FindTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn label(&self) -> &str {
        "Find"
    }

    fn description(&self) -> &str {
        "Find files and directories by name pattern and type. Searches recursively from the given path."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory to search in",
                    "default": "."
                },
                "name": {
                    "type": "string",
                    "description": "Glob pattern to match file names (e.g., '*.rs', 'test_*.py')"
                },
                "type": {
                    "type": "string",
                    "description": "Filter by type: 'file', 'dir', or 'all'",
                    "enum": ["file", "dir", "all"],
                    "default": "all"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth to search"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 100
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

        let name = params.get("name").and_then(|v: &Value| v.as_str());
        let file_type = params.get("type").and_then(|v: &Value| v.as_str());
        let max_depth = params.get("max_depth").and_then(|v: &Value| v.as_u64()).map(|d| d as usize);
        let max_results = params
            .get("max_results")
            .and_then(|v: &Value| v.as_u64())
            .unwrap_or(100) as usize;

        match Self::find_impl(path, name, file_type, max_depth, max_results).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}
