//! Streamable HTTP transport for MCP (spec 2025-03-26).
//!
//! Speaks the current Streamable HTTP transport:
//! - POST JSON-RPC messages to a single MCP endpoint; the server replies
//!   with either `Content-Type: application/json` (single response) or
//!   `Content-Type: text/event-stream` (SSE stream of one or more
//!   messages, the first matching id being our response).
//! - `Mcp-Session-Id` is captured from the `initialize` response and
//!   attached to all subsequent requests and notifications.
//! - DELETE terminates the session.
//!
//! v2.1 deliberately omits the dedicated server-push SSE listener
//! (GET on the MCP endpoint). Server-push messages that arrive *on a
//! POST SSE response stream* (the common path) are still dispatched to
//! the inbound handler inline during request correlation. A background
//! GET listener that survives across requests would require
//! `Arc<Self>` plumbing through `Box<dyn McpTransport>` and is deferred
//! — see `docs/designs/2026-06-19-mcp-v2-conformance-transports.md` §4.2.
//!
//! Authentication is delegated to an optional [`McpCredentialProvider`]:
//! the transport injects `Authorization: Bearer …` on every request and,
//! on `401`/`403`, calls [`McpCredentialProvider::refresh`] once and
//! retries the request with the newly returned token.

use super::{InboundHandler, McpTransport};
use crate::mcp::auth::{Credential, McpCredentialProvider};
use crate::mcp::types::RawJsonRpcMessage;
use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};

/// Default per-request timeout (milliseconds) when `ServerEntry::timeout`
/// is `None`.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Streamable HTTP transport.
pub struct StreamableHttpTransport {
    endpoint: String,
    server_name: String,
    client: Client,
    /// Session id captured from the `initialize` response; attached to
    /// every subsequent request header.
    session_id: Mutex<Option<String>>,
    /// Inbound handler for notifications and server→client requests.
    /// Uses `parking_lot` (sync) so the sync [`Self::set_inbound_handler`]
    /// setter can write without `async` plumbing; the guard is dropped
    /// before any `.await` so the `!Send` constraint is safe.
    inbound_handler: parking_lot::Mutex<Option<InboundHandler>>,
    /// Reserved for future use (e.g. background GET listener).
    #[allow(dead_code)]
    pending: Mutex<HashMap<u64, oneshot::Sender<RawJsonRpcMessage>>>,
    credential_provider: Option<Arc<dyn McpCredentialProvider>>,
    timeout: Duration,
    closed: Mutex<bool>,
}

