//! Tamper-evident audit trail with cryptographic hash chain (blake3).
//!
//! Each entry is cryptographically linked to the previous entry,
//! making tampering detectable. Provides rich querying, JSON export,
//! and persistence via the `AuditPersistence` trait.
//!
//! Migrated from oxios-kernel — this is the canonical implementation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Type alias for hash digest (blake3 hex output, 64 chars).
pub type HashDigest = String;

/// Unique identifier for an agent (String for flexibility).
pub type AgentId = String;

// ─── Error Types ─────────────────────────────────────────────────────────────

/// Errors that can occur during audit trail operations.
#[derive(Debug, Clone)]
pub enum AuditError {
    /// Chain link broken at given sequence number.
    ChainBroken {
        seq: u64,
        expected: String,
        found: String,
    },
    /// Invalid timestamp detected.
    InvalidTimestamp { seq: u64 },
    /// Failed to export audit log.
    ExportFailed(String),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::ChainBroken {
                seq,
                expected,
                found,
            } => {
                write!(
                    f,
                    "chain broken at seq {seq}: expected hash '{expected}', found '{found}'"
                )
            }
            AuditError::InvalidTimestamp { seq } => {
                write!(f, "invalid timestamp at seq {seq}")
            }
            AuditError::ExportFailed(msg) => {
                write!(f, "export failed: {msg}")
            }
        }
    }
}

impl std::error::Error for AuditError {}

// ─── Audit Action ────────────────────────────────────────────────────────────

/// Types of actions that can be audited.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
pub enum AuditAction {
    /// Agent spawned with task type.
    AgentSpawn { task_type: String },
    /// Agent exited with reason.
    AgentExit { reason: String },
    /// Tool was called.
    ToolCall { tool: String, args_json: String },
    /// Tool returned a result.
    ToolResult { tool: String, success: bool },
    /// Memory entry written.
    MemoryWrite { entry_id: String },
    /// Memory entry read.
    MemoryRead { entry_id: String },
    /// Configuration changed.
    ConfigChange { key: String },
    /// Program installed.
    ProgramInstall { program: String, version: String },
    /// Cron job triggered.
    CronTrigger { job_id: String },
    /// Git commit created.
    GitCommit { message: String },
    /// Access was denied.
    AccessDenied { permission: String },
    /// Other/unclassified action.
    Other { detail: String },
}

// ─── Audit Entry ─────────────────────────────────────────────────────────────

/// A single entry in the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailEntry {
    /// Sequential entry number.
    pub seq: u64,
    /// Timestamp of the entry.
    pub timestamp: DateTime<Utc>,
    /// Agent ID that performed the action.
    pub actor: AgentId,
    /// The action that was performed.
    pub action: AuditAction,
    /// Resource affected by the action.
    pub resource: String,
    /// Hash of the previous entry ("genesis" for first, "pruned" after auto-pruning).
    pub prev_hash: HashDigest,
    /// Hash of this entry.
    pub hash: HashDigest,
    /// Optional arbitrary metadata.
    pub metadata: Option<serde_json::Value>,
}

// ─── Persistence Trait ───────────────────────────────────────────────────────

/// Trait for persisting audit trail entries.
///
/// Implement this to integrate with your storage backend
/// (filesystem, database, object store, etc.).
pub trait AuditPersistence: Send + Sync {
    /// Save entries to persistent storage.
    fn save(&self, entries: &[TrailEntry]) -> anyhow::Result<()>;
    /// Load entries from persistent storage.
    fn load(&self) -> anyhow::Result<Vec<TrailEntry>>;
}

// ─── Hash Computation ────────────────────────────────────────────────────────

/// Compute the hash for an audit entry using blake3.
fn compute_entry_hash(
    seq: u64,
    ts: &DateTime<Utc>,
    actor: &str,
    action: &AuditAction,
    resource: &str,
    prev: &str,
) -> HashDigest {
    let mut h = blake3::Hasher::new();
    h.update(b"oxios-audit-v1");
    h.update(&seq.to_be_bytes());
    h.update(ts.to_rfc3339().as_bytes());
    h.update(actor.as_bytes());
    let action_bytes = serde_json::to_vec(action).unwrap_or_default();
    h.update(&action_bytes);
    h.update(prev.as_bytes());
    h.update(resource.as_bytes());
    h.finalize().to_hex().to_string()
}

// ─── Audit Trail ─────────────────────────────────────────────────────────────

