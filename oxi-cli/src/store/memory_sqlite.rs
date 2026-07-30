//! SQLite-backed memory store implementing [`MemoryBackend`].
//!
//! Provides persistent storage with WAL journal mode for concurrent access
//! safety. Embedding storage is reserved for a future cosine-search upgrade;
//! search currently uses simple SQL `LIKE` matching.

use oxi_agent::tools::{MemoryBackend, MemoryItem, ToolError};
use rusqlite::{Connection, params};
use std::path::Path;
use std::pin::Pin;
use tokio::sync::Mutex;

/// SQLite-backed memory store.
///
/// Implements [`MemoryBackend`] for the `memory_*` agent tools.
/// Each memory is stored with an auto-generated UUID, kind, content, and subject.
/// The schema reserves an `embedding` column for future cosine-search support.
#[derive(Debug)]
pub struct SqliteMemoryStore {
    db: Mutex<Connection>,
}

impl SqliteMemoryStore {
    /// Open or create a SQLite memory store at `path`.
    ///
    /// Uses `:memory:` for an in-memory database. For persistent paths,
    /// filesystem-aware journal-mode selection is applied (see
    /// [`oxi_mnemopi::journal::JournalMode`]). On network filesystems the
    /// engine falls back to `TRUNCATE` mode + per-host DB sibling to avoid
    /// SIGBUS from mmap'd `-shm`.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let is_memory = path == Path::new(":memory:");
        // Durable primary store: detect journal mode (TRUNCATE on NFS for
        // SIGBUS safety) but NEVER rewrite the path — user memories must
        // stay coherent across hosts. See `JournalMode::effective_db_path`.
        let mode = if is_memory {
            oxi_mnemopi::journal::JournalMode::Wal
        } else {
            oxi_mnemopi::journal::JournalMode::for_db_path(path)
        };
        let conn = Connection::open(path)?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(&format!(
            "PRAGMA busy_timeout = {};",
            mode.busy_timeout_ms()
        ))?;

        if !is_memory {
            // Apply the detected (or env-overridden) journal mode. Failures
            // fall back to SQLite's default `delete` journal — still safe,
            // just slower than WAL.
            if let Err(e) = conn.execute_batch(&format!("PRAGMA journal_mode = {};", mode.as_str()))
            {
                tracing::warn!(
                    mode = mode.as_str(),
                    path = %path.display(),
                    error = %e,
                    "failed to set journal_mode; SQLite default will be used"
                );
            }
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id          TEXT PRIMARY KEY,
                subject     TEXT NOT NULL,
                kind        TEXT NOT NULL,
                content     TEXT NOT NULL,
                embedding   BLOB,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
                metadata    TEXT
            );",
        )?;

        Ok(Self {
            db: Mutex::new(conn),
        })
    }
}

impl MemoryBackend for SqliteMemoryStore {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let id = uuid::Uuid::new_v4().to_string();
            let db = self.db.lock().await;
            db.execute(
                "INSERT INTO memories (id, subject, kind, content)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, subject, kind, content],
            )
            .map_err(|e| format!("Failed to store memory: {e}"))?;
            Ok(id)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let db = self.db.lock().await;
            let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
            let mut stmt = db
                .prepare(
                    "SELECT id, kind, content, subject
                     FROM memories
                     WHERE content LIKE ?1 ESCAPE '\\'
                     ORDER BY length(content) ASC
                     LIMIT ?2",
                )
                .map_err(|e| format!("Failed to prepare search: {e}"))?;