impl std::fmt::Debug for StreamableHttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamableHttpTransport")
            .field("endpoint", &self.endpoint)
            .field("server_name", &self.server_name)
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl StreamableHttpTransport {
    /// Build a new transport.
    ///
    /// `credential_provider` is queried for `Authorization` headers on
    /// every request; pass `None` to disable authentication.
    /// `timeout_ms == 0` disables the client-side per-request timeout.
    pub fn new(
        server_name: &str,
        endpoint: &str,
        credential_provider: Option<Arc<dyn McpCredentialProvider>>,
        timeout_ms: u64,
    ) -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("oxicode-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Failed to build reqwest client for MCP Streamable HTTP")?;
        Ok(Self {
            endpoint: endpoint.to_string(),
            server_name: server_name.to_string(),
            client,
            session_id: Mutex::new(None),
            inbound_handler: parking_lot::Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            credential_provider,
            timeout: if timeout_ms == 0 {
                Duration::from_secs(60 * 60 * 24 * 365)
            } else {
                Duration::from_millis(timeout_ms)
            },
            closed: Mutex::new(false),
        })
    }

    /// Build the standard request headers (Accept, session id, auth).
    async fn build_headers(&self, credential: Option<&Credential>) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Accept",
            // SAFETY: the Accept value is a compile-time literal with no
            // invalid header characters; `parse` cannot fail.
            #[allow(clippy::expect_used)]
            "application/json, text/event-stream"
                .parse()
                .expect("static Accept header is valid"),
        );
        if let Some(sid) = self.session_id.lock().await.as_deref()
            && let Ok(v) = sid.parse()
        {
            headers.insert("Mcp-Session-Id", v);
        }
        if let Some(cred) = credential
            && let Ok(v) = format!("Bearer {}", cred.access_token).parse()
        {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
        headers
    }

    /// Capture `Mcp-Session-Id` from a response header if present.
    async fn capture_session_id(&self, resp: &reqwest::Response) {
        if let Some(v) = resp.headers().get("Mcp-Session-Id")
            && let Ok(s) = v.to_str()
        {
            *self.session_id.lock().await = Some(s.to_string());
        }
    }

    /// POST `json` once and return the response.
    async fn post_once(
        &self,
        json: &str,
        credential: Option<&Credential>,
    ) -> Result<reqwest::Response> {
        let headers = self.build_headers(credential).await;
        self.client
            .post(&self.endpoint)
            .headers(headers)
            .header("Content-Type", "application/json")
            .body(json.to_string())
            .send()
            .await
            .context("MCP Streamable HTTP POST failed")
    }

    /// Dispatch an inbound message (notification or server→client
    /// request) to the installed handler.
    fn dispatch_inbound(&self, msg: RawJsonRpcMessage) {
        if let Some(h) = self.inbound_handler.lock().as_mut() {
            h(msg);
        }
    }
}

