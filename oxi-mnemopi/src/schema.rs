//! Schema initialization — ported from omp `beam/schema.ts:initBeam`.
//!
//! Uses the omp idempotent pattern: `CREATE TABLE IF NOT EXISTS` for all DDL,
//! `PRAGMA table_info` check before `ALTER TABLE ADD COLUMN`.

use rusqlite::{Connection, Row};

use crate::error::Result;

/// Initialize all tables, indexes, and FTS5 triggers.
///
/// Safe to call on every open — all statements are idempotent.
pub fn init_schema(conn: &Connection) -> Result<()> {
    // ── Tables ──────────────────────────────────────────────────────────

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS working_memory (
            id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            source TEXT,
            timestamp TEXT,
            session_id TEXT DEFAULT 'default',
            importance REAL DEFAULT 0.5,
            metadata_json TEXT,
            veracity TEXT DEFAULT 'unknown',
            memory_type TEXT DEFAULT 'unknown',
            consolidated_at TEXT,
            recall_count INTEGER DEFAULT 0,
            last_recalled TIMESTAMP DEFAULT NULL,
            valid_until TIMESTAMP DEFAULT NULL,
            superseded_by TEXT DEFAULT NULL,
            scope TEXT DEFAULT 'global',
            author_id TEXT DEFAULT NULL,
            author_type TEXT DEFAULT NULL,
            channel_id TEXT DEFAULT NULL,
            trust_tier TEXT DEFAULT 'STATED',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS episodic_memory (
            rowid INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT UNIQUE NOT NULL,
            content TEXT NOT NULL,
            source TEXT,
            timestamp TEXT,
            session_id TEXT DEFAULT 'default',
            importance REAL DEFAULT 0.5,
            metadata_json TEXT,
            veracity TEXT DEFAULT 'unknown',
            memory_type TEXT DEFAULT 'unknown',
            summary_of TEXT DEFAULT '',
            tier INTEGER DEFAULT 1,
            degraded_at TEXT,
            binary_vector BLOB,
            recall_count INTEGER DEFAULT 0,
            last_recalled TIMESTAMP DEFAULT NULL,
            valid_until TIMESTAMP DEFAULT NULL,
            superseded_by TEXT DEFAULT NULL,
            scope TEXT DEFAULT 'global',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS memory_embeddings (
            memory_id TEXT PRIMARY KEY,
            embedding_json TEXT NOT NULL,
            model TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS scratchpad (
            id TEXT PRIMARY KEY,
            content TEXT,
            session_id TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS consolidation_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT,
            items_consolidated INTEGER,
            summary_preview TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        ",
    )?;

    // ── FTS5 virtual tables ─────────────────────────────────────────────

    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_working USING fts5(
            id UNINDEXED, content
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS fts_episodes USING fts5(
            content, content='episodic_memory', content_rowid='rowid'
        );
        ",
    )?;

    // ── FTS sync triggers (6) ───────────────────────────────────────────

    conn.execute_batch(
        "
        -- working_memory triggers
        CREATE TRIGGER IF NOT EXISTS wm_ai AFTER INSERT ON working_memory BEGIN
            INSERT INTO fts_working(id, content) VALUES (new.id, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS wm_ad AFTER DELETE ON working_memory BEGIN
            DELETE FROM fts_working WHERE id = old.id;
        END;
        CREATE TRIGGER IF NOT EXISTS wm_au AFTER UPDATE OF content ON working_memory BEGIN
            DELETE FROM fts_working WHERE id = old.id;
            INSERT INTO fts_working(id, content) VALUES (new.id, new.content);
        END;

        -- episodic_memory triggers
        CREATE TRIGGER IF NOT EXISTS em_ai AFTER INSERT ON episodic_memory BEGIN
            INSERT INTO fts_episodes(rowid, content) VALUES (new.rowid, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS em_ad AFTER DELETE ON episodic_memory BEGIN
            INSERT INTO fts_episodes(fts_episodes, rowid, content) VALUES ('delete', old.rowid, old.content);
        END;
        CREATE TRIGGER IF NOT EXISTS em_au AFTER UPDATE OF content ON episodic_memory BEGIN
            INSERT INTO fts_episodes(fts_episodes, rowid, content) VALUES ('delete', old.rowid, old.content);
            INSERT INTO fts_episodes(rowid, content) VALUES (new.rowid, new.content);
        END;
        ",
    )?;

    // ── Indexes ─────────────────────────────────────────────────────────

    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_wm_session ON working_memory(session_id);
        CREATE INDEX IF NOT EXISTS idx_wm_timestamp ON working_memory(timestamp);
        CREATE INDEX IF NOT EXISTS idx_wm_scope ON working_memory(scope);
        CREATE INDEX IF NOT EXISTS idx_em_session ON episodic_memory(session_id);
        CREATE INDEX IF NOT EXISTS idx_em_tier ON episodic_memory(tier);
        ",
    )?;

    // ── Phase 3 tables ────────────────────────────────────────────────────

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memory_annotations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            value TEXT NOT NULL,
            source TEXT DEFAULT '',
            confidence REAL DEFAULT 1.0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS memory_triples (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object TEXT NOT NULL,
            valid_from TEXT,
            valid_until TEXT,
            source TEXT,
            confidence REAL DEFAULT 0.5,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS episodic_gists (
            id TEXT PRIMARY KEY,
            text TEXT NOT NULL,
            timestamp TEXT,
            participants_json TEXT DEFAULT '[]',
            location TEXT,
            emotion TEXT,
            time_scope TEXT,
            memory_id TEXT
        );

        CREATE TABLE IF NOT EXISTS episodic_facts (
            fact_id TEXT PRIMARY KEY,
            session_id TEXT,
            subject TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object TEXT NOT NULL,
            timestamp TEXT,
            source_memory_id TEXT,
            confidence REAL DEFAULT 0.5
        );

        CREATE TABLE IF NOT EXISTS episodic_edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source TEXT NOT NULL,
            target TEXT NOT NULL,
            edge_type TEXT NOT NULL,
            weight REAL DEFAULT 1.0,
            timestamp TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        CREATE TABLE IF NOT EXISTS consolidated_facts (
            id TEXT PRIMARY KEY,
            subject TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object TEXT NOT NULL,
            confidence REAL DEFAULT 0.5,
            mention_count INTEGER DEFAULT 1,
            sources_json TEXT DEFAULT '[]',
            veracity TEXT DEFAULT 'unknown',
            first_seen TEXT,
            last_seen TEXT,
            conflict_count INTEGER DEFAULT 0,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        -- Indexes for Phase 3 tables
        CREATE INDEX IF NOT EXISTS idx_ann_memory ON memory_annotations(memory_id);
        CREATE INDEX IF NOT EXISTS idx_ann_kind ON memory_annotations(kind);
        CREATE INDEX IF NOT EXISTS idx_triples_subject ON memory_triples(subject);
        CREATE INDEX IF NOT EXISTS idx_triples_object ON memory_triples(object);
        CREATE INDEX IF NOT EXISTS idx_gists_memory ON episodic_gists(memory_id);
        CREATE INDEX IF NOT EXISTS idx_facts_subject ON episodic_facts(subject);
        CREATE INDEX IF NOT EXISTS idx_edges_source ON episodic_edges(source);
        CREATE INDEX IF NOT EXISTS idx_edges_target ON episodic_edges(target);
        CREATE INDEX IF NOT EXISTS idx_cf_subject ON consolidated_facts(subject);
        ",
    )?;

    Ok(())
}

/// Add a column to a table if it doesn't already exist (idempotent).
///
/// Mirrors omp's `addColumnIfMissing(db, table, column, definition)`.
pub fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let column_names: Vec<String> = stmt
        .query_map([], row_to_column_name)?
        .filter_map(|r| r.ok())
        .collect();

    if column_names.iter().any(|c| c == column) {
        return Ok(false);
    }

    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(true)
}

fn row_to_column_name(row: &Row) -> rusqlite::Result<String> {
    row.get::<_, String>(1) // "name" is the second column in PRAGMA table_info
}
