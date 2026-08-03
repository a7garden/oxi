//! SQLite handle — ported from omp `db.ts`.
//!
//! Wraps a `rusqlite::Connection` behind a `tokio::sync::Mutex` for async-safe
//! access. All SQLite operations are inherently blocking; callers should use
//! `spawn_blocking` for heavy queries in async contexts.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::Result;
use crate::schema;

/// SQLite database handle with async-safe locking.
///
/// Mirrors omp's `openDatabase` + PRAGMA setup. The connection is guarded by
/// `tokio::sync::Mutex` so it can be safely shared across async tasks.
pub struct MnemopiDb {
    conn: tokio::sync::Mutex<Connection>,
    /// Original path (None = in-memory).
    pub db_path: Option<PathBuf>,
}

impl MnemopiDb {
    /// Open or create a database at `path`.
    ///
    /// Selects journal mode based on filesystem detection:
    /// - Local filesystem → `WAL` mode (default).
    /// - Network filesystem (NFS/SMB/CIFS/FUSE) → `TRUNCATE` mode + per-host
    ///   DB path to avoid SIGBUS from mmap'd `-shm` on incoherent shared
    ///   memory (see [`crate::journal::JournalMode`]).
    ///
    /// `OXICODE_SQLITE_JOURNAL_MODE=wal|truncate` overrides detection.
    ///
    /// Then runs `init_schema()` to ensure all tables and triggers exist.
    pub fn open(path: &Path) -> Result<Self> {
        let mode = crate::journal::JournalMode::for_db_path(path);
        // Durable primary store: detect journal mode (TRUNCATE on NFS for
        // SIGBUS safety) but NEVER rewrite the path — user memories must
        // stay coherent across hosts. See
        // [`crate::journal::JournalMode::effective_db_path`].
        let conn = Connection::open(path)?;
        Self::init_connection(&conn, Some(path), mode)?;
        let db = Self {
            conn: tokio::sync::Mutex::new(conn),
            db_path: Some(path.to_path_buf()),
        };
        Ok(db)
    }

    /// Create an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        // In-memory DBs have no filesystem; journal mode is irrelevant.
        Self::init_connection(&conn, None, crate::journal::JournalMode::Wal)?;
        Ok(Self {
            conn: tokio::sync::Mutex::new(conn),
            db_path: None,
        })
    }

    fn init_connection(
        conn: &Connection,
        path: Option<&Path>,
        mode: crate::journal::JournalMode,
    ) -> Result<()> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", mode.busy_timeout_ms())?;
        if path.is_some() {
            // Apply the filesystem-aware journal mode. SQLite returns the
            // *actual* mode (which may differ from the request on
            // in-memory or network DBs); we log but do not error on
            // mismatch — the worst case is falling back to `delete` which
            // is still safe, just slower than WAL.
            match conn.pragma_update(None, "journal_mode", mode.as_str()) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(
                        mode = mode.as_str(),
                        path = ?path.map(|p| p.display().to_string()),
                        error = %e,
                        "failed to set journal_mode; SQLite will use its default"
                    );
                }
            }
        }
        schema::init_schema(conn)?;
        Ok(())
    }

    /// Acquire the connection lock.
    ///
    /// **Important**: Do not hold the `MutexGuard` across an `.await` point.
    /// Run all SQLite operations synchronously within the guard scope, or use
    /// `spawn_blocking` from async code.
    pub fn lock(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.blocking_lock()
    }

    /// Acquire the connection lock asynchronously.
    pub async fn lock_async(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.lock().await
    }

    /// Run a synchronous closure with the connection.
    ///
    /// Intended for use inside `tokio::task::spawn_blocking` from async callers.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.blocking_lock();
        f(&conn)
    }

    /// Close the database connection.
    pub fn close(&self) {
        // The connection is dropped when the Mutex is dropped. Nothing to do.
    }
}

impl std::fmt::Debug for MnemopiDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MnemopiDb")
            .field("db_path", &self.db_path)
            .finish()
    }
}