#[async_trait::async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(&mut self, id: u64, json: &str) -> Result<RawJsonRpcMessage> {
        if *self.closed.lock().await {
            anyhow::bail!("MCP HTTP transport closed");
        }

        let credential = match self.credential_provider.as_ref() {
            Some(p) => p.access_token(&self.server_name, &self.endpoint).await,
            None => None,
        };

        let resp = self.post_once(json, credential.as_ref()).await?;
        let status = resp.status();

        // First 2xx response (the `initialize`) carries `Mcp-Session-Id`.
        if status.is_success() {
            self.capture_session_id(&resp).await;
        }

        // Refresh-on-401/403 with exactly one retry.
        if (status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN)
            && let Some(provider) = self.credential_provider.as_ref()
        {
            // Drop the failed response body.
            drop(resp);
            let refreshed = provider.refresh(&self.server_name, &self.endpoint).await;
            let credential2 = match refreshed {
                Some(_) => {
                    provider
                        .access_token(&self.server_name, &self.endpoint)
                        .await
                }
                None => credential,
            };
            let resp2 = self.post_once(json, credential2.as_ref()).await?;
            let status2 = resp2.status();
            if status2.is_success() {
                self.capture_session_id(&resp2).await;
            }
            if !status2.is_success() {
                anyhow::bail!(
                    "MCP HTTP request failed after credential refresh: {} {}",
                    status2.as_u16(),
                    status2.canonical_reason().unwrap_or("")
                );
            }
            return self.handle_response(resp2, id).await;
        }

        if !status.is_success() {
            anyhow::bail!(
                "MCP HTTP error {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            );
        }

        self.handle_response(resp, id).await
    }

    async fn notify(&mut self, json: &str) -> Result<()> {
        if *self.closed.lock().await {
            anyhow::bail!("MCP HTTP transport closed");
        }
        let credential = match self.credential_provider.as_ref() {
            Some(p) => p.access_token(&self.server_name, &self.endpoint).await,
            None => None,
        };
        let resp = self.post_once(json, credential.as_ref()).await?;
        let status = resp.status();
        if status.is_success() {
            self.capture_session_id(&resp).await;
        }
        if !status.is_success() {
            anyhow::bail!(
                "MCP HTTP notify failed: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            );
        }
        Ok(())
    }

    fn set_inbound_handler(&mut self, handler: InboundHandler) {
        *self.inbound_handler.lock() = Some(handler);
    }

    async fn close(&mut self) -> Result<()> {
        *self.closed.lock().await = true;
        // Best-effort DELETE to terminate the session.
        let sid = self.session_id.lock().await.clone();
        if let Some(sid) = sid
            && let Ok(v) = sid.parse::<reqwest::header::HeaderValue>()
        {
            let _ = self
                .client
                .delete(&self.endpoint)
                .header("Mcp-Session-Id", v)
                .send()
                .await;
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        !*self.closed.blocking_lock()
    }
}

impl StreamableHttpTransport {
    /// Process a 2xx response and return the JSON-RPC message whose id
    /// matches `id`. Any non-matching messages encountered (notifications
    /// or server→client requests interleaved on the SSE stream) are
    /// dispatched to the inbound handler.
    async fn handle_response(&self, resp: reqwest::Response, id: u64) -> Result<RawJsonRpcMessage> {
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if ct.starts_with("text/event-stream") {
            let mut stream = resp.bytes_stream();
            let mut buffer: Vec<u8> = Vec::new();
            let timeout = self.timeout;
            let resolved = tokio::time::timeout(timeout, async {
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.context("MCP HTTP SSE chunk read failed")?;
                    buffer.extend_from_slice(&chunk);
                    while let Some((event, consumed)) = parse_sse_event(&buffer) {
                        let rest = buffer.split_off(consumed);
                        buffer = rest;
                        let Some(data) = event else { continue };
                        let msg: RawJsonRpcMessage = match serde_json::from_slice(&data) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                        if msg.id == Some(id) {
                            return Ok(msg);
                        }
                        self.dispatch_inbound(msg);
                    }
                }
                Err::<RawJsonRpcMessage, _>(anyhow::anyhow!(
                    "MCP HTTP SSE response ended without matching id"
                ))
            })
            .await
            .map_err(|_| {
                anyhow::anyhow!("MCP HTTP request timed out after {:?}", self.timeout)
            })??;
            return Ok(resolved);
        }

        // application/json (or missing/unknown): single JSON-RPC response.
        let body = resp
            .bytes()
            .await
            .context("Failed to read MCP HTTP response body")?;
        let msg: RawJsonRpcMessage =
            serde_json::from_slice(&body).context("Failed to parse MCP HTTP response JSON")?;
        if msg.id != Some(id) {
            self.dispatch_inbound(msg);
            anyhow::bail!("MCP HTTP returned response with non-matching id");
        }
        Ok(msg)
    }
}

/// Parse one SSE event from `buffer`. Returns
/// `(Option<Vec<u8>>, usize)` where the `Option<Vec<u8>>` is the
/// concatenated `data:` payload for the event (without the `data: `
/// prefix) or `None` if the event had no data fields, and `usize` is
/// the number of bytes consumed (including the trailing blank line).
/// Returns `None` if `buffer` does not yet contain a complete event
/// delimiter (`\n\n`).
fn parse_sse_event(buffer: &[u8]) -> Option<(Option<Vec<u8>>, usize)> {
    let delim = find_sse_delim(buffer)?;
    let event_bytes = &buffer[..delim];
    let mut data: Option<Vec<u8>> = None;
    for line in event_bytes.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.starts_with(b"data:") {
            let rest = if line.len() > 4 && line[4] == b' ' {
                &line[5..]
            } else if line.len() > 4 {
                &line[4..]
            } else {
                &[][..]
            };
            let entry = data.get_or_insert_with(Vec::new);
            if !entry.is_empty() {
                entry.push(b'\n');
            }
            entry.extend_from_slice(rest);
        }
        // `event:` and `id:` lines are ignored in v2.1.
    }
    Some((data, delim + 2))
}

fn find_sse_delim(buffer: &[u8]) -> Option<usize> {
    (0..buffer.len().saturating_sub(1)).find(|&i| buffer[i] == b'\n' && buffer[i + 1] == b'\n')
}
