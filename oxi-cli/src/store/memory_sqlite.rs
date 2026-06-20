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
pub struct SqliteMemoryStore {
    db: Mutex<Connection>,
}

impl SqliteMemoryStore {
    /// Open or create a SQLite memory store at `path`.
    ///
    /// Uses `:memory:` for an in-memory database. For persistent paths, WAL
    /// journal mode is enabled for better concurrent read performance.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let is_memory = path == Path::new(":memory:");
        let conn = Connection::open(path)?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;

        if !is_memory {
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
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
}
