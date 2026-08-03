//! Channel-based MCP server lifecycle management.
//!
//! The previous design attempted to put lifecycle timers (idle disconnect,
//! keep-alive health checks) inside the same `McpManagerInner` that is guarded
//! by a `tokio::sync::Mutex`. This caused a deadlock risk: an idle timer
//! callback firing on a background task would need to re-acquire the mutex,
//! while the agent thread might also be waiting for the callback.
//!
//! To break this, lifecycle events are sent over an `mpsc` channel to a
//! dedicated background task that owns the timer handles. The background
//! task only holds a `Weak<McpManager>` (no reference cycles) and re-enters
//! the manager through a public async method that acquires the mutex on its
//! own — which can never deadlock with the agent thread because the agent
//! thread is not waiting for the lifecycle task while holding the lock.
//!
//! ```text
//!   [Agent task]                [Lifecycle task]            [McpManager]
//!        │                            │                          │
//!        │ call_tool()                │                           │
//!        ├─ lock(inner) ──────────────┼──────────────────────────► │
//!        │ do work                    │                           │
//!        ├─ unlock                    │                           │
//!        │                            │                           │
//!        │                            │ (idle timer fires)        │
//!        │                            ├─ Weak::upgrade() ───────► │
//!        │                            │ disconnect_server()       │
//!        │                            │   ├─ lock(inner) ───────► │
//!        │                            │   ├─ unlock              │
//!        │                            │                           │
//!        │ ← no contention: agent is not holding the lock when    │
//!        │   the lifecycle task tries to acquire it.              │
//! ```

use std::collections::HashMap;
use std::sync::Weak;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::mcp::McpManager;

/// Lifecycle events sent from [`McpManager`] to the background task.
#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    /// Start (or reset) an idle-disconnect timer for a server.
    StartIdleTimer {
        /// Name of the server to schedule the timer for.
        server: String,
        /// Idle duration before the server is disconnected.
        timeout: Duration,
    },
    /// Cancel any pending idle-disconnect timer for a server.
    CancelIdleTimer {
        /// Name of the server whose timer should be cancelled.
        server: String,
    },
    /// Start periodic health checks for a keep-alive server.
    StartHealthCheck {
        /// Name of the server to health-check.
        server: String,
    },
    /// Stop all timers for a server (it was disconnected).
    ServerStopped {
        /// Name of the server that was stopped.
        server: String,
    },
    /// Stop everything (process shutdown).
    Shutdown,
}

/// Sender side of the lifecycle channel.
pub type LifecycleTx = mpsc::UnboundedSender<LifecycleEvent>;
/// Receiver side of the lifecycle channel.
pub type LifecycleRx = mpsc::UnboundedReceiver<LifecycleEvent>;

/// Create a new lifecycle event channel.
pub fn channel() -> (LifecycleTx, LifecycleRx) {
    mpsc::unbounded_channel()
}

/// The background task that owns all timer handles.
///
/// Created by [`McpManager::spawn()`]. Exits when the `LifecycleRx` returns
/// `None` (the manager was dropped) or when a `Shutdown` event is received.
pub async fn lifecycle_event_loop(mut rx: LifecycleRx, manager: Weak<McpManager>) {
    let mut idle_timers: HashMap<String, JoinHandle<()>> = HashMap::new();
    let mut health_handles: HashMap<String, JoinHandle<()>> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            LifecycleEvent::StartIdleTimer { server, timeout } => {
                if let Some(h) = idle_timers.remove(&server) {
                    h.abort();
                }
                let mgr = manager.clone();
                let srv = server.clone();
                idle_timers.insert(
                    server,
                    tokio::spawn(async move {
                        tokio::time::sleep(timeout).await;
                        if let Some(m) = mgr.upgrade() {
                            // Best-effort: log but never panic on disconnect
                            // failure (the agent might be using the server).
                            if let Err(e) = m.disconnect_server(&srv).await {
                                tracing::debug!("MCP: idle-disconnect for '{}' failed: {}", srv, e);
                            }
                        }
                    }),
                );
            }
            LifecycleEvent::CancelIdleTimer { server } => {
                if let Some(h) = idle_timers.remove(&server) {
                    h.abort();
                }
            }
            LifecycleEvent::StartHealthCheck { server } => {
                if let Some(h) = health_handles.remove(&server) {
                    h.abort();
                }
                let mgr = manager.clone();
                let srv = server.clone();
                health_handles.insert(
                    server,
                    tokio::spawn(async move {
                        // Health-check every 30s; on failure, attempt one
                        // reconnect and stop on success.
                        let interval = Duration::from_secs(30);
                        loop {
                            tokio::time::sleep(interval).await;
                            let Some(m) = mgr.upgrade() else {
                                break;
                            };
                            match m.health_check_and_reconnect(&srv).await {
                                Ok(()) => continue,
                                Err(e) => {
                                    tracing::warn!(
                                        "MCP: health check for keep-alive server '{}' failed: {}",
                                        srv,
                                        e
                                    );
                                    // Give up after a single reconnect attempt
                                    // (we don't want to spam the system log).
                                    // The next tool call will trigger lazy
                                    // reconnection.
                                    break;
                                }
                            }
                        }
                    }),
                );
            }
            LifecycleEvent::ServerStopped { server } => {
                if let Some(h) = idle_timers.remove(&server) {
                    h.abort();
                }
                if let Some(h) = health_handles.remove(&server) {
                    h.abort();
                }
            }
            LifecycleEvent::Shutdown => break,
        }
    }

    // Cleanup on exit
    for (_, h) in idle_timers {
        h.abort();
    }
    for (_, h) in health_handles {
        h.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channel_round_trip() {
        let (tx, mut rx) = channel();
        tx.send(LifecycleEvent::CancelIdleTimer {
            server: "test".into(),
        })
        .unwrap();
        let event = rx.recv().await.unwrap();
        match event {
            LifecycleEvent::CancelIdleTimer { server } => assert_eq!(server, "test"),
            _ => panic!("wrong event"),
        }
    }

    #[tokio::test]
    async fn lifecycle_event_loop_runs_to_completion() {
        // Without a real McpManager we just verify that a spawned task runs
        // to completion. The timer callback will see `Weak::upgrade() == None`.
        let (tx, rx) = channel();
        let manager: Weak<McpManager> = Weak::new();
        let task = tokio::spawn(lifecycle_event_loop(rx, manager));

        tx.send(LifecycleEvent::CancelIdleTimer { server: "x".into() })
            .unwrap();

        drop(tx);
        task.await.unwrap();
    }
}
