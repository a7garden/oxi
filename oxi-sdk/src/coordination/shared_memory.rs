//! Shared memory — versioned KV store with optimistic locking.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::broadcast;

/// Namespaced key for shared memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryKey {
    /// Logical grouping for the key.
    pub namespace: String,
    /// Key name within the namespace.
    pub key: String,
}

impl MemoryKey {
    /// Create a new key.
    pub fn new(namespace: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            key: key.into(),
        }
    }
}

/// A value entry with version and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Stored JSON value.
    pub value: serde_json::Value,
    /// Monotonically increasing version for optimistic locking.
    pub version: u64,
    /// Wall-clock time of last modification, in ms since Unix epoch.
    pub modified_at_ms: u64,
    /// Identifier of the agent that last wrote the value.
    pub modified_by: String,
}

/// Events emitted by shared memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryEvent {
    /// A value was written or updated.
    Written {
        /// Namespaced key that was written.
        key: MemoryKey,
        /// New version number of the entry.
        version: u64,
        /// Identifier of the writing agent.
        author: String,
    },
    /// A value was removed.
    Deleted {
        /// Namespaced key that was deleted.
        key: MemoryKey,
    },
}

/// In-memory shared KV store with optimistic locking.
///
/// Thread-safe via `parking_lot::RwLock`. Supports atomic increment
/// for counters and version-based conflict detection.
pub struct SharedMemory {
    data: RwLock<HashMap<MemoryKey, MemoryEntry>>,
    tx: broadcast::Sender<MemoryEvent>,
}

impl SharedMemory {
    /// Create a new shared memory store.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            data: RwLock::new(HashMap::new()),
            tx,
        }
    }

    /// Read a value by key.
    pub fn read(&self, key: &MemoryKey) -> Option<serde_json::Value> {
        self.data.read().get(key).map(|e| e.value.clone())
    }

    /// Read a full entry (including version and metadata).
    pub fn read_entry(&self, key: &MemoryKey) -> Option<MemoryEntry> {
        self.data.read().get(key).cloned()
    }

    /// Write a value with optional optimistic locking.
    ///
    /// If `expected_version` is `Some(v)`, the write fails if the current
    /// version does not match. Returns the new version on success.
    pub fn write(
        &self,
        key: &MemoryKey,
        value: serde_json::Value,
        author: &str,
        expected_version: Option<u64>,
    ) -> Result<u64, crate::error::SdkError> {
        let mut data = self.data.write();

        if let Some(expected) = expected_version {
            if let Some(entry) = data.get(key) {
                if entry.version != expected {
                    return Err(crate::error::SdkError::VersionConflict {
                        key: format!("{}:{}", key.namespace, key.key),
                        expected,
                        current: entry.version,
                    });
                }
            } else if expected != 0 {
                return Err(crate::error::SdkError::VersionConflict {
                    key: format!("{}:{}", key.namespace, key.key),
                    expected,
                    current: 0,
                });
            }
        }

        let current_version = data.get(key).map(|e| e.version).unwrap_or(0);
        let new_version = current_version + 1;

        data.insert(
            key.clone(),
            MemoryEntry {
                value,
                version: new_version,
                modified_at_ms: now_ms(),
                modified_by: author.to_string(),
            },
        );

        let _ = self.tx.send(MemoryEvent::Written {
            key: key.clone(),
            version: new_version,
            author: author.to_string(),
        });

        Ok(new_version)
    }

    /// Atomic increment for counter values. Returns the new value.
    pub fn increment(&self, key: &MemoryKey, delta: i64, author: &str) -> i64 {
        let mut data = self.data.write();
        let entry = data.entry(key.clone()).or_insert(MemoryEntry {
            value: serde_json::json!(0),
            version: 0,
            modified_at_ms: 0,
            modified_by: String::new(),
        });

        let current = entry.value.as_i64().unwrap_or(0);
        let new_val = current + delta;
        entry.value = serde_json::json!(new_val);
        entry.version += 1;
        entry.modified_at_ms = now_ms();
        entry.modified_by = author.to_string();
        new_val
    }

    /// Delete a key. Returns true if the key existed.
    pub fn delete(&self, key: &MemoryKey) -> bool {
        let removed = self.data.write().remove(key).is_some();
        if removed {
            let _ = self.tx.send(MemoryEvent::Deleted { key: key.clone() });
        }
        removed
    }

    /// List all keys in a namespace.
    pub fn list_namespace(&self, namespace: &str) -> Vec<MemoryKey> {
        self.data
            .read()
            .keys()
            .filter(|k| k.namespace == namespace)
            .cloned()
            .collect()
    }

    /// Subscribe to memory events.
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryEvent> {
        self.tx.subscribe()
    }
}

