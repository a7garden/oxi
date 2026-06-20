//! Stdio transport for MCP.
//!
//! Spawns a child process and communicates over its stdin/stdout using
//! **newline-delimited JSON** (JSONL) per the MCP stdio transport spec
//! (modelcontextprotocol.io/specification/2025-03-26/basic/transports).
//! Each JSON-RPC message is serialized as one line of JSON terminated by
//! a single `\n`; the transport rejects any line that exceeds [`MAX_LINE_SIZE`].
//!
//! The transport is owned by a single [`crate::mcp::client::McpClient`] and
//! is `&mut`-accessed exclusively by that client. The read loop in
//! [`McpTransport::request`] dispatches inbound notifications and
//! server→client requests to the installed [`InboundHandler`] inline.

use super::{InboundHandler, McpTransport};
use crate::mcp::types::RawJsonRpcMessage;
use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Default timeout for individual MCP requests (seconds).
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Maximum allowed line size from an MCP server (10 MB).
/// Guards against a buggy or hostile local server sending a runaway line
/// that would otherwise cause unbounded allocation in the read buffer.
const MAX_LINE_SIZE: usize = 10 * 1024 * 1024;

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
    /// Inbound handler for notifications and server→client requests.
    inbound_handler: Option<InboundHandler>,
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
        env: &std::collections::HashMap<String, String>,
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
            inbound_handler: None,
        })
    }

    /// Wrap an already-spawned child process.
    /// Used internally by the client to allow later `take_child`.
    pub fn from_parts(child: Child, stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            child: Some(child),
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            inbound_handler: None,
        }
    }

    /// Take the child process out (for graceful shutdown via signal).
    pub fn take_child(&mut self) -> Option<Child> {
        self.child.take()
    }

    /// Write a single JSON-RPC message framed as one line of JSON + '\n'.
    /// The MCP spec requires messages to be single-line; `serde_json`
    /// never embeds raw newlines, so a trailing `\n` is sufficient.
    async fn write_frame(&mut self, json: &str) -> Result<()> {
        debug_assert!(
            !json.contains('\n'),
            "MCP 메시지에 내장 개행 금지 (스펙 위반)"
        );
        self.stdin
            .write_all(json.as_bytes())
            .await
            .context("Failed to write MCP body")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("Failed to write MCP newline")?;
        self.stdin
            .flush()
            .await
            .context("Failed to flush MCP stdin")?;
        Ok(())
    }

    /// Read one message (one line) from stdout, with a hard size cap.
    /// Returns `Ok(None)` on clean EOF, `Err` on overflow or I/O failure.
    async fn read_frame(&mut self) -> Result<Option<RawJsonRpcMessage>> {
        match read_line_bounded(&mut self.stdout, MAX_LINE_SIZE).await? {
            None => Ok(None),
            Some(bytes) => {
                let msg: RawJsonRpcMessage =
                    serde_json::from_slice(&bytes).context("Failed to parse JSON-RPC message")?;
                Ok(Some(msg))
            }
        }
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn request(&mut self, id: u64, json: &str) -> Result<RawJsonRpcMessage> {
        self.write_frame(json).await?;

        // Read messages until the response with a matching id arrives.
        // Anything else (notifications, server→client requests) is
        // dispatched to the inbound handler inline.
        let timeout = std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS);
        tokio::time::timeout(timeout, async {
            loop {
                let msg = self
                    .read_frame()
                    .await
                    .context("Failed to read MCP response")?
                    .ok_or_else(|| anyhow::anyhow!("MCP server closed connection"))?;

                let msg_id = msg.id;
                if let Some(mid) = msg_id {
                    if mid == id {
                        return Ok(msg);
                    }
                    // Non-matching id — if it has a `method` it's a
                    // server→client request; dispatch and maybe reply.
                    if msg.method.is_some() {
                        let response = match self.inbound_handler.as_mut() {
                            Some(h) => h(msg),
                            None => None,
                        };
                        if let Some(value) = response {
                            let reply = serde_json::to_string(&value)
                                .context("Failed to serialize inbound response")?;
                            self.write_frame(&reply)
                                .await
                                .context("Failed to write response to server→client request")?;
                        }
                        continue;
                    }
                    // Orphan response (different id, no method). Should not
                    // happen in normal operation — log and skip.
                    tracing::warn!(
                        "MCP: discarding response with non-matching id {} (expected {})",
                        mid,
                        id
                    );
                    continue;
                }
                // No id → notification → dispatch (return value ignored).
                if let Some(h) = self.inbound_handler.as_mut() {
                    h(msg);
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("MCP request timed out after {}s", REQUEST_TIMEOUT_SECS))?
    }

    async fn notify(&mut self, json: &str) -> Result<()> {
        self.write_frame(json).await
    }

    fn set_inbound_handler(&mut self, handler: InboundHandler) {
        self.inbound_handler = Some(handler);
    }

    async fn close(&mut self) -> Result<()> {
        let _ = self.stdin.shutdown().await;
        // Graceful shutdown: SIGTERM then 5s then SIGKILL.
        #[cfg(unix)]
        {
            if let Some(mut child) = self.take_child()
                && let Some(pid) = child.id()
            {
                // SAFETY: libc::kill sends a signal to a process. The PID comes
                // from child.id() which is a valid running process. SIGTERM
                // requests graceful termination. On race (process already
                // exited), kill returns ESRCH harmlessly.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
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

/// Bounded line reader over an [`AsyncBufRead`]. Reads until '\n' (included)
/// or EOF. Caps the returned buffer at `max` bytes; returns an error if the
/// line would exceed the cap, preventing unbounded allocation.
async fn read_line_bounded<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = reader
            .fill_buf()
            .await
            .context("Failed to read from MCP stdout")?;
        if chunk.is_empty() {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(anyhow::anyhow!("MCP server closed connection mid-line"))
            };
        }
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            if buf.len() + take > max {
                return Err(anyhow::anyhow!(
                    "MCP line exceeds {} bytes (mid-line cap hit)",
                    max
                ));
            }
            buf.extend_from_slice(&chunk[..take]);
            reader.consume(take);
            return Ok(Some(buf));
        }
        if buf.len() + chunk.len() > max {
            return Err(anyhow::anyhow!(
                "MCP line exceeds {} bytes (chunk would overflow)",
                max
            ));
        }
        let n = chunk.len();
        buf.extend_from_slice(chunk);
        reader.consume(n);
    }
}
