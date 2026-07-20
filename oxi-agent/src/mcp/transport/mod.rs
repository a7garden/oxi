//! MCP transport layer abstraction.
//!
//! [`McpTransport`] decouples MCP message I/O from [`crate::mcp::client::McpClient`].
//! Transports are responsible for framing, stream parsing, and the
//! request/response correlation over their own I/O channel. The client owns
//! the id counter and uses `request` / `notify` to talk to the server.
//!
//! v2 redesign (D-rev1): the previous `send`/`recv` model was stdio-shaped and
//! forced HTTP transports to either buffer full HTTP round-trips inside
//! `send` or maintain a parallel reader. The new `request`/`notify` model
//! (mirroring the OMP MCP transport) puts correlation inside the transport
//! and exposes a single `set_inbound_handler` for notifications and
//! server→client requests that may arrive between responses.
//!
//! v2.0: [`stdio::StdioTransport`] (JSONL framing, inline read loop).
//! v2.1: [`http::StreamableHttpTransport`] (Streamable HTTP + SSE responses).

pub mod http;
pub mod stdio;
/// PR-B1: WebSocket MCP transport 스켈레톤.
pub mod ws_transport;

use crate::mcp::types::RawJsonRpcMessage;
use anyhow::Result;
use async_trait::async_trait;

/// Handler invoked by a transport for inbound messages that are not the
/// currently awaited response.
///
/// - **Notifications** (no `id`): the handler is called for side-effect;
///   the return value is ignored (notifications have no reply).
/// - **Server→client requests** (`method` + `id`, id not matching the
///   pending request): the handler may return `Some(value)` to send back
///   a JSON-RPC response; the transport serializes and writes it. Return
///   `None` to leave the request unanswered (rare; usually a bug).
///
/// `Send + Sync` so the same trait object can be used from any task
/// that needs to dispatch inbound messages (e.g. the HTTP POST-SSE
/// drain in [`http::StreamableHttpTransport`]).
pub type InboundHandler =
    Box<dyn FnMut(RawJsonRpcMessage) -> Option<serde_json::Value> + Send + Sync>;

/// MCP transport layer.
///
/// Implementations own the raw I/O channel (stdio pipes, HTTP+SSE streams,
/// ...) and the framing specific to that channel. They correlate outgoing
/// requests with incoming responses and surface anything else (notifications,
/// server→client requests) to the installed [`InboundHandler`].
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and await the matching response.
    ///
    /// `id` is the JSON-RPC request id and `json` is the already-serialized
    /// JSON-RPC request body. The transport writes `json` to the channel
    /// and returns the next message whose `id` equals the one supplied.
    /// Messages that arrive in the meantime (notifications, server→client
    /// requests) are dispatched to the installed [`InboundHandler`].
    ///
    /// Implementations SHOULD apply a per-request timeout and return
    /// `Err` on timeout.
    async fn request(&mut self, id: u64, json: &str) -> Result<RawJsonRpcMessage>;

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&mut self, json: &str) -> Result<()>;

    /// Install (or replace) the inbound handler. Called by the client before
    /// the first `request` so that any peer-sent message arriving during the
    /// handshake is dispatched.
    fn set_inbound_handler(&mut self, handler: InboundHandler);

    /// Close the transport gracefully. Default is a no-op for transports
    /// that close on drop.
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }

    /// Whether the transport is currently connected.
    fn is_connected(&self) -> bool;
}
