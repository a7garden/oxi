/// Write file tool
/// Supports:
/// - Creating parent directories if they don't exist
/// - Append mode (append=true)
/// - Line count reporting
/// - Diff-style output preview (first/last few lines for large files)
/// - File mutation queue for serialized writes (concurrent safety)
/// - Output truncation for very large content
use super::file_mutation_queue::global_mutation_queue;
use super::path_security::PathGuard;
use super::truncate::{self, TruncationOptions};
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::oneshot;

/// Maximum number of lines to show in the diff-style output preview
const PREVIEW_HEAD_LINES: usize = 5;
const PREVIEW_TAIL_LINES: usize = 5;
/// Threshold above which we switch from full-content to head/tail preview display
const PREVIEW_THRESHOLD_LINES: usize = 20;

/// WriteTool.
pub struct WriteTool {
    root_dir: Option<PathBuf>,
}

impl WriteTool {
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

    /// Build a human-readable preview of the content that was written.
    /// For small files, shows everything. For large files, shows first/last few lines.
    fn build_content_preview(content: &str, total_lines: usize) -> String {
        if total_lines <= PREVIEW_THRESHOLD_LINES {
            return content.to_string();
        }

        let lines: Vec<&str> = content.lines().collect();
        let head: Vec<&str> = lines.iter().copied().take(PREVIEW_HEAD_LINES).collect();
        let tail: Vec<&str> = lines
            .iter()
            .copied()
            .rev()
            .take(PREVIEW_TAIL_LINES)
            .rev()
            .collect();

        let omitted = total_lines - PREVIEW_HEAD_LINES - PREVIEW_TAIL_LINES;

        format!(
            "{}\n\n... [{} lines omitted] ...\n\n{}",
            head.join("\n"),
            omitted,
            tail.join("\n")
        )
    }

