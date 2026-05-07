//! Graceful shutdown coordinator.
//!
//! Handles Ctrl+C in 2 stages:
//! 1st: Finish pending work, then exit
//! 2nd: Force exit immediately

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const FORCED: u8 = 2;

/// Shutdown signal
#[derive(Debug, Clone)]
pub enum ShutdownSignal {
    /// Graceful. Finish pending work, then exit.
    Graceful,
    /// Force.
}

/// Graceful shutdown coordinator.
pub struct ShutdownCoordinator {
    tx: broadcast::Sender<ShutdownSignal>,
    state: Arc<AtomicU8>,
}

impl ShutdownCoordinator {
    /// Create a new coordinator.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(2);
        Self {
            tx,
            state: Arc::new(AtomicU8::new(RUNNING)),
        }
    }

    /// Start SIGINT listener.
    pub fn listen(&self) {
        let tx = self.tx.clone();
        let state = self.state.clone();

        tokio::spawn(async move {
            // 첫 번째 Ctrl+C
            tokio::signal::ctrl_c().await.ok();
            if state
                .compare_exchange(RUNNING, DRAINING, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                tracing::info!("Graceful shutdown requested (Ctrl+C again to force)");
                let _ = tx.send(ShutdownSignal::Graceful);
            }

            // 두 번째 Ctrl+C
            tokio::signal::ctrl_c().await.ok();
            state.store(FORCED, Ordering::SeqCst);
            tracing::warn!("Forced shutdown requested");
            let _ = tx.send(ShutdownSignal::Force);
        });
    }

    /// Subscribe to shutdown signals.
    pub fn subscribe(&self) -> broadcast::Receiver<ShutdownSignal> {
        self.tx.subscribe()
    }

    /// Check if currently draining.
    pub fn is_draining(&self) -> bool {
        self.state.load(Ordering::SeqCst) >= DRAINING
    }

    /// Check if forced shutdown is active.
    pub fn is_forced(&self) -> bool {
        self.state.load(Ordering::SeqCst) == FORCED
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_running() {
        let coord = ShutdownCoordinator::new();
        assert!(!coord.is_draining());
        assert!(!coord.is_forced());
    }

    #[test]
    fn subscribe_receives_nothing_initially() {
        let coord = ShutdownCoordinator::new();
        let mut rx = coord.subscribe();
        // try_recv should return Err
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn default_creates_running_state() {
        let coord = ShutdownCoordinator::default();
        assert!(!coord.is_draining());
    }
}
