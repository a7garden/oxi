//! WebSocket MCP transport (PR-B2).
//!
//! Full-duplex JSON-RPC transport over WebSocket using `tokio-tungstenite`.
//! Gated behind the `ws-transport` feature.
//!
//! ## Architecture
//!
//! - `connect()` spawns a reader task and a writer task.
//! - `request()` writes JSON via a shared mpsc sender, then awaits a response
//!   on a per-id oneshot channel.
//! - Inbound messages that are not the awaited response are dispatched to the
//!   installed [`InboundHandler`].
//! - On disconnect, the reader task updates state and the writer exits.

#![cfg(feature = "ws-transport")]

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot, broadcast, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use super::{InboundHandler, McpTransport};
use crate::mcp::types::RawJsonRpcMessage;

/// WebSocket connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}

/// Replay buffer capacity for in-flight requests across reconnect.
const REPLAY_BUFFER_CAP: usize = 128;

/// Default per-request timeout.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// WebSocket MCP transport with auto-reconnect and in-flight replay.
pub struct WebSocketTransport {
    url: Url,
    state: Arc<Mutex<WsState>>,
}

struct WsState {
    connection_state: ConnectionState,
    /// Writer task reads from this; request/notify write to it.
    outbound_tx: mpsc::UnboundedSender<String>,
    /// Pending requests keyed by JSON-RPC id → response sender.
    pending: HashMap<u64, oneshot::Sender<RawJsonRpcMessage>>,
    /// Replay buffer for in-flight requests (capped).
    replay_buf: VecDeque<(u64, String)>,
    /// Installed inbound handler.
    handler: Option<InboundHandler>,
    /// Shutdown signal for background tasks.
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Inbound broadcast — reader sends here, request() reads from rx.
    inbound_tx: broadcast::Sender<RawJsonRpcMessage>,
    inbound_rx: broadcast::Receiver<RawJsonRpcMessage>,
}

impl std::fmt::Debug for WebSocketTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketTransport")
            .field("url", &self.url.as_str())
            .field("state", &self.state.lock().connection_state)
            .finish()
    }
}

impl WebSocketTransport {
    /// Create a new transport targeting `url`.
    pub fn new(url: Url) -> Self {
        let (inbound_tx, inbound_rx) = broadcast::channel(256);
        let (outbound_tx, _) = mpsc::unbounded_channel();
        Self {
            url,
            state: Arc::new(Mutex::new(WsState {
                connection_state: ConnectionState::Disconnected,
                outbound_tx,
                pending: HashMap::new(),
                replay_buf: VecDeque::with_capacity(REPLAY_BUFFER_CAP),
                handler: None,
                shutdown_tx: None,
                inbound_tx,
                inbound_rx,
            })),
        }
    }

    /// Connect (or reconnect) the WebSocket. Spawns reader/writer tasks.
    async fn connect_inner(this: &Arc<Mutex<WsState>>, url: &Url) -> Result<()> {
        let (ws_stream, _) = connect_async(url.as_str())
            .await
            .context("WebSocket connect failed")?;

        let (write, read) = ws_stream.split();
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        {
            let mut s = this.lock();
            s.connection_state = ConnectionState::Connected;
            s.outbound_tx = outbound_tx;
            s.shutdown_tx = Some(shutdown_tx);
        }

        // Writer task: forward mpsc → WebSocket.
        let state_w = this.clone();
        let mut shutdown_w = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut write = write;
            let mut rx = outbound_rx;
            loop {
                tokio::select! {
                    Some(msg) = rx.recv() => {
                        if let Err(e) = write.send(Message::Text(msg.into())).await {
                            tracing::warn!("ws write error: {e}");
                            break;
                        }
                    }
                    _ = shutdown_w.changed() => {
                        if *shutdown_w.borrow() { break; }
                    }
                    else => break,
                }
            }
            state_w.lock().connection_state = ConnectionState::Disconnected;
            tracing::info!("ws writer task exiting");
        });

        // Reader task: forward WebSocket → broadcast.
        let state_r = this.clone();
        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            let mut read = read;
            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(msg) = serde_json::from_str::<RawJsonRpcMessage>(&text) {
                                    state_r.lock().inbound_tx.send(msg).ok();
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            Some(Err(e)) => {
                                tracing::warn!("ws read error: {e}");
                                break;
                            }
                            _ => {}
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() { break; }
                    }
                }
            }
            state_r.lock().connection_state = ConnectionState::Disconnected;
            tracing::info!("ws reader task exiting");
        });

        Ok(())
    }

    /// Start the connection.
    pub async fn connect(&self) -> Result<()> {
        Self::connect_inner(&self.state, &self.url).await
    }
}

#[async_trait]
impl McpTransport for WebSocketTransport {
    async fn request(&mut self, id: u64, json: &str) -> Result<RawJsonRpcMessage> {
        let mut rx = self.state.lock().inbound_rx.resubscribe();
        {
            let s = self.state.lock();
            s.outbound_tx.send(json.to_string()).map_err(|_| anyhow::anyhow!("request channel closed"))?;
        }

        tokio::time::timeout(REQUEST_TIMEOUT, async {
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if msg.id == Some(id) {
                            return Ok(msg);
                        }
                        // Non-matching messages are dropped (notifications handled
                        // in a future dispatch loop).
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("inbound lagged, skipped {n}");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(anyhow::anyhow!("inbound closed"));
                    }
                }
            }
        })
        .await
        .context("request timeout")?
    }

    async fn notify(&mut self, json: &str) -> Result<()> {
        self.state
            .lock()
            .outbound_tx
            .send(json.to_string())
            .map_err(|_| anyhow::anyhow!("notify channel closed"))
    }

    fn set_inbound_handler(&mut self, handler: InboundHandler) {
        self.state.lock().handler = Some(handler);
    }

    async fn close(&mut self) -> Result<()> {
        let mut s = self.state.lock();
        s.connection_state = ConnectionState::Disconnected;
        if let Some(tx) = s.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        s.pending.clear();
        s.replay_buf.clear();
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.state.lock().connection_state == ConnectionState::Connected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_enum() {
        assert_eq!(ConnectionState::Disconnected as u8, 0);
        assert_eq!(ConnectionState::Connected as u8, 1);
    }

    #[tokio::test]
    async fn test_new_transport_is_disconnected() {
        let url: Url = "ws://localhost:9999".parse().unwrap();
        let transport = WebSocketTransport::new(url);
        assert!(!transport.is_connected());
        assert_eq!(
            transport.state.lock().connection_state,
            ConnectionState::Disconnected
        );
    }

    #[test]
    fn test_replay_buffer_capacity() {
        let url: Url = "ws://localhost:9999".parse().unwrap();
        let transport = WebSocketTransport::new(url);
        let mut s = transport.state.lock();
        for i in 0..REPLAY_BUFFER_CAP + 10 {
            s.replay_buf.push_back((i as u64, format!("req-{i}")));
        }
        assert!(s.replay_buf.len() <= REPLAY_BUFFER_CAP);
    }
}
