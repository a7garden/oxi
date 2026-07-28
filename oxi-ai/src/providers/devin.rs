//! Devin (Codeium/Windsurf Cascade) provider — remote-AGENT protocol.
//!
//! Port of omp `packages/ai/src/providers/devin.ts` (678 lines).
//!
//! Devin/Cascade uses the **Connect protocol** over HTTP/1.1 with protobuf
//! payloads. The RPC endpoint is
//! `/exa.api_server_pb.ApiServerService/GetChatMessage`.
//!
//! ## Protocol
//! - HTTP/1.1 transport (standard `reqwest::Client`)
//! - `application/connect+proto` content type
//! - Binary framing: 1 flag byte + 4-byte big-endian length + payload
//! - Flag bit 0x01 = gzip compressed, 0x02 = end-of-stream (JSON trailers)
//! - Auth: session token → GetUserJwt → Bearer JWT for chat
//!
//! ## Dependencies
//! - `reqwest` (already in oxi-ai)
//! - Manual protobuf wire encoding or `prost`
//!
//! ## Status
//! Not implemented — requires protobuf message encoding + Connect protocol
//! framing (see omp devin.ts + devin/proto/ for Codeium protobuf definitions).

use std::future::Future;
use std::pin::Pin;

use crate::{
    Context, Model, Provider, StreamOptions, StreamResult,
    error::ProviderError,
};

/// Devin/Cascade provider stub — not yet implemented.
#[derive(Clone)]
pub struct DevinProvider;

impl DevinProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DevinProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for DevinProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        _context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            Err(ProviderError::NotImplemented(
                "Devin provider requires protobuf message encoding".to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_devin_provider_creation() {
        let provider = DevinProvider::new();
        let _ = provider;
    }
}
