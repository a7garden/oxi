//! MCP transport layer abstraction.
//!
//! Defines the [`McpTransport`] trait used by [`crate::mcp::client::McpClient`]
//! to communicate with MCP servers. Phase 1 provides the [`stdio::StdioTransport`]
//! implementation; Phase 5 adds the `http_sse` implementation.

pub mod stdio;

use crate::mcp::types::RawJsonRpcMessage;
use async_trait::async_trait;

/// MCP transport layer abstraction.
///
/// A transport owns the raw I/O channel (stdio pipes, HTTP+SSE streams, etc.)
/// and provides synchronous-feeling `send` / `recv` primitives for JSON-RPC
/// messages. The client wraps the transport with request/response correlation.
///
/// Phase 1: only [`stdio::StdioTransport`] is implemented.
/// Phase 5: `HttpSseTransport` will be added without changing this trait.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC message string (Content-Length framing handled internally).
    async fn send(&mut self, json: &str) -> anyhow::Result<()>;

    /// Receive the next JSON-RPC message (request, response, or notification).
    async fn recv(&mut self) -> anyhow::Result<RawJsonRpcMessage>;

    /// Close the transport gracefully.
    async fn close(&mut self) -> anyhow::Result<()>;

    /// Whether the transport is currently connected.
    fn is_connected(&self) -> bool;
}
