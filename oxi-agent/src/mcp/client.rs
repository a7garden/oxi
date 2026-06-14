//! MCP JSON-RPC client.
//!
//! Communicates with an MCP server through a [`McpTransport`]
//! (currently only [`StdioTransport`]). Owns the request/response correlation
//! and ID counter, but delegates raw I/O to the transport.

use super::transport::{McpTransport, stdio::StdioTransport};
use super::types::{
    JsonRpcNotification, JsonRpcRequest, McpCallResult, McpContent, McpToolDef, RawJsonRpcMessage,
    ServerInfo,
};
use anyhow::{Context, Result};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// MCP protocol version we advertise during initialization.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// Default timeout for individual MCP requests (seconds).
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Maximum number of orphaned responses to drain on timeout.
const MAX_DRAIN_RESPONSES: usize = 16;

/// MCP prompt template.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpPrompt {
    /// Prompt identifier.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arguments accepted by the prompt template.
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// A single argument accepted by an [`McpPrompt`] template.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpPromptArgument {
    /// Argument name.
    pub name: String,
    /// Optional human-readable description of the argument.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the argument must be supplied.
    #[serde(default)]
    pub required: bool,
}

/// MCP log level for `logging/setLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpLogLevel {
    /// Fine-grained diagnostic information.
    Debug,
    /// General informational messages.
    Info,
    /// Normal-but-significant conditions.
    Notice,
    /// Indication that something unexpected happened.
    Warning,
    /// Runtime errors that do not halt execution.
    Error,
    /// Critical conditions requiring immediate attention.
    Critical,
    /// Action must be taken immediately.
    Alert,
    /// System is unusable.
    Emergency,
}

impl McpLogLevel {
    /// Return the wire string representation used by the MCP protocol.
    pub fn as_str(&self) -> &'static str {
        match self {
            McpLogLevel::Debug => "debug",
            McpLogLevel::Info => "info",
            McpLogLevel::Notice => "notice",
            McpLogLevel::Warning => "warning",
            McpLogLevel::Error => "error",
            McpLogLevel::Critical => "critical",
            McpLogLevel::Alert => "alert",
            McpLogLevel::Emergency => "emergency",
        }
    }
}

/// MCP sampling request — server asks oxi to make an LLM call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpSamplingRequest {
    /// Conversation messages to sample from.
    pub messages: Vec<serde_json::Value>,
    /// Optional system prompt to prepend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Maximum number of tokens to generate.
    pub max_tokens: u32,
    /// Optional sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// An MCP client connected to a single server through a transport.
pub struct McpClient {
    /// Underlying transport (stdio, HTTP/SSE, ...).
    transport: Box<dyn McpTransport>,
    /// Next JSON-RPC request ID.
    next_id: u64,
    /// Server info from the initialize handshake.
    pub server_info: ServerInfo,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server_info", &self.server_info)
            .field("next_id", &self.next_id)
            .field("connected", &self.transport.is_connected())
            .finish()
    }
}

impl McpClient {
    /// Connect to an MCP server via stdio transport.
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
        let transport: Box<dyn McpTransport> =
            Box::new(StdioTransport::spawn(command, args, env, cwd, debug)?);

        let mut client = Self {
            transport,
            next_id: 1,
            server_info: ServerInfo {
                name: String::new(),
                version: None,
                protocol_version: String::new(),
            },
        };