impl Default for SharedMemory {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read() {
        let mem = SharedMemory::new();
        let key = MemoryKey::new("ns", "counter");
        mem.write(&key, serde_json::json!(42), "agent-1", None)
            .unwrap();
        assert_eq!(mem.read(&key), Some(serde_json::json!(42)));
    }

    #[test]
    fn version_increments() {
        let mem = SharedMemory::new();
        let key = MemoryKey::new("ns", "val");
        let v1 = mem.write(&key, serde_json::json!("a"), "a1", None).unwrap();
        let v2 = mem.write(&key, serde_json::json!("b"), "a2", None).unwrap();
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        let entry = mem.read_entry(&key).unwrap();
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn optimistic_lock_success() {
        let mem = SharedMemory::new();
        let key = MemoryKey::new("ns", "val");
        let v1 = mem.write(&key, serde_json::json!("a"), "a1", None).unwrap();
        let v2 = mem
            .write(&key, serde_json::json!("b"), "a2", Some(v1))
            .unwrap();
        assert_eq!(v2, 2);
    }

    #[test]
    fn optimistic_lock_conflict() {
        let mem = SharedMemory::new();
        let key = MemoryKey::new("ns", "val");
        let _v1 = mem.write(&key, serde_json::json!("a"), "a1", None).unwrap();
        let result = mem.write(&key, serde_json::json!("b"), "a2", Some(99));
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::SdkError::VersionConflict {
                expected, current, ..
            } => {
                assert_eq!(expected, 99);
                assert_eq!(current, 1);
            }
            _ => panic!("Expected VersionConflict"),
        }
    }

    #[test]
    fn atomic_increment() {
        let mem = SharedMemory::new();
        let key = MemoryKey::new("ns", "counter");
        assert_eq!(mem.increment(&key, 5, "a1"), 5);
        assert_eq!(mem.increment(&key, 3, "a2"), 8);
        assert_eq!(mem.read(&key), Some(serde_json::json!(8)));
    }

    #[test]
    fn delete_key() {
        let mem = SharedMemory::new();
        let key = MemoryKey::new("ns", "val");
        mem.write(&key, serde_json::json!(1), "a1", None).unwrap();
        assert!(mem.delete(&key));
        assert!(mem.read(&key).is_none());
        assert!(!mem.delete(&key)); // already deleted
    }

    #[test]
    fn list_namespace() {
        let mem = SharedMemory::new();
        mem.write(
            &MemoryKey::new("reviews", "a"),
            serde_json::json!(1),
            "a1",
            None,
        )
        .unwrap();
        mem.write(
            &MemoryKey::new("reviews", "b"),
            serde_json::json!(2),
            "a1",
            None,
        )
        .unwrap();
        mem.write(
            &MemoryKey::new("other", "c"),
            serde_json::json!(3),
            "a1",
            None,
        )
        .unwrap();
        let keys = mem.list_namespace("reviews");
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn subscribe_events() {
        let mem = SharedMemory::new();
        let mut rx = mem.subscribe();
        let key = MemoryKey::new("ns", "val");
        mem.write(&key, serde_json::json!(1), "a1", None).unwrap();
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, MemoryEvent::Written { .. }));
    }
}
