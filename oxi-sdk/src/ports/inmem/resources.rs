//! `ResourceMonitor` backed by `sysinfo` (optional dependency-free stub here).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::SdkError;
use crate::ports::{ResourceMonitor, ResourceUsage};

/// Resource monitor that tracks counters set by the application.
///
/// This impl does **not** read OS-level stats (no `sysinfo` dep). It
/// tracks:
/// - active_agents (counter, atomic)
/// - tokens_consumed (counter, atomic)
///
/// CPU/memory/disk default to zero. For OS-level monitoring, implement
/// `ResourceMonitor` directly using `sysinfo` or `/proc`.
pub struct CountingResourceMonitor {
    active_agents: Arc<AtomicU64>,
    tokens: Arc<AtomicU64>,
}

impl std::fmt::Debug for CountingResourceMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CountingResourceMonitor").finish()
    }
}

impl Default for CountingResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl CountingResourceMonitor {
    pub fn new() -> Self {
        Self {
            active_agents: Arc::new(AtomicU64::new(0)),
            tokens: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn inc_active(&self) {
        self.active_agents.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active(&self) {
        self.active_agents.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn add_tokens(&self, n: u64) {
        self.tokens.fetch_add(n, Ordering::Relaxed);
    }
}

impl ResourceMonitor for CountingResourceMonitor {
    fn snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceUsage, SdkError>> + Send + '_>> {
        let active = self.active_agents.load(Ordering::Relaxed) as usize;
        let tokens = self.tokens.load(Ordering::Relaxed);
        Box::pin(async move {
            Ok(ResourceUsage {
                cpu_percent: 0.0,
                memory_bytes: 0,
                disk_bytes: 0,
                active_agents: active,
                tokens_consumed: tokens,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counters_increment() {
        let m = CountingResourceMonitor::new();
        m.inc_active();
        m.inc_active();
        m.add_tokens(100);
        let snap = m.snapshot().await.unwrap();
        assert_eq!(snap.active_agents, 2);
        assert_eq!(snap.tokens_consumed, 100);
        m.dec_active();
        let snap = m.snapshot().await.unwrap();
        assert_eq!(snap.active_agents, 1);
    }
}
