//! Read file tool

use super::{AgentTool, AgentToolResult, ProgressCallback, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::fs;
use tokio::io::AsyncReadExt;

pub struct ReadTool {
    progress_callback: Arc<Mutex<Option<ProgressCallback>>>,
}

impl ReadTool {
    pub fn new() -> Self {
        Self {
            progress_callback: Arc::new(Mutex::new(None)),
        }
    }

    async fn read_file_impl(path: &str, progress_cb: &Option<ProgressCallback>) -> Result<String, ToolError> {
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

        let file_size = fs::File::open(path)
            .await
            .map_err(|e| format!("Cannot open file: {}", e))?
            .metadata()
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        if let Some(cb) = progress_cb {
            cb(format!("Reading file: {} ({} bytes)", path.display(), file_size));
        }

        // Read file content in chunks for progress tracking
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| format!("Cannot open file: {}", e))?;

        let mut content = String::new();
        let mut buffer = vec![0u8; 8192];
        let mut bytes_read: u64 = 0;

        loop {
            let n = file.read(&mut buffer).await.map_err(|e| format!("Cannot read file: {}", e))?;
            if n == 0 {
                break;
            }
            
            // Convert bytes to string, handling partial UTF-8
            let chunk = String::from_utf8_lossy(&buffer[..n]);
            content.push_str(&chunk);
            bytes_read += n as u64;

            // Emit progress for large files (> 1KB)
            if file_size > 1024 {
                let progress = if file_size > 0 {
                    (bytes_read as f64 / file_size as f64 * 100.0) as u32
                } else {
                    100
                };
                if let Some(cb) = progress_cb {
                    cb(format!("Reading: {}% ({}/{} bytes)", progress, bytes_read, file_size));
                }
            }
        }

        if let Some(cb) = progress_cb {
            cb(format!("Completed reading {} bytes", content.len()));
        }

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
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let path = params
            .get("path")
            .and_then(|v: &Value| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let progress_cb = self.progress_callback.lock().unwrap().clone();
        match Self::read_file_impl(path, &progress_cb).await {
            Ok(content) => Ok(AgentToolResult::success(content)),
            Err(e) => Ok(AgentToolResult::error(e)),
        }
    }

    fn on_progress(&self, callback: ProgressCallback) {
        let cb = self.progress_callback.clone();
        let mut guard = cb.lock().unwrap();
        *guard = Some(callback);
    }
}