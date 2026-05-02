//! Grep tool - search files for patterns

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::sync::oneshot;

pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }

    /// Check if a filename matches a simple glob pattern like "*.rs", "*.ts"
    fn matches_glob(file_name: &str, pattern: &str) -> bool {
        if pattern.starts_with("*.") {
            let ext = &pattern[2..];
            file_name.ends_with(ext)
        } else if pattern.contains('*') {
            // Simple wildcard matching
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                file_name.starts_with(parts[0]) && file_name.ends_with(parts[1])
            } else {
                file_name == pattern
            }
        } else {
            file_name == pattern
        }
    }

    async fn grep_impl(
        pattern: &str,
        path: &str,
        case_insensitive: bool,
        include: Option<&str>,
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

        let re = RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| format!("Invalid regex pattern '{}': {}", pattern, e))?;

        let mut matches: Vec<String> = Vec::new();
        Self::grep_walk(root, root, &re, include, max_results, &mut matches).await?;

        if matches.is_empty() {
            Ok("No matches found".to_string())
        } else {
            let header = format!("Found {} matches:\n", matches.len());
            Ok(header + &matches.join("\n"))
        }
    }

    async fn grep_walk(
        root: &Path,
        current: &Path,
        re: &regex::Regex,
        include: Option<&str>,
        max_results: usize,
        matches: &mut Vec<String>,
    ) -> Result<(), ToolError> {
        if matches.len() >= max_results {
            return Ok(());
        }

        if current.is_file() {
            // Check include filter
            if let Some(glob) = include {
                let file_name = current
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !Self::matches_glob(&file_name, glob) {
                    return Ok(());
                }
            }

            // Try to read and search the file
            match fs::read_to_string(current).await {
                Ok(content) => {
                    let relative = current
                        .strip_prefix(root)
                        .unwrap_or(current)
                        .display();

                    for (i, line) in content.lines().enumerate() {
                        if matches.len() >= max_results {
                            break;
                        }
                        if re.is_match(line) {
                            matches.push(format!(
                                "{}:{}: {}",
                                relative,
                                i + 1,
                                line.trim_end()
                            ));
                        }
                    }
                }
                Err(_) => {
                    // Skip files we can't read (binary, permissions, etc.)
                }
            }
            return Ok(());
        }

        // Directory: walk entries
        let mut entries = fs::read_dir(current)
            .await
            .map_err(|e| format!("Cannot read directory {}: {}", current.display(), e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Error reading entry: {}", e))?
        {
            let entry_path = entry.path();

            // Skip hidden files/dirs
            if entry_path
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }

            // Skip common non-searchable dirs
            if entry_path.is_dir() {
                let dir_name = entry_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if matches!(
                    dir_name.as_str(),
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
            }

            Box::pin(Self::grep_walk(
                root,
                &entry_path,
                re,
                include,
                max_results,
                matches,
            ))
            .await?;
        }

        Ok(())
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn label(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "Search files for a regex pattern. Returns matching lines with file paths and line numbers. Searches recursively from the given path."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "The directory or file to search in",
                    "default": "."
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "If true, perform case-insensitive search",
                    "default": false
                },
                "include": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs', '*.ts')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 100
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let pattern = params
            .get("pattern")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: pattern".to_string())?;

        let path = params
            .get("path")
            .and_then(|v: &Value| v.as_str())
            .unwrap_or(".");

        let case_insensitive = params
            .get("case_insensitive")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        let include = params.get("include").and_then(|v: &Value| v.as_str());

        let max_results = params
            .get("max_results")
            .and_then(|v: &Value| v.as_u64())
            .unwrap_or(100) as usize;

        match Self::grep_impl(pattern, path, case_insensitive, include, max_results).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}
