//! Session lifecycle — ported from omp `beam/index.ts` BeamMemory class.
//!
//! MnemopiSessionState ties together the DB connection, session ID, and
//! high-level operations (remember, recall, sleep, stats) for a single
//! conversational session. It owns the consolidation lifecycle and
//! exposes a unified API.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::consolidate::{self, SleepResult};
use crate::error::Result;
use crate::recall;
use crate::schema::init_schema;
use crate::store;
use crate::types::{MemoryRow, MemoryStats, RecallResult, RememberOptions};

// ── Session config ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub session_id: String,
    pub working_memory_ttl_hours: i64,
    pub max_episode_chars: usize,
    pub auto_sleep_threshold: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            session_id: "default".to_string(),
            working_memory_ttl_hours: 24,
            max_episode_chars: 100_000,
            auto_sleep_threshold: 200,
        }
    }
}

// ── Session stats ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStats {
    pub working_count: usize,
    pub episodic_count: usize,
    pub unconsolidated_count: usize,
    pub oldest_unconsolidated: Option<String>,
    pub last_consolidation: Option<String>,
}

// ── Session state ────────────────────────────────────────────────────────

/// Owns a DB connection and session configuration.
///
/// In omp, this is the `BeamMemory` class. In Rust we use a struct
/// holding a `Connection` directly (no async indirection needed for
/// blocking SQLite). For async contexts, wrap operations in
/// `spawn_blocking`.
pub struct MnemopiSessionState {
    conn: Connection,
    config: SessionConfig,
}

