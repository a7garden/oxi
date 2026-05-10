//! MCP JSON-RPC client over stdio transport.
//!
//! Implements the MCP protocol with Content-Length framed JSON-RPC messages
//! over a spawned child process's stdin/stdout.

use super::types::{JsonRpcNotification, JsonRpcRequest, McpCallResult, McpContent, McpToolDef, RawJsonRpcMessage, ServerInfo};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// MCP protocol version we advertise during initialization.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// Default timeout for individual MCP requests (seconds).
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// An MCP client connected to a single server via stdio.
pub struct McpClient {
    /// Child process handle (kept alive to prevent process death).
    _child: tokio::process::Child,
    /// Writer to the server's stdin.
    stdin: tokio::process::ChildStdin,
    /// Buffered reader from the server's stdout.
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
    /// Next JSON-RPC request ID.
    next_id: u64,
    /// Server info from the initialize handshake.
    pub server_info: ServerInfo,
}

impl McpClient {
    /// Connect to an MCP server by spawning a child process.
    ///
    /// Performs the full initialization handshake:
    /// 1. Spawn the process
    /// 2. Send `initialize` request
    /// 3. Send `notifications/initialized`
    pub async fn connect(
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

        // Build environment: inherit parent + overlay server-specific
        for (key, value) in env {
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

        let mut client = Self {
            _child: child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            next_id: 1,
            server_info: ServerInfo {
                name: String::new(),
                version: None,
                protocol_version: String::new(),
            },
        };

        // Initialize handshake
        client.initialize().await?;

        Ok(client)
    }

    /// Perform the MCP initialize handshake.
    async fn initialize(&mut self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "oxi-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let result = self
            .send_request("initialize", Some(params))
            .await
            .context("MCP initialize failed")?;

        // Parse server info
        if let Some(info) = result.get("serverInfo") {
            self.server_info.name = info
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            self.server_info.version = info.get("version").and_then(|v| v.as_str()).map(String::from);
        }
        if let Some(version) = result.get("protocolVersion").and_then(|v| v.as_str()) {
            self.server_info.protocol_version = version.to_string();
        }

        // Send initialized notification
        let notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized".to_string(),
            params: None,
        };
        self.write_message(&serde_json::to_string(&notification)?)
            .await?;

        Ok(())
    }

    /// List all tools provided by the server.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>> {
        let result = self
            .send_request("tools/list", None)
            .await
            .context("MCP tools/list failed")?;

        let tools = result
            .get("tools")
            .cloned()
            .and_then(|v| serde_json::from_value::<Vec<McpToolDef>>(v).ok())
            .unwrap_or_default();

        Ok(tools)
    }

    /// Call a tool on the server.
    pub async fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpCallResult> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args
        });

        let result = self
            .send_request("tools/call", Some(params))
            .await
            .with_context(|| format!("MCP tools/call '{}' failed", name))?;

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let content = result
            .get("content")
            .cloned()
            .and_then(|v| serde_json::from_value::<Vec<McpContent>>(v).ok())
            .unwrap_or_default();

        Ok(McpCallResult { content, is_error })
    }

    /// List resources provided by the server.
    pub async fn list_resources(&mut self) -> Result<Vec<serde_json::Value>> {
        let result = self
            .send_request("resources/list", None)
            .await
            .context("MCP resources/list failed")?;

        Ok(result
            .get("resources")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Read a resource from the server.
    pub async fn read_resource(&mut self, uri: &str) -> Result<Vec<McpContent>> {
        let params = serde_json::json!({ "uri": uri });
        let result = self
            .send_request("resources/read", Some(params))
            .await
            .with_context(|| format!("MCP resources/read '{}' failed", uri))?;

        let contents = result
            .get("contents")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Convert resource contents to McpContent
        let mut content = Vec::new();
        for item in contents {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                content.push(McpContent::Text {
                    text: text.to_string(),
                });
            } else if let Some(_blob) = item.get("blob").and_then(|b| b.as_str()) {
                content.push(McpContent::Text {
                    text: format!("[Binary data: {}]", item.get("mimeType").and_then(|m| m.as_str()).unwrap_or("unknown")),
                });
            }
        }
        Ok(content)
    }

    /// Shut down the client and kill the server process.
    pub async fn close(&mut self) -> Result<()> {
        let _ = self.stdin.shutdown().await;
        let _ = self._child.kill().await;
        Ok(())
    }

    // ── JSON-RPC transport layer ─────────────────────────────────────

    /// Send a JSON-RPC request and wait for the matching response.
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let json = serde_json::to_string(&request)?;
        self.write_message(&json).await?;

        // Read responses until we get one with matching ID
        let timeout = std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS);
        let result = tokio::time::timeout(timeout, async {
            loop {
                let msg = self.read_message().await?;
                // Check if this is our response
                if let Some(response_id) = msg.id {
                    if response_id == id {
                        if let Some(error) = msg.error {
                            return Err(anyhow::anyhow!(
                                "JSON-RPC error {}: {}",
                                error.code,
                                error.message
                            ));
                        }
                        return Ok(msg.result.unwrap_or(serde_json::Value::Null));
                    }
                }
                // Notification or unmatched response — skip
            }
        })
        .await
        .with_context(|| format!("MCP request '{}' timed out after {}s", method, REQUEST_TIMEOUT_SECS))??;

        Ok(result)
    }

    /// Write a JSON-RPC message with Content-Length framing.
    async fn write_message(&mut self, json: &str) -> Result<()> {
        let bytes = json.as_bytes();
        let header = format!("Content-Length: {}\r\n\r\n", bytes.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(bytes).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Read a single JSON-RPC message from the transport.
    async fn read_message(&mut self) -> Result<RawJsonRpcMessage> {
        // Parse Content-Length header
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let bytes_read = self.stdout.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Err(anyhow::anyhow!("MCP server closed connection"));
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

        let len = content_length
            .ok_or_else(|| anyhow::anyhow!("Missing Content-Length header"))?;

        // Read body
        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf).await?;

        let msg: RawJsonRpcMessage =
            serde_json::from_slice(&buf).context("Failed to parse JSON-RPC message")?;

        Ok(msg)
    }
}
