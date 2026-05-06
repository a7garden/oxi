//! RPC server state management.

use base64::Engine;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

use super::protocol::*;

/// RPC server for headless operation
pub struct RpcServer {
    /// Port for TCP mode (0 for stdio mode)
    port: u16,
    /// Shutdown flag
    shutdown: Arc<parking_lot::RwLock<bool>>,
    /// Session state
    session_state: Arc<parking_lot::RwLock<SessionState>>,
    /// Pending extension UI requests
    pending_extension_requests: Arc<Mutex<Vec<(String, oneshot::Sender<RpcExtensionUiResponse>)>>>,
    /// Event subscribers
    event_tx: mpsc::UnboundedSender<RpcEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<RpcEvent>>,
}

impl RpcServer {
    /// Create a new RPC server
    pub fn new(port: u16) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            port,
            shutdown: Arc::new(parking_lot::RwLock::new(false)),
            session_state: Arc::new(parking_lot::RwLock::new(SessionState {
                model: None,
                thinking_level: "default".to_string(),
                is_streaming: false,
                is_compacting: false,
                steering_mode: "all".to_string(),
                follow_up_mode: "all".to_string(),
                session_file: None,
                session_id: uuid::Uuid::new_v4().to_string(),
                session_name: None,
                auto_compaction_enabled: true,
                message_count: 0,
                pending_message_count: 0,
            })),
            pending_extension_requests: Arc::new(Mutex::new(Vec::new())),
            event_tx,
            event_rx: Some(event_rx),
        }
    }

    /// Get the port (0 for stdio mode)
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Check if shutdown has been requested
    pub fn is_shutdown_requested(&self) -> bool {
        *self.shutdown.read()
    }

    /// Request shutdown
    pub fn request_shutdown(&self) {
        *self.shutdown.write() = true;
    }

    /// Update session state
    pub fn update_session_state<F>(&self, f: F)
    where
        F: FnOnce(&mut SessionState),
    {
        let mut state = self.session_state.write();
        f(&mut state);
    }

    /// Get current session state
    pub fn get_session_state(&self) -> SessionState {
        self.session_state.read().clone()
    }

    /// Emit an event to subscribers
    pub fn emit_event(&self, event: RpcEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<RpcEvent>> {
        self.event_rx.take()
    }

    /// Register a pending extension UI request and return a oneshot receiver
    pub async fn register_extension_request(
        &self,
        id: String,
    ) -> oneshot::Receiver<RpcExtensionUiResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending_extension_requests
            .lock()
            .await
            .push((id, tx));
        rx
    }

    /// Resolve a pending extension UI request
    pub async fn resolve_extension_request(
        &self,
        id: &str,
        response: RpcExtensionUiResponse,
    ) -> bool {
        let mut pending = self.pending_extension_requests.lock().await;
        if let Some(pos) = pending.iter().position(|(req_id, _)| req_id == id) {
            let (_, sender) = pending.remove(pos);
            let _ = sender.send(response);
            true
        } else {
            false
        }
    }

    /// Parse image data from RPC command
    pub fn parse_images(images: Option<Vec<ImageData>>) -> Vec<RpcImageSource> {
        images
            .unwrap_or_default()
            .into_iter()
            .filter_map(|img| {
                if img.source.starts_with("data:") {
                    let parts: Vec<&str> = img.source.splitn(2, ',').collect();
                    if parts.len() == 2 {
                        let base64_data = parts[1].split(';').next().unwrap_or(parts[1]);
                        if let Ok(decoded) =
                            base64::engine::general_purpose::STANDARD.decode(base64_data)
                        {
                            return Some(RpcImageSource {
                                data: decoded,
                                mime_type: img.media_type,
                            });
                        }
                    }
                }
                None
            })
            .collect()
    }
}