/// A tamper-evident audit trail with cryptographic hash chain.
///
/// Each entry is cryptographically linked to the previous entry using
/// blake3 hashing. This makes it possible to detect any tampering with
/// historical entries.
pub struct AuditTrail {
    entries: parking_lot::RwLock<Vec<TrailEntry>>,
    seq_counter: AtomicU64,
    #[allow(dead_code)]
    chain_hasher: parking_lot::Mutex<blake3::Hasher>,
    max_entries: usize,
}

impl AuditTrail {
    /// Create a new audit trail with the given maximum entry count.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: parking_lot::RwLock::new(Vec::new()),
            seq_counter: AtomicU64::new(1),
            chain_hasher: parking_lot::Mutex::new(blake3::Hasher::new()),
            max_entries,
        }
    }

    /// Get the current number of entries.
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Check if the trail is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the last hash in the chain.
    fn last_hash(&self) -> HashDigest {
        let entries = self.entries.read();
        entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| "genesis".to_string())
    }

    /// Append an audit entry. Computes hash chain automatically.
    pub fn append(&self, actor: AgentId, action: AuditAction, resource: String) -> HashDigest {
        self.append_with_meta(actor, action, resource, None)
    }

    /// Append an audit entry with optional metadata.
    pub fn append_with_meta(
        &self,
        actor: AgentId,
        action: AuditAction,
        resource: String,
        metadata: Option<serde_json::Value>,
    ) -> HashDigest {
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);
        let timestamp = Utc::now();
        let prev_hash = self.last_hash();
        let hash = compute_entry_hash(seq, &timestamp, &actor, &action, &resource, &prev_hash);

        let entry = TrailEntry {
            seq,
            timestamp,
            actor,
            action,
            resource,
            prev_hash,
            hash,
            metadata,
        };

        let entry_hash = entry.hash.clone();

        {
            let mut entries = self.entries.write();
            entries.push(entry);
            if entries.len() > self.max_entries {
                let excess = entries.len() - self.max_entries;
                entries.drain(0..excess);
                if let Some(first) = entries.first_mut() {
                    first.prev_hash = "pruned".to_string();
                }
            }
        }

        entry_hash
    }

    /// Verify the integrity of the hash chain.
    pub fn verify(&self) -> Result<bool, AuditError> {
        let entries = self.entries.read();
        let mut prev_hash = "genesis".to_string();

        for (i, entry) in entries.iter().enumerate() {
            if entry.seq == 0 {
                return Err(AuditError::ChainBroken {
                    seq: 0,
                    expected: "non-zero sequence".to_string(),
                    found: "0".to_string(),
                });
            }

            if i == 0 && entry.prev_hash == "pruned" {
                prev_hash = entry.hash.clone();
                continue;
            } else if entry.prev_hash != prev_hash {
                return Err(AuditError::ChainBroken {
                    seq: entry.seq,
                    expected: prev_hash,
                    found: entry.prev_hash.clone(),
                });
            }

            let now = Utc::now();
            if entry.timestamp > now {
                return Err(AuditError::InvalidTimestamp { seq: entry.seq });
            }

            let computed = compute_entry_hash(
                entry.seq,
                &entry.timestamp,
                &entry.actor,
                &entry.action,
                &entry.resource,
                &entry.prev_hash,
            );

            if computed != entry.hash {
                return Err(AuditError::ChainBroken {
                    seq: entry.seq,
                    expected: computed,
                    found: entry.hash.clone(),
                });
            }

            prev_hash = entry.hash.clone();
        }

        Ok(true)
    }

    /// Get entries within a sequence range (inclusive).
    pub fn entries(&self, from_seq: u64, to_seq: u64) -> Vec<TrailEntry> {
        let entries = self.entries.read();
        entries
            .iter()
            .filter(|e| e.seq >= from_seq && e.seq <= to_seq)
            .cloned()
            .collect()
    }

    /// Get all entries.
    pub fn all_entries(&self) -> Vec<TrailEntry> {
        self.entries.read().clone()
    }

    /// Query entries by agent ID.
    pub fn by_agent(&self, agent_id: &str) -> Vec<TrailEntry> {
        let entries = self.entries.read();
        entries
            .iter()
            .filter(|e| e.actor == agent_id)
            .cloned()
            .collect()
    }

    /// Query entries by exact action match.
    pub fn by_action(&self, action: &AuditAction) -> Vec<TrailEntry> {
        let entries = self.entries.read();
        entries
            .iter()
            .filter(|e| &e.action == action)
            .cloned()
            .collect()
    }

    /// Query entries by action discriminant name (e.g., "ToolCall", "AgentSpawn").
    pub fn by_action_type(&self, type_name: &str) -> Vec<TrailEntry> {
        let entries = self.entries.read();
        entries
            .iter()
            .filter(|e| {
                let action_name = match &e.action {
                    AuditAction::AgentSpawn { .. } => "AgentSpawn",
                    AuditAction::AgentExit { .. } => "AgentExit",
                    AuditAction::ToolCall { .. } => "ToolCall",
                    AuditAction::ToolResult { .. } => "ToolResult",
                    AuditAction::MemoryWrite { .. } => "MemoryWrite",
                    AuditAction::MemoryRead { .. } => "MemoryRead",
                    AuditAction::ConfigChange { .. } => "ConfigChange",
                    AuditAction::ProgramInstall { .. } => "ProgramInstall",
                    AuditAction::CronTrigger { .. } => "CronTrigger",
                    AuditAction::GitCommit { .. } => "GitCommit",
                    AuditAction::AccessDenied { .. } => "AccessDenied",
                    AuditAction::Other { .. } => "Other",
                };
                action_name == type_name
            })
            .cloned()
            .collect()
    }

    /// Export entries from a sequence number as pretty JSON.
    pub fn export_json(&self, from_seq: u64) -> Result<String, AuditError> {
        let entries = self.entries.read();
        let filtered: Vec<&TrailEntry> = entries.iter().filter(|e| e.seq >= from_seq).collect();
        serde_json::to_string_pretty(&filtered).map_err(|e| AuditError::ExportFailed(e.to_string()))
    }

    /// Export all entries as pretty JSON.
    pub fn export_all_json(&self) -> Result<String, AuditError> {
        let entries = self.entries.read();
        serde_json::to_string_pretty(&*entries).map_err(|e| AuditError::ExportFailed(e.to_string()))
    }

    /// Flush entries to a persistence backend.
    pub fn flush_to(&self, store: &dyn AuditPersistence) -> anyhow::Result<()> {
        let entries = self.all_entries();
        store.save(&entries)
    }

    /// Restore entries from a persistence backend.
    pub fn restore_from_store(&self, store: &dyn AuditPersistence) -> anyhow::Result<()> {
        let entries = store.load()?;
        self.restore_from(entries);
        Ok(())
    }

    /// Restore previously persisted entries directly.
    ///
    /// Sets `seq_counter` to `max(entries.seq) + 1` so new entries
    /// don't collide with restored ones. Trims to `max_entries` if needed.
    pub fn restore_from(&self, entries: Vec<TrailEntry>) {
        if entries.is_empty() {
            return;
        }

        let max_seq = entries.iter().map(|e| e.seq).max().unwrap_or(0);
        self.seq_counter.store(max_seq + 1, Ordering::SeqCst);

        let mut current = self.entries.write();
        *current = entries;

        if current.len() > self.max_entries {
            let excess = current.len() - self.max_entries;
            current.drain(0..excess);
            if let Some(first) = current.first_mut() {
                first.prev_hash = "pruned".to_string();
            }
        }

        tracing::info!(
            restored = current.len(),
            next_seq = max_seq + 1,
            "Audit trail restored from persistence"
        );
    }
}