            let results: Vec<MemoryItem> = stmt
                .query_map(params![pattern, k as i64], |row| {
                    Ok(MemoryItem {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        content: row.get(2)?,
                        subject: row.get(3)?,
                    })
                })
                .map_err(|e| format!("Failed to search memories: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(results)
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let db = self.db.lock().await;
            let mut stmt = db
                .prepare(
                    "SELECT id, kind, content, subject
                     FROM memories
                     WHERE subject = ?1
                     ORDER BY updated_at DESC",
                )
                .map_err(|e| format!("Failed to prepare list: {e}"))?;

            let results: Vec<MemoryItem> = stmt
                .query_map(params![subject], |row| {
                    Ok(MemoryItem {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        content: row.get(2)?,
                        subject: row.get(3)?,
                    })
                })
                .map_err(|e| format!("Failed to list memories: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(results)
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let db = self.db.lock().await;
            db.execute("DELETE FROM memories WHERE id = ?1", params![id])
                .map_err(|e| format!("Failed to delete memory: {e}"))?;
            Ok(())
        })
    }

    /// Bulk delete: a single SQL statement, no per-row round-trip.
    /// Returns the number of rows removed across all subjects.
    fn clear_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<usize, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let db = self.db.lock().await;
            let removed = db
                .execute("DELETE FROM memories", [])
                .map_err(|e| format!("Failed to clear memories: {e}"))?;
            Ok(removed)
        })
    }

    /// No background pipeline: surface an honest "not supported" rather
    /// than claiming to have enqueued work that did not happen.
    fn enqueue_consolidation<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            Err(
                "enqueue_consolidation not supported by SqliteMemoryStore (no pipeline)"
                    .to_string(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (SqliteMemoryStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SqliteMemoryStore::open(&dir.path().join("mem.db")).unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn put_list_delete_roundtrip() {
        let (store, _dir) = tmp_store();
        let id = store
            .put("user prefers dark mode", "preference", "alice")
            .await
            .unwrap();
        assert!(!id.is_empty());

        let items = store.list("alice").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "user prefers dark mode");
        assert_eq!(items[0].subject, "alice");
        assert_eq!(items[0].kind, "preference");

        store.delete(&id).await.unwrap();
        assert!(store.list("alice").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_scopes_per_subject() {
        let (store, _dir) = tmp_store();
        let a = store.put("a1", "fact", "alice").await.unwrap();
        let b = store.put("b1", "fact", "bob").await.unwrap();

        assert_eq!(store.list("alice").await.unwrap().len(), 1);
        assert_eq!(store.list("bob").await.unwrap().len(), 1);
        assert!(store.list("nobody").await.unwrap().is_empty());

        store.delete(&a).await.unwrap();
        store.delete(&b).await.unwrap();
    }

    #[tokio::test]
    async fn search_finds_substring_match() {
        let (store, _dir) = tmp_store();
        store
            .put("likes rust ownership", "fact", "alice")
            .await
            .unwrap();
        store.put("prefers python", "fact", "alice").await.unwrap();

        let results = store.search("rust", 5).await.unwrap();
        assert!(!results.is_empty(), "LIKE search must match substring");
        assert!(results.iter().any(|i| i.content.contains("rust")));
    }

    #[tokio::test]
    async fn memory_info_returns_none_for_sqlite_backend() {
        // Pins the contract — the SQLite backend has no bespoke
        // diagnostics; future accidental override should be caught here.
        let (store, _dir) = tmp_store();
        assert!(store.memory_info().is_none());
    }

    #[tokio::test]
    async fn clear_all_wipes_every_subject() {
        let (store, _dir) = tmp_store();
        store.put("a1", "fact", "alice").await.unwrap();
        store.put("a2", "fact", "alice").await.unwrap();
        store.put("b1", "fact", "bob").await.unwrap();

        let removed = store.clear_all().await.unwrap();
        assert_eq!(removed, 3, "all three rows removed in one DELETE");
        assert!(store.list("alice").await.unwrap().is_empty());
        assert!(store.list("bob").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn clear_all_is_idempotent_on_empty_store() {
        let (store, _dir) = tmp_store();
        let removed = store.clear_all().await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn enqueue_consolidation_surfaces_honest_unsupported() {
        let (store, _dir) = tmp_store();
        let err = store.enqueue_consolidation().await.unwrap_err();
        assert!(
            err.contains("not supported"),
            "SqliteMemoryStore must surface the absence of a pipeline, got: {err}"
        );
    }
}
