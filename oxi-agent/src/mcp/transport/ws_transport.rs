//! WebSocket MCP transport 스켈레톤 (PR-B1).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::{InboundHandler, McpTransport};
use crate::mcp::types::RawJsonRpcMessage;

/// WebSocket 연결 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// 초기 상태. 연결 안 됨.
    Disconnected,
    /// 핸드셰이크 진행 중 (PR-B2 부터).
    Connecting,
    /// 연결됨 — 메시지 송수신 가능 (PR-B2 부터).
    Connected,
    /// 끊어짐 — 자동 재연결 대기 중 (PR-B2 부터).
    Reconnecting,
}

impl ConnectionState {
    /// Connected 상태 여부.
    pub fn is_connected(self) -> bool {
        matches!(self, ConnectionState::Connected)
    }
}

/// Replay buffer 최대 크기 (in-flight 요청).
const REPLAY_BUFFER_CAP: usize = 128;

/// WebSocket MCP transport 스켈레톤.
///
/// PR-B1 에서는 상태 머신과 메시지 큐만 노출. 실제 I/O는 PR-B2.
pub struct WebSocketTransport {
    /// WebSocket URL (`wss://...` 또는 `ws://...`).
    url: String,
    /// 인증 — PR-B2 에서 사용.
    #[allow(dead_code)]
    credential: Option<Arc<dyn crate::mcp::auth::McpCredentialProvider>>,
    /// 연결 상태.
    state: Arc<Mutex<ConnectionState>>,
    /// In-flight 요청 → 응답 oneshot.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RawJsonRpcMessage>>>>,
    /// 재연결 시 재전송할 in-flight 요청 (id + JSON).
    replay_buf: Arc<Mutex<VecDeque<(u64, String)>>>,
    /// 알림 + server→client 요청 핸들러.
    inbound_handler: Arc<Mutex<Option<InboundHandler>>>,
}

impl std::fmt::Debug for WebSocketTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketTransport")
            .field("url", &self.url)
            .field("state", &*self.state.lock())
            .field("pending", &self.pending.lock().len())
            .field("replay_buf_len", &self.replay_buf.lock().len())
            .finish()
    }
}

impl WebSocketTransport {
    /// 새 transport 생성 (Disconnected 상태).
    pub fn new(
        url: impl Into<String>,
        credential: Option<Arc<dyn crate::mcp::auth::McpCredentialProvider>>,
    ) -> Self {
        Self {
            url: url.into(),
            credential,
            state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            replay_buf: Arc::new(Mutex::new(VecDeque::new())),
            inbound_handler: Arc::new(Mutex::new(None)),
        }
    }

    /// 현재 상태.
    pub fn state(&self) -> ConnectionState {
        *self.state.lock()
    }

    /// WebSocket URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// In-flight 요청 수.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }

    /// Replay buffer 크기.
    pub fn replay_buffer_len(&self) -> usize {
        self.replay_buf.lock().len()
    }

    /// 연결 시도 — PR-B1 에서는 stub. PR-B2 부터 TCP+WS 핸드셰이크.
    pub async fn connect(&self) -> Result<()> {
        *self.state.lock() = ConnectionState::Connecting;
        Err(anyhow!(
            "WebSocket I/O not yet wired (PR-B2); url={}",
            self.url
        ))
    }

    /// 연결 종료 — pending 응답 모두 폐기.
    pub async fn close(&self) -> Result<()> {
        *self.state.lock() = ConnectionState::Disconnected;
        self.pending.lock().clear();
        Ok(())
    }
}

#[async_trait]
impl McpTransport for WebSocketTransport {
    async fn request(&mut self, id: u64, json: &str) -> Result<RawJsonRpcMessage> {
        if matches!(*self.state.lock(), ConnectionState::Disconnected) {
            self.connect().await?;
        }
        // (1) Save into replay buffer.
        {
            let mut buf = self.replay_buf.lock();
            buf.push_back((id, json.to_string()));
            if buf.len() > REPLAY_BUFFER_CAP {
                buf.pop_front();
            }
        }
        // (2) Register pending (registered for future PR-B2 use; dropped on error).
        let (_tx, _rx) = oneshot::channel();
        self.pending.lock().insert(id, _tx);

        // (3) Wire write — PR-B2 부터. 현재는 에러 반환.
        self.pending.lock().remove(&id);
        Err(anyhow!(
            "WebSocket wire I/O not yet implemented (PR-B2); id={id}"
        ))
    }

    async fn notify(&mut self, _json: &str) -> Result<()> {
        if matches!(*self.state.lock(), ConnectionState::Disconnected) {
            self.connect().await?;
        }
        Err(anyhow!("WebSocket wire I/O not yet implemented (PR-B2)"))
    }

    fn set_inbound_handler(&mut self, handler: InboundHandler) {
        *self.inbound_handler.lock() = Some(handler);
    }

    async fn close(&mut self) -> Result<()> {
        WebSocketTransport::close(self).await
    }

    fn is_connected(&self) -> bool {
        self.state().is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_disconnected() {
        let ws = WebSocketTransport::new("wss://example/mcp", None);
        assert_eq!(ws.state(), ConnectionState::Disconnected);
        assert_eq!(ws.pending_count(), 0);
        assert_eq!(ws.replay_buffer_len(), 0);
        assert!(!ws.is_connected());
    }

    #[test]
    fn url_stored() {
        let ws = WebSocketTransport::new("wss://example.com/mcp", None);
        assert_eq!(ws.url(), "wss://example.com/mcp");
    }

    #[test]
    fn state_predicates_distinct() {
        assert!(!ConnectionState::Disconnected.is_connected());
        assert!(!ConnectionState::Connecting.is_connected());
        assert!(ConnectionState::Connected.is_connected());
        assert!(!ConnectionState::Reconnecting.is_connected());
    }

    #[tokio::test]
    async fn connect_returns_not_wired_error_in_pr_b1() {
        let ws = WebSocketTransport::new("wss://example/mcp", None);
        let res = ws.connect().await;
        match res {
            Ok(()) => panic!("connect should fail in PR-B1"),
            Err(e) => assert!(
                e.to_string().contains("WebSocket I/O not yet wired"),
                "unexpected error: {e}"
            ),
        }
    }

    #[tokio::test]
    async fn request_returns_not_wired_error() {
        let mut ws = WebSocketTransport::new("wss://example/mcp", None);
        let res = ws
            .request(1, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .await;
        assert!(res.is_err(), "request should fail until PR-B2");
    }

    #[tokio::test]
    async fn notify_returns_not_wired_error() {
        let mut ws = WebSocketTransport::new("wss://example/mcp", None);
        let res = ws
            .notify(r#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#)
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn close_clears_state_to_disconnected() {
        let ws = WebSocketTransport::new("wss://example/mcp", None);
        *ws.state.lock() = ConnectionState::Connecting;
        WebSocketTransport::close(&ws).await.expect("close ok");
        assert_eq!(ws.state(), ConnectionState::Disconnected);
    }
}
