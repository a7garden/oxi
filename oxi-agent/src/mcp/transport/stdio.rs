//! Stdio transport for MCP.
//!
//! Spawns a child process and communicates over its stdin/stdout using
//! Content-Length framed JSON-RPC messages. The transport is owned by a
//! single [`crate::mcp::client::McpClient`] and is `&mut`-accessed
//! exclusively by that client.

use super::McpTransport;
use crate::mcp::types::RawJsonRpcMessage;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Default timeout for individual MCP requests (seconds).
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Maximum number of header lines before giving up (prevents infinite loop).
const MAX_HEADER_LINES: usize = 64;

/// Maximum allowed body size from an MCP server (10 MB).
const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

/// Environment variables that servers must not override (security).
const BLOCKED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
];

/// Stdio transport for a spawned MCP server process.
pub struct StdioTransport {
    /// Child process handle (kept alive to prevent process death).
    /// `None` after `take_child` has been called.
    child: Option<Child>,
    /// Writer to the server's stdin.
    stdin: ChildStdin,
    /// Buffered reader from the server's stdout.
    stdout: tokio::io::BufReader<ChildStdout>,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport")
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl StdioTransport {
    /// Spawn a child process and return a connected transport.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&str>,
        debug: bool,
    ) -> Result<Self> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);

        if debug {
            cmd.stderr(Stdio::inherit());
        } else {
            cmd.stderr(Stdio::null());
        }

        for (key, value) in env {
            let upper = key.to_uppercase();
            if BLOCKED_ENV_VARS.iter().any(|blocked| upper == *blocked) {
                tracing::warn!("MCP: blocked dangerous env override: {}", key);
                continue;
            }
            cmd.env(key, value);
        }

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", command))?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to acquire stdin from MCP server")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to acquire stdout from MCP server")?;

        Ok(Self {
            child: Some(child),
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
        })
    }

    /// Wrap an already-spawned child process.
    /// Used internally by the client to allow later `take_child`.
    pub fn from_parts(child: Child, stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            child: Some(child),
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
        }
    }

    /// Take the child process out (for graceful shutdown via signal).
    pub fn take_child(&mut self) -> Option<Child> {
        self.child.take()
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn send(&mut self, json: &str) -> Result<()> {
        let bytes = json.as_bytes();
        let header = format!("Content-Length: {}\r\n\r\n", bytes.len());
        self.stdin
            .write_all(header.as_bytes())
            .await
            .context("Failed to write MCP header")?;
        self.stdin
            .write_all(bytes)
            .await
            .context("Failed to write MCP body")?;
        self.stdin
            .flush()
            .await
            .context("Failed to flush MCP stdin")?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<RawJsonRpcMessage> {
        tokio::time::timeout(
            std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS),
            self.read_message_inner(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!("MCP read_message timed out after {}s", REQUEST_TIMEOUT_SECS)
        })?
    }

    async fn close(&mut self) -> Result<()> {
        let _ = self.stdin.shutdown().await;
        // Try graceful shutdown first
        #[cfg(unix)]
        {
            if let Some(mut child) = self.take_child()
                && let Some(id) = child.id()
            {
                // SAFETY: libc::kill sends a signal to a process. The PID comes
                // from child.id() which is a valid running process. SIGTERM
                // requests graceful termination. On race (process already
                // exited), kill returns ESRCH harmlessly.
                unsafe {
                    libc::kill(id as libc::pid_t, libc::SIGTERM);
                }
                match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                    Ok(Ok(_)) => return Ok(()),
                    _ => {
                        let _ = child.kill().await;
                    }
                }
            }
        }
        #[cfg(not(unix))]
        {
            if let Some(mut child) = self.take_child() {
                let _ = child.kill().await;
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.child.is_some()
    }
}

impl StdioTransport {
    /// Read a single JSON-RPC message (Content-Length header + body).
    async fn read_message_inner(&mut self) -> Result<RawJsonRpcMessage> {
        let mut content_length: Option<usize> = None;
        let mut lines_read = 0;
        loop {
            let mut line = String::new();
            let bytes_read = self
                .stdout
                .read_line(&mut line)
                .await
                .context("Failed to read MCP header")?;
            if bytes_read == 0 {
                return Err(anyhow::anyhow!("MCP server closed connection"));
            }
            lines_read += 1;
            if lines_read > MAX_HEADER_LINES {
                return Err(anyhow::anyhow!(
                    "MCP server sent too many header lines (>{})",
                    MAX_HEADER_LINES
                ));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(
                    rest.trim()
                        .parse::<usize>()
                        .context("Invalid Content-Length header")?,
                );
            }
        }

        let len = content_length.ok_or_else(|| anyhow::anyhow!("Missing Content-Length header"))?;

        if len > MAX_BODY_SIZE {
            return Err(anyhow::anyhow!(
                "MCP server sent oversized body: {} bytes (max {})",
                len,
                MAX_BODY_SIZE
            ));
        }

        let mut buf = vec![0u8; len];
        self.stdout
            .read_exact(&mut buf)
            .await
            .context("Failed to read MCP body")?;

        let msg: RawJsonRpcMessage =
            serde_json::from_slice(&buf).context("Failed to parse JSON-RPC message")?;
        Ok(msg)
    }
}
