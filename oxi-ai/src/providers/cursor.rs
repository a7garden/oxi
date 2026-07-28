//! Cursor provider — remote-AGENT protocol via HTTP/2 + Protobuf (Connect).
//!
//! Port of omp `packages/ai/src/providers/cursor.ts` (3395 lines).
//!
//! Cursor's Run RPC (`/agent.v1.AgentService/Run`) uses HTTP/2 with the Connect
//! streaming protocol and protobuf-encoded messages. This provider requires:
//! - `h2` or HTTP/2 client
//! - `prost` for protobuf message types (agent_pb)
//!
//! ## Status
//! Not implemented — requires HTTP/2 + protobuf infra that oxi-ai does not
//! currently bundle. See omp `cursor.ts` for the reference implementation.

use std::future::Future;
use std::pin::Pin;

use crate::{
    Context, Model, Provider, StreamOptions, StreamResult,
    error::ProviderError,
};

/// Cursor provider stub — not yet implemented.
#[derive(Clone)]
pub struct CursorProvider;

impl CursorProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CursorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for CursorProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        _context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            Err(ProviderError::NotImplemented(
                "Cursor provider requires HTTP/2 + protobuf transport".to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_provider_creation() {
        let provider = CursorProvider::new();
        // Provider identity via fields only (no name() method)
        let _ = provider;
    }
}
