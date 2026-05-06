//! RPC mode for headless operation
//!
//! Implements a JSONL-over-stdio protocol for embedding oxi in other applications.
//! Commands are received as JSON lines on stdin, responses and events are emitted
//! as JSON lines on stdout.
//!
//! # Protocol
//!
//! - **Commands**: JSON objects with a `type` field and optional `id` for correlation
//! - **Responses**: JSON objects with `type: "response"`, `command`, `success`, and optional `data`/`error`
//! - **Events**: `AgentEvent` objects streamed as they occur
//! - **Extension UI**: Extension UI requests emitted to client; client responds with `extension_ui_response`
//! - **JSON-RPC 2.0**: Alternatively supports JSON-RPC 2.0 framing (detected by `jsonrpc` field)
//!
//! # Framing
//!
//! Strict JSONL (JSON Lines): each record is a single JSON object terminated by `\n` (LF).
//! Carriage returns inside payloads are preserved; records are split on `\n` only.

#![allow(dead_code, unused_imports)]

pub mod handlers;
pub mod protocol;
pub mod state;
#[cfg(test)]
mod tests;
pub mod utils;

// ── Re-exports ──────────────────────────────────────────────────

// Public API: main entry point
pub use handlers::run_rpc_mode;

// Protocol types
pub use protocol::{
    serialize_json_line, serialize_json_line_obj, parse_json_line,
    JsonRpcRequest, JsonRpcSuccessResponse, JsonRpcError, JsonRpcErrorResponse,
    JSONRPC_PARSE_ERROR, JSONRPC_INVALID_REQUEST, JSONRPC_METHOD_NOT_FOUND,
    JSONRPC_INVALID_PARAMS, JSONRPC_INTERNAL_ERROR,
    jsonrpc_to_command, rpc_response_to_jsonrpc,
    RpcCommand, ImageData, RpcImageSource,
    RpcResponse, RpcExtensionUiRequest, RpcExtensionUiResponse,
    SessionState, ModelInfo, CommandInfo, SourceInfo, SessionStats, CompactionResult,
    RpcEvent, PendingExtensionRequest,
    JsonlLineReader, RpcOutput, SessionHandoff,
};

// State types
pub use state::RpcServer;

// Utility types
pub use utils::{
    PasteState, PasteHandler,
    RpcClientConfig, BoxedEventListener, RpcClient,
};