    /// Core write implementation — runs inside the mutation queue lock.
    async fn write_file_impl(
        root_dir: &Path,
        path: &str,
        content: &str,
        append: bool,
    ) -> Result<String, ToolError> {
        // Security: validate path with PathGuard
        let guard = PathGuard::new(root_dir);
        let file_path = guard
            .validate_traversal(Path::new(path))
            .map_err(|e| e.to_string())?;

        // Ensure parent directory exists (create if missing)
        if let Some(parent) = file_path.parent() {
            // Only try to create if the parent is non-empty (e.g. not just "")
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("Cannot create parent directory: {}", e))?;
            }
        }

        // Check if file already existed before write (for reporting)
        let existed = file_path.exists();

        // Perform the write through the mutation queue for serialized access
        let content_owned = content.to_string();
        let result = global_mutation_queue()
            .with_queue(&file_path, || async {
                if append {
                    let mut file = tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&file_path)
                        .await
                        .map_err(|e| format!("Cannot open file for append: {}", e))?;
                    use tokio::io::AsyncWriteExt;
                    file.write_all(content_owned.as_bytes())
                        .await
                        .map_err(|e| format!("Cannot write file: {}", e))?;
                    file.flush()
                        .await
                        .map_err(|e| format!("Cannot flush file: {}", e))?;
                } else {
                    fs::write(&file_path, &content_owned)
                        .await
                        .map_err(|e| format!("Cannot write file: {}", e))?;
                }
                Ok::<(), ToolError>(())
            })
            .await;

        result?;

        let total_lines = content.lines().count();
        let total_bytes = content.len();
        let action = if append { "Appended" } else { "Wrote" };
        let status = if existed && !append {
            " (overwritten)"
        } else if append && existed {
            " (appended)"
        } else if !existed {
            " (new file)"
        } else {
            ""
        };

        // Build result with preview
        let preview = Self::build_content_preview(content, total_lines);

        // Truncate the preview if very large
        let truncation_opts = TruncationOptions {
            max_lines: Some(50),
            max_bytes: Some(4 * 1024),
        };
        let truncated = truncate::truncate_head(&preview, &truncation_opts);

        let mut msg = format!(
            "{} {} lines ({} bytes) to {}{}\n",
            action, total_lines, total_bytes, path, status
        );

        msg.push_str(&format!("--- Content Preview ---\n{}", truncated.content));

        if truncated.truncated {
            msg.push_str(&format!(
                "\n[Output truncated: {} total lines, {} total bytes]",
                truncated.total_lines, truncated.total_bytes
            ));
        }

        Ok(msg)
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

    fn essential(&self) -> bool {
        true
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
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let append = params
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Use root_dir if set, else ctx.root()
        let root = self.root_dir.as_deref().unwrap_or(ctx.root());

        let write_result = Self::write_file_impl(root, path, content, append).await;
        // Notify LSP provider so diagnostics refresh after the
        // file's contents change. Best-effort: a transient LSP
        // error must not fail the write.
        // Best-effort: resolve the path for LSP notification. The
        // `write_file_impl` already does the same PathGuard
        // validation; this is a read-only normalization for the
        // background task only.
        let notify_path = std::path::Path::new(path)
            .to_path_buf();
        let notify_abs = if notify_path.is_absolute() {
            notify_path
        } else {
            std::path::Path::new(root).join(&notify_path)
        };
        if write_result.is_ok()
            && let Some(provider) = ctx.lsp.as_ref()
        {
            let provider_clone = provider.clone();
            let abs_path_clone = notify_abs.clone();
            let content_owned = content.to_string();
            tokio::spawn(async move {
                provider_clone
                    .notify_file_changed(&abs_path_clone, &content_owned)
                    .await;
            });
        }
        match write_result {
            Ok(msg) => Ok(AgentToolResult::success(msg)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_build_content_preview_small() {
        let content = "line1\nline2\nline3";
        let preview = WriteTool::build_content_preview(content, 3);
        assert_eq!(preview, content);
    }

    #[test]
    fn test_build_content_preview_large() {
        let lines: Vec<String> = (1..=30).map(|i| format!("line {}", i)).collect();
        let content = lines.join("\n");
        let preview = WriteTool::build_content_preview(&content, 30);

        assert!(preview.contains("line 1"));
        assert!(preview.contains("line 5"));
        assert!(preview.contains("line 26"));
        assert!(preview.contains("line 30"));
        assert!(preview.contains("lines omitted"));
        assert!(!preview.contains("line 10")); // middle should be omitted
    }

    #[test]
    fn test_build_content_preview_exact_threshold() {
        let lines: Vec<String> = (1..=20).map(|i| format!("line {}", i)).collect();
        let content = lines.join("\n");
        let preview = WriteTool::build_content_preview(&content, 20);
        // At threshold, should show full content
        assert_eq!(preview, content);
    }

    #[test]
    fn test_build_content_preview_one_over_threshold() {
        let lines: Vec<String> = (1..=21).map(|i| format!("line {}", i)).collect();
        let content = lines.join("\n");
        let preview = WriteTool::build_content_preview(&content, 21);
        // Over threshold, should show head/tail
        assert!(preview.contains("lines omitted"));
    }

    #[tokio::test]
    async fn test_write_new_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        let path_str = path.to_str().unwrap();

        let result =
            WriteTool::write_file_impl(Path::new("."), path_str, "hello world\nline 2", false)
                .await;
        assert!(result.is_ok());

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "hello world\nline 2");

        let msg = result.unwrap();
        assert!(msg.contains("2 lines"));
        assert!(msg.contains("new file"));
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a/b/c/test.txt");
        let path_str = path.to_str().unwrap();

        let result =
            WriteTool::write_file_impl(Path::new("."), path_str, "deep nested", false).await;
        assert!(result.is_ok());

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "deep nested");
    }

    #[tokio::test]
    async fn test_write_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        let path_str = path.to_str().unwrap();

        // Create initial file
        std::fs::write(&path, "old content").unwrap();

        let result =
            WriteTool::write_file_impl(Path::new("."), path_str, "new content", false).await;
        assert!(result.is_ok());

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "new content");

        let msg = result.unwrap();
        assert!(msg.contains("overwritten"));
    }

    #[tokio::test]
    async fn test_write_append_mode() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        let path_str = path.to_str().unwrap();

        // Write initial content
        WriteTool::write_file_impl(Path::new("."), path_str, "line 1\n", false)
            .await
            .unwrap();

        // Append to it
        let result = WriteTool::write_file_impl(Path::new("."), path_str, "line 2\n", true).await;
        assert!(result.is_ok());

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "line 1\nline 2\n");

        let msg = result.unwrap();
        assert!(msg.contains("Appended"));
    }

    #[tokio::test]
    async fn test_write_append_to_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("new.txt");
        let path_str = path.to_str().unwrap();

        let result =
            WriteTool::write_file_impl(Path::new("."), path_str, "appended content", true).await;
        assert!(result.is_ok());

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "appended content");
    }

    #[tokio::test]
    async fn test_write_path_traversal_blocked() {
        let result =
            WriteTool::write_file_impl(Path::new("."), "../../etc/passwd", "hack", false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Path traversal"));
    }

    #[tokio::test]
    async fn test_write_empty_content() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.txt");
        let path_str = path.to_str().unwrap();

        let result = WriteTool::write_file_impl(Path::new("."), path_str, "", false).await;
        assert!(result.is_ok());

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "");

        let msg = result.unwrap();
        assert!(msg.contains("0 lines"));
    }

    #[tokio::test]
    async fn test_write_large_file_has_preview() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("large.txt");
        let path_str = path.to_str().unwrap();

        let lines: Vec<String> = (1..=100).map(|i| format!("line {}", i)).collect();
        let content = lines.join("\n");

        let result = WriteTool::write_file_impl(Path::new("."), path_str, &content, false).await;
        assert!(result.is_ok());

        let msg = result.unwrap();
        assert!(msg.contains("100 lines"));
        assert!(msg.contains("Content Preview"));
    }

    #[tokio::test]
    async fn test_execute_via_tool_trait() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trait_test.txt");
        let path_str = path.to_str().unwrap().to_string();

        let tool = WriteTool::new();
        let params = json!({
            "path": path_str,
            "content": "via trait"
        });

        let result = tool
            .execute("test-id", params, None, &ToolContext::default())
            .await;
        assert!(result.is_ok());
        let tool_result = result.unwrap();
        assert!(tool_result.success);
        assert!(tool_result.output.contains("via trait"));

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "via trait");
    }

    #[tokio::test]
    async fn test_execute_missing_path_param() {
        let tool = WriteTool::new();
        let params = json!({
            "content": "no path"
        });

        let result = tool
            .execute("test-id", params, None, &ToolContext::default())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("path"));
    }

    #[tokio::test]
    async fn test_execute_missing_content_param() {
        let tool = WriteTool::new();
        let params = json!({
            "path": "/tmp/test.txt"
        });

        let result = tool
            .execute("test-id", params, None, &ToolContext::default())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("content"));
    }

    #[tokio::test]
    async fn test_execute_append_via_trait() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("append_trait.txt");
        let path_str = path.to_str().unwrap().to_string();

        let tool = WriteTool::new();

        // First write
        let params = json!({
            "path": &path_str,
            "content": "first "
        });
        tool.execute("test-id-1", params, None, &ToolContext::default())
            .await
            .unwrap();

        // Append
        let params = json!({
            "path": &path_str,
            "content": "second",
            "append": true
        });
        let result = tool
            .execute("test-id-2", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Appended"));

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "first second");
    }

    #[test]
    fn test_default_impl() {
        let tool = WriteTool::default();
        assert_eq!(tool.name(), "write");
        assert_eq!(tool.label(), "Write File");
    }

    #[test]
    fn test_parameters_schema_required_fields() {
        let tool = WriteTool::new();
        let schema = tool.parameters_schema();
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.contains(&json!("path")));
        assert!(required.contains(&json!("content")));
        assert!(!required.contains(&json!("append"))); // append is optional
    }
}
