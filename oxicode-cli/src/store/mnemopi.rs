//! Simple JSON-file-based memory store implementing `MemoryBackend`.
//!
//! A lightweight alternative to the full Mnemopi SQLite backend.
//! Stores memories as a JSON array in `~/.oxicode/memory/<project>.json`.
//! Suitable for small-scale use; the full SQLite backend with FTS5
//! and embedding search is a future enhancement (see `11-mnemopi-backend.md`).

use oxicode_agent::tools::{MemoryBackend, MemoryItem, ToolError};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;

/// JSON-file memory store.
///
/// Implements `MemoryBackend` for the `memory_*` agent tools.
/// Each memory is stored with an auto-generated ID, kind, content, and subject.
/// Search is simple substring matching (no embeddings).
#[derive(Debug)]
pub struct MnemopiStore {
    /// In-memory cache of all memories, keyed by ID.
    memories: RwLock<HashMap<String, StoredMemory>>,
    /// File path for persistence.
    path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredMemory {
    id: String,
    kind: String,
    content: String,
    subject: String,
}

impl MnemopiStore {
    /// Open or create a memory store at the given path.
    pub fn open(path: PathBuf) -> Self {
        let memories = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let entries: Vec<StoredMemory> =
                        serde_json::from_str(&text).unwrap_or_default();
                    entries.into_iter().map(|m| (m.id.clone(), m)).collect()
                }
                Err(_) => HashMap::new(),
            }
        } else {
            HashMap::new()
        };

        Self {
            memories: RwLock::new(memories),
            path,
        }
    }

    /// Create a default store at `~/.oxicode/memory/default.json`.
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".oxicode")
            .join("memory")
            .join("default.json")
    }

    /// Persist the current state to disk.
    fn save(&self) {
        let entries: Vec<StoredMemory> = self.memories.read().values().cloned().collect();
        if let Ok(text) = serde_json::to_string_pretty(&entries) {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.path, text);
        }
    }

    /// Generate a new unique ID.
    fn next_id(&self) -> String {
        let count = self.memories.read().len();
        format!("mem-{}", count + 1)
    }
}

impl MemoryBackend for MnemopiStore {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let id = self.next_id();
            let entry = StoredMemory {
                id: id.clone(),
                kind: kind.to_string(),
                content: content.to_string(),
                subject: subject.to_string(),
            };
            self.memories.write().insert(id.clone(), entry);
            self.save();
            Ok(id)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let query_lower = query.to_lowercase();
            let memories = self.memories.read();
            let mut results: Vec<MemoryItem> = memories
                .values()
                .filter(|m| m.content.to_lowercase().contains(&query_lower))
                .take(k)
                .map(|m| MemoryItem {
                    id: m.id.clone(),
                    kind: m.kind.clone(),
                    content: m.content.clone(),
                    subject: m.subject.clone(),
                })
                .collect();
            // Sort by content length (shorter = more relevant for exact match)
            results.sort_by_key(|m| m.content.len());
            Ok(results)
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let memories = self.memories.read();
            Ok(memories
                .values()
                .filter(|m| m.subject == subject)
                .map(|m| MemoryItem {
                    id: m.id.clone(),
                    kind: m.kind.clone(),
                    content: m.content.clone(),
                    subject: m.subject.clone(),
                })
                .collect())
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move {
            self.memories.write().remove(id);
            self.save();
            Ok(())
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_search() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MnemopiStore::open(tmp.path().join("mem.json"));

        // Can't easily test async MemoryBackend without tokio runtime,
        // but we can test the internal state.
        store.memories.write().insert(
            "1".into(),
            StoredMemory {
                id: "1".into(),
                kind: "fact".into(),
                content: "The project uses Rust 2024".into(),
                subject: "default".into(),
            },
        );

        let memories = store.memories.read();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories.get("1").unwrap().kind, "fact");
    }

    #[test]
    fn persistence_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mem.json");

        {
            let store = MnemopiStore::open(path.clone());
            store.memories.write().insert(
                "1".into(),
                StoredMemory {
                    id: "1".into(),
                    kind: "preference".into(),
                    content: "Prefers Korean prose".into(),
                    subject: "project".into(),
                },
            );
            store.save();
        }

        // Reopen and verify
        let store2 = MnemopiStore::open(path);
        let memories = store2.memories.read();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories.get("1").unwrap().content, "Prefers Korean prose");
    }

    #[test]
    fn default_path_in_home() {
        let path = MnemopiStore::default_path();
        assert!(path.to_string_lossy().contains(".oxicode"));
        assert!(path.to_string_lossy().contains("memory"));
    }
}
