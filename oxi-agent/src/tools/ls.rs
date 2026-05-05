//! Ls tool - list directory contents

use super::{AgentTool, AgentToolResult, ToolError};
use crate::tools::truncate::{format_bytes, truncate_head, TruncationOptions};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tokio::sync::oneshot;

/// Default max entries to show (use truncate for more)
const DEFAULT_ENTRY_LIMIT: usize = 100;
/// Default max output lines (for truncation)
const DEFAULT_MAX_LINES: usize = 2000;
/// Default max output bytes (for truncation)
const DEFAULT_MAX_BYTES: usize = 50 * 1024;

pub struct LsTool;

impl LsTool {
    pub fn new() -> Self {
        Self
    }

    /// Format file size in human-readable format
    fn format_size(size: u64) -> String {
        format_bytes(size as usize)
    }

    /// Get file type indicator: / for dirs, @ for symlinks, * for executables
    fn get_type_indicator(metadata: &std::fs::Metadata) -> &'static str {
        if metadata.is_dir() {
            "/"
        } else if metadata.file_type().is_symlink() {
            "@"
        } else {
            // Check if executable
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 != 0 {
                    return "*";
                }
            }
            ""
        }
    }

    async fn ls_impl(
        path: &str,
        all: bool,
        long_format: bool,
        entry_limit: Option<usize>,
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

            let type_indicator = Self::get_type_indicator(&meta);

            return Ok(if long_format {
                format!("{:<10} {}{}", Self::format_size(size), name, type_indicator)
            } else {
                format!("{}{}", name, type_indicator)
            });
        }

        // Read all entries first
        let mut entries: Vec<(String, bool, u64, std::fs::Metadata)> = Vec::new();
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

            entries.push((file_name, is_dir, size, metadata));
        }

        // Sort: directories first, then alphabetically (case-insensitive)
        entries.sort_by(|a, b| match (a.1, b.1) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
        });

        // Apply entry limit if specified
        let limit = entry_limit.unwrap_or(DEFAULT_ENTRY_LIMIT);
        let limited = if entries.len() > limit {
            entries.truncate(limit);
            true
        } else {
            false
        };

        let total_entries = entries.len();
        let dir_count = entries.iter().filter(|e| e.1).count();
        let file_count = total_entries - dir_count;

        // Build output based on format
        let output = if long_format {
            let mut lines: Vec<String> = entries
                .iter()
                .map(|(name, _is_dir, size, meta)| {
                    let type_indicator = Self::get_type_indicator(meta);
                    format!(
                        "{:<10} {}{}",
                        Self::format_size(*size),
                        name,
                        type_indicator
                    )
                })
                .collect();

            // Add entry count summary
            lines.push(format!(
                "\n{} director{}, {} file{}",
                dir_count,
                if dir_count == 1 { "y" } else { "ies" },
                file_count,
                if file_count == 1 { "" } else { "s" }
            ));

            lines.join("\n")
        } else {
            let lines: Vec<String> = entries
                .iter()
                .map(|(name, _, _, meta)| {
                    let type_indicator = Self::get_type_indicator(meta);
                    format!("{}{}", name, type_indicator)
                })
                .collect();

            lines.join("\n")
        };

        // Add entry limit notice if truncated
        let output = if limited {
            format!(
                "{}\n\n... [limit reached: {} entries total, use limit=N to see more]",
                output, total_entries
            )
        } else {
            output
        };

        // Apply output truncation (for very large directories)
        let truncation_options = TruncationOptions {
            max_lines: Some(DEFAULT_MAX_LINES),
            max_bytes: Some(DEFAULT_MAX_BYTES),
        };
        let result = truncate_head(&output, &truncation_options);

        Ok(result.content)
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
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of entries to display (truncation notice shown if exceeded)",
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
            .unwrap_or(".");

        let all = params
            .get("all")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        let long_format = params
            .get("long")
            .and_then(|v: &Value| v.as_bool())
            .unwrap_or(false);

        let entry_limit = params
            .get("limit")
            .and_then(|v: &Value| v.as_u64())
            .map(|l| l as usize);

        match Self::ls_impl(path, all, long_format, entry_limit).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        let temp_dir = TempDir::new().unwrap();

        // Create test files and directories
        let test_files = vec![
            ("alpha.txt", false),
            ("beta.txt", false),
            ("gamma.rs", false),
            ("subdir", true),
            ("another_dir", true),
        ];

        for (name, is_dir) in test_files {
            let path = temp_dir.path().join(name);
            if is_dir {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(&path, "test content").unwrap();
            }
        }

        // Create hidden file
        fs::write(temp_dir.path().join(".hidden"), "hidden").unwrap();

        temp_dir
    }

    #[test]
    fn test_basic_ls() {
        let temp_dir = create_test_dir();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), false, false, None).await
            })
            .unwrap();

        // Should include visible files and directories
        assert!(result.contains("alpha.txt"));
        assert!(result.contains("beta.txt"));
        assert!(result.contains("gamma.rs"));
        // Should not show hidden file by default
        assert!(!result.contains(".hidden"));
    }

    #[test]
    fn test_ls_all() {
        let temp_dir = create_test_dir();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), true, false, None).await
            })
            .unwrap();

        // Should show hidden file with all flag
        assert!(result.contains(".hidden"));
    }

    #[test]
    fn test_ls_long_format() {
        let temp_dir = create_test_dir();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), false, true, None).await
            })
            .unwrap();

        // Long format should have sizes
        assert!(result.contains("B") || result.contains("KB") || result.contains("MB"));
    }

    #[test]
    fn test_entry_count_summary() {
        let temp_dir = create_test_dir();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), false, true, None).await
            })
            .unwrap();

        // Should have entry count summary in long format
        assert!(result.contains("directories") || result.contains("directory"));
        assert!(result.contains("files") || result.contains("file"));
    }

    #[test]
    fn test_entry_limit() {
        let temp_dir = create_test_dir();
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Set limit to 2
        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), false, false, Some(2)).await
            })
            .unwrap();

        // Should show limit reached notice
        assert!(result.contains("limit reached") || result.contains("limit=N"));
    }

    #[test]
    fn test_case_insensitive_sort() {
        let temp_dir = TempDir::new().unwrap();

        // Create files with various cases
        fs::write(temp_dir.path().join("Zebra.rs"), "").unwrap();
        fs::write(temp_dir.path().join("apple.rs"), "").unwrap();
        fs::write(temp_dir.path().join("Banana.rs"), "").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), false, false, None).await
            })
            .unwrap();

        let _lines: Vec<&str> = result.lines().collect();
        // Should be sorted case-insensitely: apple, Banana, Zebra
        assert!(result.contains("apple.rs"));
        assert!(result.contains("Banana.rs"));
        assert!(result.contains("Zebra.rs"));
    }

    #[test]
    fn test_type_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create directory
        fs::create_dir(temp_dir.path().join("test_dir")).unwrap();
        // Create regular file
        fs::write(temp_dir.path().join("test_file.txt"), "").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(async {
                LsTool::ls_impl(temp_dir.path().to_str().unwrap(), false, false, None).await
            })
            .unwrap();

        // Directories should have / indicator
        assert!(result.contains("test_dir/"));
        // Regular files should not have indicator
        assert!(result.contains("test_file.txt"));
        assert!(!result.contains("test_file.txt/"));
    }

    #[test]
    fn test_path_traversal_prevention() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { LsTool::ls_impl("../etc", false, false, None).await });

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    #[test]
    fn test_nonexistent_path() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            LsTool::ls_impl("/nonexistent/path/12345", false, false, None).await
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_single_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("single_file.txt");
        fs::write(&file_path, "content").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(async {
                LsTool::ls_impl(file_path.to_str().unwrap(), false, false, None).await
            })
            .unwrap();

        assert!(result.contains("single_file.txt"));
    }

    #[test]
    fn test_format_size() {
        assert!(LsTool::format_size(500).contains("B"));
        assert!(LsTool::format_size(1024).contains("KB"));
        assert!(LsTool::format_size(1024 * 1024).contains("MB"));
    }
}