impl Default for AuditTrail {
    fn default() -> Self {
        Self::new(100_000)
    }
}

impl std::fmt::Debug for AuditTrail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditTrail")
            .field("entries", &self.len())
            .field("seq_counter", &self.seq_counter)
            .field("max_entries", &self.max_entries)
            .finish()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_trail() -> AuditTrail {
        AuditTrail::new(1000)
    }

    #[test]
    fn test_append_generates_hash() {
        let trail = create_test_trail();
        let hash = trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_append_increments_seq() {
        let trail = create_test_trail();
        let h1 = trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        let h2 = trail.append(
            "agent-002".into(),
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            "/test/resource2".into(),
        );
        assert_ne!(h1, h2);
        let entries = trail.all_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
    }

    #[test]
    fn test_hash_chain_linked() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::AgentExit {
                reason: "done".into(),
            },
            "/test/resource".into(),
        );
        let entries = trail.all_entries();
        assert_eq!(entries[0].prev_hash, "genesis");
        assert_eq!(entries[1].prev_hash, entries[0].hash);
    }

    #[test]
    fn test_verify_passes_clean_chain() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::ToolResult {
                tool: "bash".into(),
                success: true,
            },
            "/test/resource".into(),
        );
        assert!(trail.verify().is_ok());
    }

    #[test]
    fn test_verify_detects_tampering() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            "/test/resource".into(),
        );
        {
            let mut entries = trail.entries.write();
            entries[0].actor = "hacker-001".into();
        }
        let result = trail.verify();
        assert!(result.is_err());
        match result {
            Err(AuditError::ChainBroken { seq, .. }) => {
                assert_eq!(seq, 1);
            }
            _ => panic!("expected ChainBroken error"),
        }
    }

    #[test]
    fn test_verify_detects_prev_hash_tampering() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            "/test/resource".into(),
        );
        {
            let mut entries = trail.entries.write();
            entries[1].prev_hash = "fake-hash".into();
        }
        assert!(trail.verify().is_err());
    }

    #[test]
    fn test_export_json_format() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        let json = trail.export_json(0).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].get("seq").is_some());
        assert!(parsed[0].get("hash").is_some());
    }

    #[test]
    fn test_by_agent_query() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-002".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::AgentExit {
                reason: "done".into(),
            },
            "/test/resource".into(),
        );
        assert_eq!(trail.by_agent("agent-001").len(), 2);
        assert_eq!(trail.by_agent("agent-002").len(), 1);
    }

    #[test]
    fn test_by_action_query() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            "/test/resource".into(),
        );
        trail.append(
            "agent-001".into(),
            AuditAction::ToolCall {
                tool: "grep".into(),
                args_json: "{}".into(),
            },
            "/test/resource".into(),
        );
        assert_eq!(
            trail
                .by_action(&AuditAction::AgentSpawn {
                    task_type: "test".into()
                })
                .len(),
            1
        );
        assert_eq!(trail.by_action_type("ToolCall").len(), 2);
    }

    #[test]
    fn test_entries_range() {
        let trail = create_test_trail();
        for i in 0..10 {
            trail.append(
                "agent-001".into(),
                AuditAction::Other {
                    detail: format!("action-{i}"),
                },
                "/test/resource".into(),
            );
        }
        let range = trail.entries(3, 7);
        assert_eq!(range.len(), 5);
        assert_eq!(range[0].seq, 3);
        assert_eq!(range[4].seq, 7);
    }

    #[test]
    fn test_auto_prune() {
        let trail = AuditTrail::new(5);
        for i in 0..10 {
            trail.append(
                "agent-001".into(),
                AuditAction::Other {
                    detail: format!("action-{i}"),
                },
                "/test/resource".into(),
            );
        }
        assert_eq!(trail.len(), 5);
        let entries = trail.all_entries();
        assert_eq!(entries[0].seq, 6);
        assert_eq!(entries[4].seq, 10);
        assert!(trail.verify().is_ok(), "Pruned trail should still verify");
    }

    #[test]
    fn test_append_with_metadata() {
        let trail = create_test_trail();
        let metadata = serde_json::json!({"duration_ms": 150, "memory_mb": 32});
        trail.append_with_meta(
            "agent-001".into(),
            AuditAction::MemoryWrite {
                entry_id: "mem-001".into(),
            },
            "/memory/entries".into(),
            Some(metadata.clone()),
        );
        let entries = trail.all_entries();
        assert_eq!(entries[0].metadata.as_ref().unwrap(), &metadata);
    }

    #[test]
    fn test_genesis_hash() {
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/test/resource".into(),
        );
        assert_eq!(trail.all_entries()[0].prev_hash, "genesis");
    }

    #[test]
    fn test_deterministic_hash() {
        let trail = create_test_trail();
        let action = AuditAction::AgentSpawn {
            task_type: "test".into(),
        };
        trail.append("agent-001".into(), action.clone(), "/test/resource".into());
        let hash = compute_entry_hash(
            1,
            &trail.all_entries()[0].timestamp,
            "agent-001",
            &action,
            "/test/resource",
            "genesis",
        );
        assert_eq!(hash, trail.all_entries()[0].hash);
    }

    #[test]
    fn test_empty_trail_verify() {
        assert!(create_test_trail().verify().is_ok());
    }

    #[test]
    fn test_all_action_types() {
        let trail = create_test_trail();
        let actions: Vec<AuditAction> = vec![
            AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            AuditAction::AgentExit {
                reason: "done".into(),
            },
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            AuditAction::ToolResult {
                tool: "bash".into(),
                success: true,
            },
            AuditAction::MemoryWrite {
                entry_id: "mem-001".into(),
            },
            AuditAction::MemoryRead {
                entry_id: "mem-001".into(),
            },
            AuditAction::ConfigChange {
                key: "max_agents".into(),
            },
            AuditAction::ProgramInstall {
                program: "test-program".into(),
                version: "1.0.0".into(),
            },
            AuditAction::CronTrigger {
                job_id: "job-001".into(),
            },
            AuditAction::GitCommit {
                message: "test commit".into(),
            },
            AuditAction::AccessDenied {
                permission: "write".into(),
            },
            AuditAction::Other {
                detail: "misc".into(),
            },
        ];
        for (i, action) in actions.into_iter().enumerate() {
            trail.append("agent-001".into(), action, format!("/resource/{i}"));
        }
        assert_eq!(trail.len(), 12);
        assert!(trail.verify().is_ok());
    }

    #[test]
    fn test_hash_different_for_different_inputs() {
        let ts = Utc::now();
        let h1 = compute_entry_hash(
            1,
            &ts,
            "agent-001",
            &AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/resource",
            "genesis",
        );
        let h2 = compute_entry_hash(
            2,
            &ts,
            "agent-001",
            &AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/resource",
            "genesis",
        );
        assert_ne!(h1, h2);
        let h3 = compute_entry_hash(
            1,
            &ts,
            "agent-002",
            &AuditAction::AgentSpawn {
                task_type: "test".into(),
            },
            "/resource",
            "genesis",
        );
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_restore_from_empty() {
        let trail = create_test_trail();
        trail.restore_from(Vec::new());
        assert!(trail.is_empty());
    }

    #[test]
    fn test_restore_from_advances_seq_counter() {
        let trail = create_test_trail();
        let ts = Utc::now();
        let mut entries = Vec::new();
        let mut prev = "genesis".to_string();
        for i in 1..=5u64 {
            let hash = compute_entry_hash(
                i,
                &ts,
                "agent-001",
                &AuditAction::Other {
                    detail: format!("action-{i}"),
                },
                "/resource",
                &prev,
            );
            entries.push(TrailEntry {
                seq: i,
                timestamp: ts,
                actor: "agent-001".into(),
                action: AuditAction::Other {
                    detail: format!("action-{i}"),
                },
                resource: "/resource".into(),
                prev_hash: prev.clone(),
                hash: hash.clone(),
                metadata: None,
            });
            prev = hash;
        }
        trail.restore_from(entries);
        assert_eq!(trail.len(), 5);
        let new_hash = trail.append(
            "agent-001".into(),
            AuditAction::Other {
                detail: "new".into(),
            },
            "/resource".into(),
        );
        assert!(!new_hash.is_empty());
        assert_eq!(trail.len(), 6);
        assert_eq!(trail.all_entries()[5].seq, 6);
    }

    #[test]
    fn test_restore_from_trims_to_max() {
        let trail = AuditTrail::new(3);
        let ts = Utc::now();
        let mut entries = Vec::new();
        let mut prev = "genesis".to_string();
        for i in 1..=5u64 {
            let hash = compute_entry_hash(
                i,
                &ts,
                "agent-001",
                &AuditAction::Other {
                    detail: format!("action-{i}"),
                },
                "/resource",
                &prev,
            );
            entries.push(TrailEntry {
                seq: i,
                timestamp: ts,
                actor: "agent-001".into(),
                action: AuditAction::Other {
                    detail: format!("action-{i}"),
                },
                resource: "/resource".into(),
                prev_hash: prev.clone(),
                hash: hash.clone(),
                metadata: None,
            });
            prev = hash;
        }
        trail.restore_from(entries);
        assert_eq!(trail.len(), 3);
        let all = trail.all_entries();
        assert_eq!(all[0].seq, 3);
        assert_eq!(all[2].seq, 5);
        assert!(trail.verify().is_ok());
    }

    #[test]
    fn test_persistence_roundtrip() {
        use std::sync::Mutex;

        struct MemStore {
            data: Mutex<Vec<TrailEntry>>,
        }
        impl AuditPersistence for MemStore {
            fn save(&self, entries: &[TrailEntry]) -> anyhow::Result<()> {
                *self.data.lock().unwrap() = entries.to_vec();
                Ok(())
            }
            fn load(&self) -> anyhow::Result<Vec<TrailEntry>> {
                Ok(self.data.lock().unwrap().clone())
            }
        }

        let store = MemStore {
            data: Mutex::new(Vec::new()),
        };
        let trail = create_test_trail();
        trail.append(
            "agent-001".into(),
            AuditAction::ToolCall {
                tool: "bash".into(),
                args_json: "{}".into(),
            },
            "/test".into(),
        );
        trail.flush_to(&store).unwrap();

        let trail2 = create_test_trail();
        trail2.restore_from_store(&store).unwrap();
        assert_eq!(trail2.len(), 1);
        assert_eq!(trail2.all_entries()[0].actor, "agent-001");
    }
}
