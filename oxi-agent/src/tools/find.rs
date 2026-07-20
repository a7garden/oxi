use super::path_security::PathGuard;
/// Find tool - find files by name or pattern
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use glob::Pattern;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::oneshot;
use crate::tools::typed::TypedTool;

/// Typed arguments for [`FindTool`].
#[derive(Deserialize, JsonSchema)]
pub struct FindArgs {
    path: String,
    name: Option<String>,
    #[serde(rename = "type")]
    file_type: Option<String>,
    max_depth: Option<usize>,
    #[serde(default = "default_find_results")]
    max_results: usize,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    follow_symlinks: bool,
}

fn default_find_results() -> usize { 100 }

/// FindTool.
pub struct FindTool {
    root_dir: Option<PathBuf>,
}

impl FindTool {
    /// Create with no explicit root (uses ToolContext.workspace_dir at runtime).
    pub fn new() -> Self {
        Self { root_dir: None }
    }

    /// Create with a specific working directory (overrides ToolContext).
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            root_dir: Some(cwd),
        }
    }

    /// Check if a filename matches a simple glob pattern
    fn matches_pattern(file_name: &str, pattern: &str) -> bool {
        if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            match parts.len() {
                1 => file_name == parts[0],
                2 => {
                    let (prefix, suffix) = (parts[0], parts[1]);
                    // Handle patterns that can match the entire string
                    // e.g., "*_test.txt" matches "test.txt"
                    if prefix.is_empty() {
                        file_name.ends_with(suffix)
                    } else if suffix.is_empty() {
                        file_name.starts_with(prefix)
                    } else {
                        file_name.starts_with(prefix) && file_name.ends_with(suffix)
                    }
                }
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

    /// Check if a path matches any of the exclude patterns
    fn matches_exclude(path: &Path, patterns: &[String]) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in patterns {
            // Try to match as glob pattern
            if let Ok(glob) = Pattern::new(pattern) {
                // Check full path match
                if glob.matches(&path_str) {
                    return true;
                }
                // Also check just the filename
                if let Some(file_name) = path.file_name()
                    && glob.matches(&file_name.to_string_lossy())
                {
                    return true;
                }
                // Check with directory prefix pattern (e.g., "node_modules")
                if path_str.contains(pattern) {
                    return true;
                }
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    async fn find_impl(
        root_dir: &Path,
        path: &str,
        name: Option<&str>,
        file_type: Option<&str>,
        max_depth: Option<usize>,
        max_results: usize,
        exclude: &[String],
        follow_symlinks: bool,
    ) -> Result<String, ToolError> {
        // Security: validate path with PathGuard
        let guard = PathGuard::new(root_dir);
        let root = guard
            .validate_traversal(Path::new(path))
            .map_err(|e| e.to_string())?;

        if !root.is_dir() {
            return Err(format!("Path is not a directory: {}", path));
        }

        let mut results: Vec<String> = Vec::new();
        Self::find_walk(
            &root,
            &root,
            name,
            file_type,
            max_depth,
            0,
            &mut results,
            max_results,
            exclude,
            follow_symlinks,
        )
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
        exclude: &[String],
        follow_symlinks: bool,
    ) -> Result<(), ToolError> {
        if results.len() >= max_results {
            return Ok(());
        }

        // Check depth limit
        if let Some(max) = max_depth
            && current_depth > max
        {
            return Ok(());
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
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden entries (unless explicitly excluded)
            if file_name.starts_with('.') {
                continue;
            }

            let metadata = entry
                .metadata()
                .await
                .map_err(|e| format!("Cannot read metadata: {}", e))?;

            // Handle symlinks
            let is_symlink = metadata.file_type().is_symlink();
            let (is_dir, is_file) = if is_symlink && follow_symlinks {
                // Follow symlink to determine actual type
                match fs::metadata(&entry_path).await {
                    Ok(meta) => (meta.is_dir(), meta.is_file()),
                    Err(_) => (false, metadata.is_file()),
                }
            } else if is_symlink {
                // Don't follow symlinks - skip them
                continue;
            } else {
                (metadata.is_dir(), metadata.is_file())
            };

            // Check exclude patterns
            if Self::matches_exclude(&entry_path, exclude) {
                // If it's a directory, skip descending into it
                if is_dir {
                    continue;
                }
                // If it's a file, skip it entirely
                continue;
            }

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
                // Skip common non-searchable dirs unless excluded
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
                ) && !Self::matches_exclude(&entry_path, exclude)
                {
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
                    exclude,
                    follow_symlinks,
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

    fn essential(&self) -> bool {
        true
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
                },
                "exclude": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "Array of glob patterns to exclude (e.g., ['*.log', 'temp/**', '.git'])",
                    "default": []
                },
                "follow_symlinks": {
                    "type": "boolean",
                    "description": "Whether to follow symbolic links",
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
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: FindArgs = serde_json::from_value(params)
            .map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, ctx).await
    }
}

#[async_trait]
impl TypedTool for FindTool {
    type Args = FindArgs;
    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let path = &args.path;
        if let Some(ref resolver) = ctx.url_resolver && resolver.can_resolve(path) {
            return Ok(AgentToolResult::error("find does not support internal URLs. Use grep for searching URL content."));
        }
        let root = self.root_dir.as_deref().unwrap_or(ctx.root());
        match Self::find_impl(root, path, args.name.as_deref(), args.file_type.as_deref(), args.max_depth, args.max_results, &args.exclude, args.follow_symlinks).await {
            Ok(output) => Ok(AgentToolResult::success(output)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_pattern_simple() {
        assert!(FindTool::matches_pattern("test.rs", "test.rs"));
        assert!(!FindTool::matches_pattern("test.txt", "test.rs"));
    }

    #[test]
    fn test_matches_pattern_single_wildcard() {
        assert!(FindTool::matches_pattern("test.rs", "*.rs"));
        assert!(FindTool::matches_pattern("example.txt", "*.txt"));
        assert!(!FindTool::matches_pattern("test.rs", "*.txt"));
    }

    #[test]
    fn test_matches_pattern_prefix() {
        assert!(FindTool::matches_pattern("test_file.rs", "test_*"));
        assert!(FindTool::matches_pattern("test_file", "test_*"));
    }

    #[test]
    fn test_matches_pattern_suffix() {
        // *_test.txt matches files ending with _test.txt
        assert!(FindTool::matches_pattern("file_test.txt", "*_test.txt"));
        assert!(FindTool::matches_pattern("my_test.txt", "*_test.txt"));
        // test.txt does NOT match *_test.txt because it ends with .txt, not _test.txt
        assert!(!FindTool::matches_pattern("test.txt", "*_test.txt"));
    }

    #[test]
    fn test_matches_pattern_multi_wildcard() {
        assert!(FindTool::matches_pattern(
            "test_file_backup.txt",
            "test*backup.txt"
        ));
        assert!(FindTool::matches_pattern(
            "abcxyzbackup.txt",
            "abc*xyz*backup.txt"
        ));
    }

    #[test]
    fn test_matches_exclude() {
        let patterns = vec![
            "*.log".to_string(),
            "*.tmp".to_string(),
            "node_modules".to_string(),
        ];

        let path = Path::new("debug.log");
        assert!(FindTool::matches_exclude(path, &patterns));

        let path = Path::new("temp.tmp");
        assert!(FindTool::matches_exclude(path, &patterns));

        let path = Path::new("/path/to/node_modules/file.txt");
        assert!(FindTool::matches_exclude(path, &patterns));

        let path = Path::new("source.rs");
        assert!(!FindTool::matches_exclude(path, &patterns));
    }
}
