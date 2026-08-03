//! Display metadata for the Agent Hub overlay.
//!
//! Parallel to `oxicode_sdk::AgentPool` but stores TUI display fields
//! (kind, status, last_activity, current_task, session_file) keyed by
//! the same agent id. AgentPool is the runtime owner; HubRegistry is
//! the display projection.
// Foundation module — consumers land in Tasks 3-10 (overlay + bridge +
// slash command). Until then nothing in this crate calls into the public
// surface, so silence dead-code on the lib build; tests still cover it.
#![allow(dead_code)]
use oxicode_sdk::{HubKind, HubStatus};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// One row in the Hub table.
#[derive(Debug, Clone)]
pub struct HubEntry {
    pub kind: HubKind,
    pub status: HubStatus,
    pub display_name: String,
    pub current_task: Option<String>,
    pub last_activity_ms: u64,
    pub session_file: Option<PathBuf>,
}

impl HubEntry {
    /// "3s ago" / "5m ago" / "1h ago" formatting — millisecond-precise,
    /// minute-precise, hour-precise tiers.
    pub fn age_text(&self, now_ms: u64) -> String {
        let delta = now_ms.saturating_sub(self.last_activity_ms);
        if delta < 60_000 {
            format!("{}s ago", delta / 1000)
        } else if delta < 3_600_000 {
            format!("{}m ago", delta / 60_000)
        } else {
            format!("{}h ago", delta / 3_600_000)
        }
    }
}

/// Concurrent map of agent id → display entry.
#[derive(Debug, Default)]
pub struct HubRegistry {
    inner: RwLock<HashMap<String, HubEntry>>,
}

impl HubRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the entry for `id`.
    pub fn register(&self, id: String, entry: HubEntry) {
        self.inner.write().insert(id, entry);
    }

    /// Apply `f` to the entry for `id`. No-op if absent.
    pub fn update<F: FnOnce(&mut HubEntry)>(&self, id: &str, f: F) {
        if let Some(e) = self.inner.write().get_mut(id) {
            f(e);
        }
    }

    pub fn unregister(&self, id: &str) -> Option<HubEntry> {
        self.inner.write().remove(id)
    }

    /// Sorted snapshot: Running → Idle → Parked → Aborted; within status,
    /// by descending last_activity_ms.
    pub fn snapshot(&self) -> Vec<(String, HubEntry)> {
        let mut v: Vec<(String, HubEntry)> = self
            .inner
            .read()
            .iter()
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        v.sort_by(|a, b| {
            a.1.status
                .sort_key()
                .cmp(&b.1.status.sort_key())
                .then_with(|| b.1.last_activity_ms.cmp(&a.1.last_activity_ms))
        });
        v
    }
}

/// UNIX epoch milliseconds. Returns 0 on platforms where the system
/// clock is configured before the epoch (effectively impossible in
/// practice, but a defensive fallback keeps callers off the Option
/// hot-path).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `Arc<HubRegistry>` shorthand.
pub type SharedHubRegistry = Arc<HubRegistry>;

#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_sdk::{HubKind, HubStatus};

    fn entry(kind: HubKind, status: HubStatus) -> HubEntry {
        HubEntry {
            kind,
            status,
            display_name: "test".into(),
            current_task: None,
            last_activity_ms: 0,
            session_file: None,
        }
    }

    #[test]
    fn register_and_snapshot() {
        let r = HubRegistry::new();
        r.register("a".into(), entry(HubKind::Main, HubStatus::Running));
        r.register("b".into(), entry(HubKind::Advisor, HubStatus::Idle));
        let snap = r.snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn snapshot_sorts_running_first_then_idle() {
        let r = HubRegistry::new();
        r.register("idle".into(), entry(HubKind::Subagent, HubStatus::Idle));
        r.register("running".into(), entry(HubKind::Main, HubStatus::Running));
        r.register("parked".into(), entry(HubKind::Subagent, HubStatus::Parked));
        r.register(
            "aborted".into(),
            entry(HubKind::Subagent, HubStatus::Aborted),
        );
        let snap = r.snapshot();
        assert_eq!(snap[0].0, "running");
        assert_eq!(snap[1].0, "idle");
        assert_eq!(snap[2].0, "parked");
        assert_eq!(snap[3].0, "aborted");
    }

    #[test]
    fn update_touches_last_activity() {
        let r = HubRegistry::new();
        r.register("a".into(), entry(HubKind::Main, HubStatus::Running));
        r.update("a", |e| e.last_activity_ms = 100);
        let snap = r.snapshot();
        assert_eq!(snap[0].1.last_activity_ms, 100);
    }

    #[test]
    fn unregister_removes() {
        let r = HubRegistry::new();
        r.register("a".into(), entry(HubKind::Main, HubStatus::Running));
        r.unregister("a");
        assert!(r.snapshot().is_empty());
    }
}
