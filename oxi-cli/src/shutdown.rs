//! Graceful shutdown 조정자.
//!
//! Ctrl+C를 2단계로 처리:
//! 1차: 진행 중인 작업 완료 후 종료
//! 2차: 즉시 종료

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const FORCED: u8 = 2;

/// Shutdown 신호
#[derive(Debug, Clone)]
pub enum ShutdownSignal {
    /// 정상 종료. 진행 중인 작업 완료 후 종료.
    Graceful,
    /// 강제 종료.
    Force,
}

/// Graceful shutdown 조정자.
pub struct ShutdownCoordinator {
    tx: broadcast::Sender<ShutdownSignal>,
    state: Arc<AtomicU8>,
}

impl ShutdownCoordinator {
    /// 새 조정자 생성.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(2);
        Self {
            tx,
            state: Arc::new(AtomicU8::new(RUNNING)),
        }
    }

    /// SIGINT 리스너 시작.
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
                tracing::info!("Graceful shutdown 요청 (다시 Ctrl+C로 강제 종료)");
                let _ = tx.send(ShutdownSignal::Graceful);
            }

            // 두 번째 Ctrl+C
            tokio::signal::ctrl_c().await.ok();
            state.store(FORCED, Ordering::SeqCst);
            tracing::warn!("강제 종료 요청");
            let _ = tx.send(ShutdownSignal::Force);
        });
    }

    /// shutdown 신호 구독.
    pub fn subscribe(&self) -> broadcast::Receiver<ShutdownSignal> {
        self.tx.subscribe()
    }

    /// 현재 draining 상태인지 확인.
    pub fn is_draining(&self) -> bool {
        self.state.load(Ordering::SeqCst) >= DRAINING
    }

    /// 현재 강제 종료 상태인지 확인.
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
