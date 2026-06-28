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
    /// Applies the standard PRAGMA set:
    /// - `foreign_keys = ON`
    /// - `busy_timeout = 5000`
    /// - `journal_mode = WAL` (file databases only)
    ///
    /// Then runs `init_schema()` to ensure all tables and triggers exist.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_connection(&conn, Some(path))?;
        let db = Self {
            conn: tokio::sync::Mutex::new(conn),
            db_path: Some(path.to_path_buf()),
        };
        Ok(db)
    }

    /// Create an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_connection(&conn, None)?;
        Ok(Self {
            conn: tokio::sync::Mutex::new(conn),
            db_path: None,
        })
    }

    fn init_connection(conn: &Connection, path: Option<&Path>) -> Result<()> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        if path.is_some() {
            conn.pragma_update(None, "journal_mode", "WAL")?;
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
