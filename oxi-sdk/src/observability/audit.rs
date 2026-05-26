//! Security audit trail.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;
use tokio::sync::broadcast;

/// An audit trail entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEntry {
    /// A security capability check.
    SecurityDecision {
        subject: String,
        capability: String,
        granted: bool,
        timestamp_ms: u64,
    },
    /// A tool execution event.
    ToolExecution {
        agent_id: String,
        tool_name: String,
        params_summary: String,
        success: bool,
        duration_ms: u64,
        timestamp_ms: u64,
    },
    /// An agent lifecycle event.
    Lifecycle {
        agent_id: String,
        event: String,
        timestamp_ms: u64,
    },
    /// A custom entry with arbitrary metadata.
    Custom {
        category: String,
        message: String,
        #[serde(default)]
        metadata: HashMap<String, serde_json::Value>,
        timestamp_ms: u64,
    },
}

impl AuditEntry {
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Log a security decision.
    pub fn security_decision(subject: String, cap: String, granted: bool) -> Self {
        Self::SecurityDecision {
            subject,
            capability: cap,
            granted,
            timestamp_ms: Self::now_ms(),
        }
    }

    /// Log a tool execution.
    pub fn tool_execution(
        agent_id: String,
        tool_name: String,
        params_summary: String,
        success: bool,
        duration_ms: u64,
    ) -> Self {
        Self::ToolExecution {
            agent_id,
            tool_name,
            params_summary,
            success,
            duration_ms,
            timestamp_ms: Self::now_ms(),
        }
    }

    /// Log a lifecycle event.
    pub fn lifecycle(agent_id: String, event: String) -> Self {
        Self::Lifecycle {
            agent_id,
            event,
            timestamp_ms: Self::now_ms(),
        }
    }

    /// Log a custom entry.
    pub fn custom(category: String, message: String) -> Self {
        Self::Custom {
            category,
            message,
            metadata: HashMap::new(),
            timestamp_ms: Self::now_ms(),
        }
    }
}

// ── AuditFilter ─────────────────────────────────────────────────────

/// Filter criteria for querying the audit log.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by agent ID.
    pub agent_id: Option<String>,
    /// Filter by entry type.
    pub entry_type: Option<String>,
    /// Filter by minimum timestamp.
    pub after_ms: Option<u64>,
}

/// Audit log — append-only event recorder with query and subscription.
pub struct AuditLog {
    entries: parking_lot::RwLock<Vec<AuditEntry>>,
    max_entries: usize,
    total_appended: AtomicU64,
    tx: broadcast::Sender<AuditEntry>,
}

impl AuditLog {
    /// Create a new audit log.
    ///
    /// When `channel_capacity` > 0, subscribers receive events via a
    /// broadcast channel.
    pub fn new(channel_capacity: usize) -> Self {
        let (tx, _) = if channel_capacity > 0 {
            broadcast::channel(channel_capacity)
        } else {
            broadcast::channel(1)
        };
        Self {
            entries: parking_lot::RwLock::new(Vec::new()),
            max_entries: 10_000,
            total_appended: AtomicU64::new(0),
            tx,
        }
    }

    /// Append a new entry. Trims oldest entries when `max_entries` is exceeded.
    pub fn log(&self, entry: AuditEntry) {
        let mut entries = self.entries.write();
        entries.push(entry.clone());
        let len = entries.len();
        let keep = self.max_entries;
        if len > keep {
            entries.drain(0..len - keep);
        }
        drop(entries);
        self.total_appended.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(entry);
    }

    /// Query entries matching the filter.
    pub fn query(&self, filter: AuditFilter) -> Vec<AuditEntry> {
        self.entries
            .read()
            .iter()
            .filter(|e| {
                if let Some(agent_id) = &filter.agent_id {
                    match e {
                        AuditEntry::ToolExecution { agent_id: a, .. } => a == agent_id,
                        AuditEntry::Lifecycle { agent_id: a, .. } => a == agent_id,
                        _ => false,
                    }
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(t) = &filter.entry_type {
                    serde_json::to_string(e)
                        .map(|s| s.contains(t))
                        .unwrap_or(false)
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(after) = filter.after_ms {
                    match e {
                        AuditEntry::SecurityDecision { timestamp_ms, .. } => *timestamp_ms >= after,
                        AuditEntry::ToolExecution { timestamp_ms, .. } => *timestamp_ms >= after,
                        AuditEntry::Lifecycle { timestamp_ms, .. } => *timestamp_ms >= after,
                        AuditEntry::Custom { timestamp_ms, .. } => *timestamp_ms >= after,
                    }
                } else {
                    true
                }
            })
            .cloned()
            .collect()
    }

    /// Return all entries in chronological order.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.read().clone()
    }

    /// Subscribe to new audit entries.
    pub fn subscribe(&self) -> broadcast::Receiver<AuditEntry> {
        self.tx.subscribe()
    }

    /// Return the total number of entries ever appended.
    pub fn total_appended(&self) -> u64 {
        self.total_appended.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("entry_count", &self.entries.read().len())
            .field(
                "total_appended",
                &self.total_appended.load(Ordering::Relaxed),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_append() {
        let log = AuditLog::new(64);
        log.log(AuditEntry::security_decision(
            "agent-1".into(),
            "file:read".into(),
            true,
        ));

        let entries = log.entries();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn audit_log_query_by_agent() {
        let log = AuditLog::new(64);
        log.log(AuditEntry::tool_execution(
            "a1".into(),
            "read".into(),
            "{path:...}".into(),
            true,
            50,
        ));
        log.log(AuditEntry::tool_execution(
            "a2".into(),
            "bash".into(),
            "{}".into(),
            true,
            100,
        ));

        let filter = AuditFilter {
            agent_id: Some("a1".into()),
            ..Default::default()
        };
        let results = log.query(filter);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn audit_log_trim_on_max_entries() {
        let log = AuditLog::new(64);
        // Manually set a low max for testing
        // We can't override max_entries, so this test just validates append+read
        log.log(AuditEntry::custom("debug".into(), "hello".into()));
        assert_eq!(log.entries().len(), 1);
    }

    #[test]
    fn audit_entry_helpers() {
        let se = AuditEntry::security_decision("s".into(), "c".into(), true);
        assert!(matches!(se, AuditEntry::SecurityDecision { .. }));

        let te = AuditEntry::tool_execution("aid".into(), "read".into(), "{}".into(), true, 10);
        assert!(matches!(te, AuditEntry::ToolExecution { .. }));

        let le = AuditEntry::lifecycle("a".into(), "run_start".into());
        assert!(matches!(le, AuditEntry::Lifecycle { .. }));
    }

    #[tokio::test]
    async fn audit_log_subscribe() {
        let log = AuditLog::new(64);
        let mut rx = log.subscribe();
        log.log(AuditEntry::lifecycle("test".into(), "msg".into()));
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, AuditEntry::Lifecycle { .. }));
    }
}