impl MnemopiSessionState {
    /// Open an in-memory session (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn,
            config: SessionConfig::default(),
        })
    }

    /// Open a file-backed session.
    ///
    /// Uses filesystem-aware journal-mode selection (see
    /// [`crate::journal::JournalMode`]). On network filesystems (NFS/SMB/
    /// CIFS/FUSE) the engine falls back to `TRUNCATE` mode to avoid SIGBUS
    /// from mmap'd `-shm`. The path is NOT rewritten — session state is a
    /// durable primary store that must stay coherent across hosts.
    pub fn open(path: &str) -> Result<Self> {
        // Durable primary store: detect journal mode (TRUNCATE on NFS for
        // SIGBUS safety) but NEVER rewrite the path — session state must
        // stay coherent across hosts.
        let path_obj = std::path::Path::new(path);
        let mode = crate::journal::JournalMode::for_db_path(path_obj);
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", mode.as_str())?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", mode.busy_timeout_ms())?;
        init_schema(&conn)?;
        Ok(Self {
            conn,
            config: SessionConfig::default(),
        })
    }

    /// Open with custom config.
    ///
    /// See [`open`](Self::open) for journal-mode selection semantics.
    pub fn open_with_config(path: Option<&str>, config: SessionConfig) -> Result<Self> {
        let conn = match path {
            Some(p) => {
                // Durable primary store — keep shared path, only adjust
                // journal mode for NFS SIGBUS safety.
                let path_obj = std::path::Path::new(p);
                let mode = crate::journal::JournalMode::for_db_path(path_obj);
                let c = Connection::open(p)?;
                c.pragma_update(None, "journal_mode", mode.as_str())?;
                c.pragma_update(None, "synchronous", "NORMAL")?;
                c.pragma_update(None, "foreign_keys", "ON")?;
                c.pragma_update(None, "busy_timeout", mode.busy_timeout_ms())?;
                c
            }
            None => Connection::open_in_memory()?,
        };
        init_schema(&conn)?;
        Ok(Self { conn, config })
    }

    /// Get a reference to the underlying connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get the session config.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.config.session_id
    }

    // ── Memory operations ────────────────────────────────────────────────

    /// Store a new working memory entry.
    pub fn remember(&self, content: &str, opts: &RememberOptions) -> Result<String> {
        store::remember(&self.conn, content, &self.config.session_id, opts, None)
    }

    /// Recall memories matching a query.
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<RecallResult>> {
        let opts = crate::types::RecallOptions {
            limit: Some(limit),
            ..Default::default()
        };
        recall::recall(&self.conn, query, &self.config.session_id, &opts)
    }

    /// Get a memory by ID.
    pub fn get(&self, id: &str) -> Result<Option<MemoryRow>> {
        store::get(&self.conn, id)
    }

    /// Delete a memory.
    pub fn forget(&self, id: &str) -> Result<bool> {
        store::forget(&self.conn, id)
    }

    /// Update a memory's content.
    pub fn update(&self, id: &str, content: &str) -> Result<bool> {
        store::update(&self.conn, id, Some(content), None)
    }

    /// Get memory stats.
    pub fn get_stats(&self) -> Result<MemoryStats> {
        store::get_stats(&self.conn)
    }

    // ── Consolidation ────────────────────────────────────────────────────

    /// Run the sleep/consolidation cycle.
    pub fn sleep(&self, dry_run: bool) -> Result<SleepResult> {
        consolidate::sleep(
            &self.conn,
            &self.config.session_id,
            self.config.working_memory_ttl_hours,
            Some(self.config.max_episode_chars),
            dry_run,
        )
    }

    /// Check if auto-sleep should trigger.
    pub fn should_auto_sleep(&self) -> Result<bool> {
        let stats = self.session_stats()?;
        Ok(stats.unconsolidated_count >= self.config.auto_sleep_threshold)
    }

    /// Run auto-sleep if threshold is exceeded.
    pub fn maybe_auto_sleep(&self) -> Result<Option<SleepResult>> {
        if self.should_auto_sleep()? {
            Ok(Some(self.sleep(false)?))
        } else {
            Ok(None)
        }
    }

    // ── Session stats ────────────────────────────────────────────────────

    /// Get session-level statistics.
    pub fn session_stats(&self) -> Result<SessionStats> {
        let working_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM working_memory WHERE COALESCE(session_id, 'default') = ?1",
                rusqlite::params![self.config.session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let unconsolidated_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM working_memory
                 WHERE COALESCE(session_id, 'default') = ?1 AND consolidated_at IS NULL",
                rusqlite::params![self.config.session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let episodic_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM episodic_memory WHERE COALESCE(session_id, 'default') = ?1",
                rusqlite::params![self.config.session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let oldest_unconsolidated: Option<String> = self
            .conn
            .query_row(
                "SELECT MIN(timestamp) FROM working_memory
                 WHERE COALESCE(session_id, 'default') = ?1 AND consolidated_at IS NULL",
                rusqlite::params![self.config.session_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let last_consolidation: Option<String> = self
            .conn
            .query_row(
                "SELECT MAX(created_at) FROM consolidation_log WHERE session_id = ?1",
                rusqlite::params![self.config.session_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        Ok(SessionStats {
            working_count: working_count as usize,
            episodic_count: episodic_count as usize,
            unconsolidated_count: unconsolidated_count as usize,
            oldest_unconsolidated,
            last_consolidation,
        })
    }

    /// Get the consolidation log.
    #[allow(clippy::type_complexity)]
    pub fn consolidation_log(&self, limit: i64) -> Result<Vec<(i64, String, i64, String, String)>> {
        consolidate::get_consolidation_log(&self.conn, &self.config.session_id, limit)
    }
}

// ── Free functions (for facade use) ──────────────────────────────────────

/// Get session-level statistics (free function version).
pub fn session_stats(conn: &Connection, session_id: &str) -> Result<SessionStats> {
    let working_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM working_memory WHERE COALESCE(session_id, 'default') = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let unconsolidated_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM working_memory
             WHERE COALESCE(session_id, 'default') = ?1 AND consolidated_at IS NULL",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let episodic_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodic_memory WHERE COALESCE(session_id, 'default') = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let oldest_unconsolidated: Option<String> = conn
        .query_row(
            "SELECT MIN(timestamp) FROM working_memory
             WHERE COALESCE(session_id, 'default') = ?1 AND consolidated_at IS NULL",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let last_consolidation: Option<String> = conn
        .query_row(
            "SELECT MAX(created_at) FROM consolidation_log WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    Ok(SessionStats {
        working_count: working_count as usize,
        episodic_count: episodic_count as usize,
        unconsolidated_count: unconsolidated_count as usize,
        oldest_unconsolidated,
        last_consolidation,
    })
}

/// Check whether auto-sleep should trigger (free function version).
pub fn should_auto_sleep(conn: &Connection, session_id: &str, threshold: usize) -> bool {
    let unconsolidated: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM working_memory
             WHERE COALESCE(session_id, 'default') = ?1 AND consolidated_at IS NULL",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    (unconsolidated as usize) >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RememberOptions;

    #[test]
    fn test_session_remember_and_recall() {
        let session = MnemopiSessionState::open_in_memory().unwrap();

        let id = session
            .remember(
                "User prefers Rust over Python for systems programming",
                &RememberOptions {
                    source: Some("chat".into()),
                    importance: Some(0.8),
                    ..Default::default()
                },
            )
            .unwrap();

        let results = session
            .recall("programming language preferences", 10)
            .unwrap();
        assert!(!results.is_empty());

        let mem = session.get(&id).unwrap();
        assert!(mem.is_some());
    }

    #[test]
    fn test_session_sleep_lifecycle() {
        let session = MnemopiSessionState::open_in_memory().unwrap();

        // Insert old memories by manipulating timestamps directly
        let old_ts = (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        session.conn.execute(
            "INSERT INTO working_memory (id, content, source, timestamp, session_id, importance, scope, veracity)
             VALUES ('wm-old-1', 'Old memory content', 'test', ?1, 'default', 0.7, 'session', 'stated')",
            rusqlite::params![old_ts],
        ).unwrap();

        let stats = session.session_stats().unwrap();
        assert_eq!(stats.unconsolidated_count, 1);

        let result = session.sleep(false).unwrap();
        assert_eq!(result.status, "consolidated");

        let stats = session.session_stats().unwrap();
        assert_eq!(stats.unconsolidated_count, 0);
        assert!(stats.episodic_count > 0);
    }

    #[test]
    fn test_auto_sleep_threshold() {
        let config = SessionConfig {
            auto_sleep_threshold: 2,
            ..Default::default()
        };

        let session = MnemopiSessionState::open_with_config(None, config).unwrap();

        // Insert 2 working memories (below threshold)
        session
            .remember("First memory", &RememberOptions::default())
            .unwrap();
        assert!(!session.should_auto_sleep().unwrap());

        session
            .remember("Second memory", &RememberOptions::default())
            .unwrap();
        assert!(session.should_auto_sleep().unwrap());
    }

    #[test]
    fn test_consolidation_log() {
        let session = MnemopiSessionState::open_in_memory().unwrap();
        let old_ts = (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        session.conn.execute(
            "INSERT INTO working_memory (id, content, source, timestamp, session_id, importance, scope, veracity)
             VALUES ('wm-log-1', 'Log test', 'test', ?1, 'default', 0.5, 'session', 'unknown')",
            rusqlite::params![old_ts],
        ).unwrap();

        session.sleep(false).unwrap();

        let log = session.consolidation_log(10).unwrap();
        assert!(!log.is_empty());
    }
}
