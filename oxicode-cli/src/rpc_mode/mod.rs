//! RPC mode for headless operation
//!
//! Implements a JSONL-over-stdio protocol for embedding oxicode in other applications.
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
#[cfg(test)]
mod tests;
pub mod utils;

// ── Re-exports ──────────────────────────────────────────────────

// Public API: main entry point
pub use handlers::run_rpc_mode;

// Protocol types
pub use protocol::{
    CommandInfo, CompactionResult, ImageData, JSONRPC_INTERNAL_ERROR, JSONRPC_INVALID_PARAMS,
    JSONRPC_INVALID_REQUEST, JSONRPC_METHOD_NOT_FOUND, JSONRPC_PARSE_ERROR, JsonRpcError,
    JsonRpcErrorResponse, JsonRpcRequest, JsonRpcSuccessResponse, JsonlLineReader, ModelInfo,
    PendingExtensionRequest, RpcCommand, RpcEvent, RpcExtensionUiRequest, RpcExtensionUiResponse,
    RpcImageSource, RpcOutput, RpcResponse, SessionHandoff, SessionState, SessionStats, SourceInfo,
    jsonrpc_to_command, parse_json_line, rpc_response_to_jsonrpc, serialize_json_line,
    serialize_json_line_obj,
};

// Utility types
pub use utils::{BoxedEventListener, PasteHandler, PasteState, RpcClient, RpcClientConfig};