        client.initialize().await?;
        Ok(client)
    }

    /// Connect through a pre-built transport (used by tests and by future
    /// HTTP/SSE transports).
    pub async fn connect_with_transport(transport: Box<dyn McpTransport>) -> Result<Self> {
        let mut client = Self {
            transport,
            next_id: 1,
            server_info: ServerInfo {
                name: String::new(),
                version: None,
                protocol_version: String::new(),
            },
        };
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

        if let Some(info) = result.get("serverInfo") {
            self.server_info.name = info
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            self.server_info.version = info
                .get("version")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
        if let Some(version) = result.get("protocolVersion").and_then(|v| v.as_str()) {
            self.server_info.protocol_version = version.to_string();
        }

        let notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&notification)?;
        self.transport
            .send(&json)
            .await
            .context("Failed to send notifications/initialized")?;

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
            .unwrap_or_else(|| {
                tracing::warn!(
                    "MCP: failed to parse tools/list response from '{}'",
                    self.server_info.name
                );
                Vec::new()
            });

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

        let mut content = Vec::new();
        for item in contents {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                content.push(McpContent::Text {
                    text: text.to_string(),
                });
            } else if item.get("blob").is_some() {
                content.push(McpContent::Text {
                    text: format!(
                        "[Binary data: {}]",
                        item.get("mimeType")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown")
                    ),
                });
            }
        }
        Ok(content)
    }

    /// List prompt templates available on the MCP server.
    pub async fn list_prompts(&mut self) -> Result<Vec<McpPrompt>> {
        let result = self.send_request("prompts/list", None).await?;
        let prompts = result
            .get("prompts")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(prompts)
            .map_err(|e| anyhow::anyhow!("Failed to parse prompts/list response: {}", e))
    }

    /// Get a prompt template with arguments applied.
    pub async fn get_prompt(
        &mut self,
        name: &str,
        args: std::collections::HashMap<String, String>,
    ) -> Result<Vec<serde_json::Value>> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args
        });
        let result = self.send_request("prompts/get", Some(params)).await?;
        let messages = result
            .get("messages")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        Ok(serde_json::from_value(messages).unwrap_or_default())
    }

    /// Set the minimum log level for MCP server notifications.
    pub async fn set_log_level(&mut self, level: McpLogLevel) -> Result<()> {
        let params = serde_json::json!({ "level": level.as_str() });
        self.send_request("logging/setLevel", Some(params)).await?;
        Ok(())
    }

    /// Request the host to create a message via LLM sampling.
    pub async fn create_sample(
        &mut self,
        request: McpSamplingRequest,
    ) -> Result<serde_json::Value> {
        let params = serde_json::to_value(&request)
            .map_err(|e| anyhow::anyhow!("Failed to serialize sampling request: {}", e))?;
        self.send_request("sampling/createMessage", Some(params))
            .await
    }

    /// Send a low-level ping to verify the server is alive.
    pub async fn ping(&mut self) -> Result<()> {
        self.send_request("ping", None).await?;
        Ok(())
    }

    /// Whether the transport is currently connected.
    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    /// Shut down the client gracefully.
    pub async fn close(&mut self) -> Result<()> {
        self.transport.close().await
    }

    // ── JSON-RPC request/response correlation ─────────────────────

    /// Send a JSON-RPC request and wait for the matching response.
    ///
    /// On timeout, drains orphaned responses to prevent ID mismatch on
    /// subsequent requests.
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
        self.transport
            .send(&json)
            .await
            .with_context(|| format!("MCP send '{}' failed", method))?;

        let timeout = std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS);
        let result = tokio::time::timeout(timeout, async {
            loop {
                let msg = self
                    .transport
                    .recv()
                    .await
                    .context("Failed to read MCP response")?;
                if let Some(response_id) = msg.id
                    && response_id == id
                {
                    if let Some(error) = msg.error {
                        return Err(anyhow::anyhow!(
                            "JSON-RPC error {}: {}",
                            error.code,
                            error.message
                        ));
                    }
                    return Ok(msg.result.unwrap_or(serde_json::Value::Null));
                }
                // Notification or unmatched response — skip
            }
        })
        .await;

        match result {
            Ok(inner) => inner.with_context(|| format!("MCP request '{}' failed", method)),
            Err(_) => {
                tracing::warn!(
                    "MCP request '{}' timed out after {}s, draining orphaned responses",
                    method,
                    REQUEST_TIMEOUT_SECS
                );
                self.drain_orphaned_responses(MAX_DRAIN_RESPONSES).await;
                Err(anyhow::anyhow!(
                    "MCP request '{}' timed out after {}s",
                    method,
                    REQUEST_TIMEOUT_SECS
                ))
            }
        }
    }

    /// Drain up to `max` orphaned responses from the transport.
    async fn drain_orphaned_responses(&mut self, max: usize) {
        for _ in 0..max {
            match tokio::time::timeout(std::time::Duration::from_millis(100), self.transport.recv())
                .await
            {
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
    }
}

/// Write a JSON-RPC message with Content-Length framing.
#[allow(dead_code)]
pub async fn write_framed<W: AsyncWriteExt + Unpin>(writer: &mut W, json: &str) -> Result<()> {
    let bytes = json.as_bytes();
    let header = format!("Content-Length: {}\r\n\r\n", bytes.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a single Content-Length framed message from a buffered reader.
#[allow(dead_code)]
pub async fn read_framed<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Result<RawJsonRpcMessage> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line).await?;
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
    let len = content_length.ok_or_else(|| anyhow::anyhow!("Missing Content-Length header"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    let msg: RawJsonRpcMessage =
        serde_json::from_slice(&buf).context("Failed to parse JSON-RPC message")?;
    Ok(msg)
}
