/// Read file tool
/// Reads file contents with support for:
/// - Text files with line numbers, offset/limit, and truncation
/// - Image files (jpg/png/gif/webp) returned as base64-encoded content blocks
/// - Binary file detection

use super::path_security::PathGuard;
use super::truncate::{self, TruncationOptions};
use super::{AgentTool, AgentToolResult, ProgressCallback, ToolError};
use async_trait::async_trait;
use base64::Engine;
use oxi_ai::{ContentBlock, ImageContent, TextContent};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs;
use tokio::io::AsyncReadExt;

/// Maximum bytes to read for binary detection
const BINARY_DETECT_BYTES: usize = 8192;

/// Supported image extensions and their MIME types
const IMAGE_EXTENSIONS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// ReadTool.
pub struct ReadTool {
    root_dir: PathBuf,
    progress_callback: Arc<Mutex<Option<ProgressCallback>>>,
}

impl ReadTool {
/// Create with current directory as root.
    pub fn new() -> Self {
        Self::with_cwd(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Create with a specific working directory.
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            root_dir: cwd,
            progress_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// Determine if a file extension corresponds to a supported image type.
    /// Returns the MIME type if it's a supported image.
    fn image_mime_type(path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        IMAGE_EXTENSIONS
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|(_, mime)| *mime)
    }

    /// Check if data appears to be binary by looking for null bytes in the first chunk.
    fn is_binary(data: &[u8]) -> bool {
        data.contains(&0)
    }

    /// Read an image file and return it as a base64-encoded content block.
    async fn read_image(
        path: &Path,
        progress_cb: &Option<ProgressCallback>,
    ) -> Result<AgentToolResult, ToolError> {
        let display_path = path.display();

        if let Some(cb) = progress_cb {
            cb(format!("Reading image: {}", display_path));
        }

        let data = fs::read(path)
            .await
            .map_err(|e| format!("Cannot read image file: {}", e))?;

        if let Some(cb) = progress_cb {
            cb(format!("Read {} bytes, encoding as base64", data.len()));
        }

        let mime_type = Self::image_mime_type(path).unwrap_or("application/octet-stream");
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);

        // Build a text summary and an image content block
        let summary = format!(
            "Image file: {} ({} bytes, {})",
            display_path,
            data.len(),
            mime_type
        );

        let image_block = ContentBlock::Image(ImageContent::new(encoded, mime_type));
        let text_block = ContentBlock::Text(TextContent::new(summary.clone()));

        Ok(AgentToolResult::success(summary).with_content_blocks(vec![text_block, image_block]))
    }

    /// Read a text file with optional offset/limit, line numbers, and truncation.
    async fn read_text(
        path: &Path,
        offset: Option<usize>,
        limit: Option<usize>,
        progress_cb: &Option<ProgressCallback>,
    ) -> Result<AgentToolResult, ToolError> {
        let display_path = path.display();

        // Check file metadata
        let file_size = match fs::metadata(path).await {
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("File not found: {}", display_path));
            }
            Err(e) => {
                return Err(format!("Cannot access file: {}", e));
            }
        };

        if let Some(cb) = progress_cb {
            cb(format!(
                "Reading file: {} ({} bytes)",
                display_path, file_size
            ));
        }

        // Open and read file
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| format!("Cannot open file: {}", e))?;

        // Read a chunk for binary detection
        let mut detect_buf = vec![0u8; BINARY_DETECT_BYTES.min(file_size as usize)];
        let n = file
            .read(&mut detect_buf)
            .await
            .map_err(|e| format!("Cannot read file: {}", e))?;

        if Self::is_binary(&detect_buf[..n]) {
            return Ok(AgentToolResult::error(format!(
                "File appears to be binary: {} ({} bytes). Cannot display as text.",
                display_path, file_size
            )));
        }

        // Now read the full content: what we already read + the rest
        let mut content = String::from_utf8_lossy(&detect_buf[..n]).into_owned();
        let mut buffer = vec![0u8; 8192];
        loop {
            let n = file
                .read(&mut buffer)
                .await
                .map_err(|e| format!("Cannot read file: {}", e))?;
            if n == 0 {
                break;
            }
            content.push_str(&String::from_utf8_lossy(&buffer[..n]));
        }

        if let Some(cb) = progress_cb {
            cb(format!("Completed reading {} bytes", content.len()));
        }

        // Split into lines for offset/limit/numbering
        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len();

        // Apply offset (1-indexed) and limit
        let start_idx = offset
            .map(|o| if o == 0 { 0 } else { o - 1 }) // Convert 1-indexed to 0-indexed
            .unwrap_or(0);

        if start_idx >= total_lines && total_lines > 0 {
            return Ok(AgentToolResult::error(format!(
                "Offset {} exceeds file length ({} lines). Use offset=1 to {}.",
                offset.unwrap_or(1),
                total_lines,
                total_lines
            )));
        }

        let effective_limit = limit.unwrap_or(usize::MAX);
        let end_idx = if effective_limit > total_lines - start_idx {
            total_lines
        } else {
            start_idx + effective_limit
        };
        let selected_lines = &all_lines[start_idx..end_idx];
        let selected_count = selected_lines.len();

        // Apply truncation if no explicit limit was provided
        let (output_lines, truncated) = if limit.is_none() {
            let trunc_opts = TruncationOptions::default();
            let max_lines = trunc_opts.max_lines.unwrap_or(truncate::DEFAULT_MAX_LINES);
            let max_bytes = trunc_opts.max_bytes.unwrap_or(truncate::DEFAULT_MAX_BYTES);

            // Count bytes as we add lines
            let mut byte_count: usize = 0;
            let mut line_count: usize = 0;
            for line in selected_lines {
                // line number prefix + content + newline
                let prefix_len = format!("{}", start_idx + line_count + 1).len() + 2; // "  " separator
                byte_count += prefix_len + line.len() + 1;
                if line_count >= max_lines || byte_count > max_bytes {
                    break;
                }
                line_count += 1;
            }

            if line_count < selected_count {
                (line_count, true)
            } else {
                (selected_count, false)
            }
        } else {
            (selected_count, false)
        };

        // Build numbered output
        let mut output = String::new();
        for (i, line) in selected_lines.iter().enumerate().take(output_lines) {
            let line_num = start_idx + i + 1; // 1-indexed
            output.push_str(&format!("{:>6}\t{}", line_num, line));
            if i < output_lines - 1 || !content.ends_with('\n') {
                output.push('\n');
            }
        }

        // Add truncation notice
        if truncated {
            let next_offset = start_idx + output_lines + 1;
            output.push_str(&format!(
                "\n... [truncated: {} of {} lines shown. Use offset={} to continue]",
                output_lines,
                total_lines - start_idx,
                next_offset
            ));
        }

        // If offset was used, add context header
        if start_idx > 0 {
            output = format!(
                "Showing lines {}-{} of {}:\n",
                start_idx + 1,
                start_idx + output_lines,
                total_lines
            ) + &output;
        }

        Ok(AgentToolResult::success(output))
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

    fn essential(&self) -> bool { true }
    fn description(&self) -> &str {
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp). Images are sent as attachments. For text files, output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When reading with offset, line numbering starts from 1."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative or absolute)"
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let path_str = params
            .get("path")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let offset = params
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        // Security: validate path with PathGuard
        let guard = PathGuard::new(&self.root_dir);
        let validated = guard.validate_traversal(Path::new(path_str))
            .map_err(|e| e.to_string())?;
        let path = validated.as_path();

        // Check if path exists and is a directory
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

        let progress_cb = self.progress_callback.lock().expect("progress callback lock poisoned").clone();

        // Check if it's an image file
        if Self::image_mime_type(path).is_some() {
            return Self::read_image(path, &progress_cb).await;
        }

        // Otherwise, read as text
        Self::read_text(path, offset, limit, &progress_cb).await
    }

    fn on_progress(&self, callback: ProgressCallback) {
        let cb = self.progress_callback.clone();
        let mut guard = cb.lock().expect("progress callback lock poisoned");
        *guard = Some(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::NamedTempFile;

    fn make_text_file(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[tokio::test]
    async fn test_read_simple_text() {
        let f = make_text_file("hello\nworld\n");
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
        assert!(result.output.contains("world"));
    }

    #[tokio::test]
    async fn test_read_with_line_numbers() {
        let f = make_text_file("line1\nline2\nline3\n");
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        // Should contain line numbers
        assert!(result.output.contains("1"));
        assert!(result.output.contains("2"));
        assert!(result.output.contains("3"));
        // Should contain tab-separated line numbers
        assert!(result.output.contains("\tline1"));
        assert!(result.output.contains("\tline2"));
    }

    #[tokio::test]
    async fn test_read_with_offset() {
        let f = make_text_file("line1\nline2\nline3\nline4\nline5\n");
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap(), "offset": 3});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        // Should show lines 3 onwards
        assert!(result.output.contains("Showing lines 3-5 of 5"));
        assert!(result.output.contains("\tline3"));
        assert!(result.output.contains("\tline4"));
        assert!(result.output.contains("\tline5"));
        // Should NOT contain line1 or line2
        assert!(!result.output.contains("\tline1"));
        assert!(!result.output.contains("\tline2"));
    }

    #[tokio::test]
    async fn test_read_with_offset_and_limit() {
        let f = make_text_file("line1\nline2\nline3\nline4\nline5\n");
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap(), "offset": 2, "limit": 2});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("\tline2"));
        assert!(result.output.contains("\tline3"));
        assert!(!result.output.contains("\tline4"));
    }

    #[tokio::test]
    async fn test_read_offset_beyond_file() {
        let f = make_text_file("line1\nline2\n");
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap(), "offset": 999});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("exceeds file length"));
    }

    #[tokio::test]
    async fn test_read_truncation_notice() {
        // Create a file with many lines to trigger truncation
        let content: Vec<String> = (1..3000).map(|i| format!("line {}", i)).collect();
        let f = make_text_file(&content.join("\n"));
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("truncated"));
        assert!(result.output.contains("Use offset="));
    }

    #[tokio::test]
    async fn test_read_path_traversal_rejected() {
        let tool = ReadTool::new();
        let params = json!({"path": "../../etc/passwd"});
        let result = tool.execute("test", params, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Path traversal"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tool = ReadTool::new();
        let params = json!({"path": "/nonexistent/path/file.txt"});
        let result = tool.execute("test", params, None).await;
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[tokio::test]
    async fn test_read_binary_detection() {
        let mut f = NamedTempFile::new().unwrap();
        // Write bytes with null bytes
        f.write_all(b"hello\x00world\x00binary").unwrap();
        f.flush().unwrap();
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("binary"));
    }

    #[tokio::test]
    async fn test_read_image_file() {
        let mut f = NamedTempFile::with_suffix(".png").unwrap();
        // Write a fake PNG-like header + data
        f.write_all(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00])
            .unwrap();
        f.flush().unwrap();
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("image/png"));
        // Should have content blocks with image
        let blocks = result.content_blocks.unwrap();
        assert!(blocks.iter().any(|b| matches!(b, ContentBlock::Image(_))));
    }

    #[tokio::test]
    async fn test_read_image_jpg() {
        let mut f = NamedTempFile::with_suffix(".jpg").unwrap();
        f.write_all(b"\xFF\xD8\xFF\xE0").unwrap();
        f.flush().unwrap();
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("image/jpeg"));
        let blocks = result.content_blocks.unwrap();
        assert!(blocks.iter().any(|b| matches!(b, ContentBlock::Image(_))));
    }

    #[tokio::test]
    async fn test_read_image_webp() {
        let mut f = NamedTempFile::with_suffix(".webp").unwrap();
        f.write_all(b"RIFF\x00\x00\x00\x00WEBP").unwrap();
        f.flush().unwrap();
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("image/webp"));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let f = make_text_file("");
        let tool = ReadTool::new();
        let params = json!({"path": f.path().to_str().unwrap()});
        let result = tool.execute("test", params, None).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tool = ReadTool::new();
        let params = json!({"path": "/tmp/nonexistent_oxi_test_file_12345.txt"});
        let result = tool.execute("test", params, None).await;
        match result {
            Err(e) => assert!(e.contains("File not found")),
            Ok(r) => assert!(!r.success),
        }
    }

    #[tokio::test]
    async fn test_read_directory_error() {
        let tool = ReadTool::new();
        let params = json!({"path": "/tmp"});
        let result = tool.execute("test", params, None).await;
        match result {
            Err(e) => assert!(e.contains("directory")),
            Ok(r) => assert!(!r.success || r.output.contains("directory")),
        }
    }

    #[test]
    fn test_image_mime_type_detection() {
        assert_eq!(
            ReadTool::image_mime_type(Path::new("photo.jpg")),
            Some("image/jpeg")
        );
        assert_eq!(
            ReadTool::image_mime_type(Path::new("photo.jpeg")),
            Some("image/jpeg")
        );
        assert_eq!(
            ReadTool::image_mime_type(Path::new("icon.png")),
            Some("image/png")
        );
        assert_eq!(
            ReadTool::image_mime_type(Path::new("anim.gif")),
            Some("image/gif")
        );
        assert_eq!(
            ReadTool::image_mime_type(Path::new("img.webp")),
            Some("image/webp")
        );
        assert_eq!(ReadTool::image_mime_type(Path::new("file.txt")), None);
        assert_eq!(ReadTool::image_mime_type(Path::new("noext")), None);
    }

    #[test]
    fn test_binary_detection() {
        assert!(ReadTool::is_binary(b"hello\x00world"));
        assert!(!ReadTool::is_binary(b"hello world\nfoo bar\n"));
        assert!(!ReadTool::is_binary(b""));
        assert!(!ReadTool::is_binary(b"pure ascii text"));
    }
}
