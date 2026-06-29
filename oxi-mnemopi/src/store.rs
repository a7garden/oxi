//! Working memory store — ported from omp `beam/store.ts`.
//!
//! Provides remember / forget / update / get / get_context / get_stats.
//! All operations are synchronous (blocking SQLite). In async contexts,
//! wrap calls in `tokio::task::spawn_blocking`.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;
use crate::types::{MemoryRow, MemoryScope, MemoryStats, Metadata, RememberOptions, Veracity};

/// Generate a new memory ID (UUID v4, no hyphens).
fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Current UTC timestamp in ISO 8601.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Serialize metadata to JSON string for storage.
fn metadata_to_json(metadata: &Option<Metadata>) -> Option<String> {
    metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default())
}

/// Store a new memory entry in `working_memory`. Returns its ID.
///
/// Mirrors omp `remember()`. The FTS5 index is updated automatically by the
/// `wm_ai` trigger.
pub fn remember(
    conn: &Connection,
    content: &str,
    session_id: &str,
    options: &RememberOptions,
) -> Result<String> {
    let id = new_id();
    let importance = options.importance.unwrap_or(0.5);
    let source = options.source.as_deref();
    let now = now_iso();
    let timestamp = options.timestamp.as_deref().unwrap_or(&now);
    let veracity = options
        .veracity
        .as_ref()
        .map(|v| v.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let memory_type = options.memory_type.as_deref().unwrap_or("unknown");
    let scope = options
        .scope
        .as_ref()
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "global".to_string());
    let metadata_json = metadata_to_json(&options.metadata);

    conn.execute(
        "INSERT INTO working_memory
            (id, content, source, timestamp, session_id, importance,
             metadata_json, veracity, memory_type, scope)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            content,
            source,
            timestamp,
            session_id,
            importance,
            metadata_json,
            veracity,
            memory_type,
            scope,
        ],
    )?;

    trim_working_memory(conn, session_id)?;
    Ok(id)
}

/// Trim working memory to stay within limits.
///
/// Mirrors omp `trimWorkingMemory()`. Deletes entries that exceed the TTL or
/// the max-items limit.
fn trim_working_memory(conn: &Connection, session_id: &str) -> Result<()> {
    const WM_LIMIT: usize = 10_000;
    const WM_TTL_HOURS: i64 = 24;

    let cutoff = chrono::Utc::now()
        .timestamp()
        .saturating_sub(WM_TTL_HOURS * 3600);
    let cutoff_iso = chrono::DateTime::from_timestamp(cutoff, 0)
        .unwrap_or_default()
        .to_rfc3339();

    conn.execute(
        "DELETE FROM working_memory
         WHERE session_id = ?1
           AND consolidated_at IS NULL
           AND (
             timestamp < ?2 OR
             id NOT IN (
               SELECT id FROM working_memory
               WHERE session_id = ?1 AND consolidated_at IS NULL
               ORDER BY timestamp DESC
               LIMIT ?3
             )
           )",
        params![session_id, cutoff_iso, WM_LIMIT as i64],
    )?;

    Ok(())
}

/// Fetch a working memory row by ID.
pub fn get(conn: &Connection, id: &str) -> Result<Option<MemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, session_id, importance,
                metadata_json, veracity, memory_type, recall_count,
                last_recalled, valid_until, superseded_by, scope,
                author_id, author_type, channel_id, created_at
         FROM working_memory WHERE id = ?1",
    )?;

    let row = stmt.query_row(params![id], row_to_memory_row).optional()?;

    // If not in working memory, try episodic
    if row.is_none() {
        let mut stmt2 = conn.prepare(
            "SELECT id, content, source, timestamp, session_id, importance,
                    metadata_json, veracity, memory_type, recall_count,
                    last_recalled, valid_until, superseded_by, scope,
                    NULL as author_id, NULL as author_type, NULL as channel_id,
                    created_at
             FROM episodic_memory WHERE id = ?1",
        )?;
        return Ok(stmt2.query_row(params![id], row_to_memory_row).optional()?);
    }

    Ok(row)
}

/// Delete a working memory entry by ID. Returns true if deleted.
///
/// Mirrors omp `forgetWorking()`. Only deletes from `working_memory`
/// (episodic memories are immutable post-consolidation).
pub fn forget(conn: &Connection, id: &str) -> Result<bool> {
    let rows = conn.execute("DELETE FROM working_memory WHERE id = ?1", params![id])?;
    if rows > 0 {
        conn.execute(
            "DELETE FROM memory_embeddings WHERE memory_id = ?1",
            params![id],
        )?;
        return Ok(true);
    }
    Ok(false)
}

/// Update a working memory entry's content and/or importance.
///
/// Mirrors omp `updateWorking()`. Returns true if the entry was found and
/// updated.
pub fn update(
    conn: &Connection,
    id: &str,
    content: Option<&str>,
    importance: Option<f64>,
) -> Result<bool> {
    if content.is_none() && importance.is_none() {
        // No-op — nothing to update.
        return Ok(true);
    }

    let mut sql = String::from("UPDATE working_memory SET ");
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut sep = "";

    if let Some(c) = content {
        sql.push_str(sep);
        sql.push_str("content = ?");
        params_vec.push(Box::new(c.to_string()));
        sep = ", ";
    }
    if let Some(imp) = importance {
        sql.push_str(sep);
        sql.push_str("importance = ?");
        params_vec.push(Box::new(imp));
    }

    sql.push_str(" WHERE id = ?");
    params_vec.push(Box::new(id.to_string()));

    let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let rows = conn.execute(&sql, param_refs.as_slice())?;
    Ok(rows > 0)
}

/// Invalidate a memory by marking it superseded.
///
/// Mirrors omp `invalidate()`. Sets `superseded_by` and optionally stores
/// the replacement ID.
pub fn invalidate(conn: &Connection, id: &str, replacement_id: Option<&str>) -> Result<bool> {
    let replacement = replacement_id.unwrap_or("");
    let rows = conn.execute(
        "UPDATE working_memory SET superseded_by = ?1 WHERE id = ?2 AND superseded_by IS NULL",
        params![replacement, id],
    )?;
    Ok(rows > 0)
}

/// Increment recall count and update last_recalled timestamp.
pub fn touch_recall(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE working_memory
         SET recall_count = recall_count + 1, last_recalled = ?1
         WHERE id = ?2",
        params![now_iso(), id],
    )?;
    conn.execute(
        "UPDATE episodic_memory
         SET recall_count = recall_count + 1, last_recalled = ?1
         WHERE id = ?2",
        params![now_iso(), id],
    )?;
    Ok(())
}

/// Get recent working memory entries (newest first).
pub fn get_context(conn: &Connection, limit: usize) -> Result<Vec<MemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, session_id, importance,
                metadata_json, veracity, memory_type, recall_count,
                last_recalled, valid_until, superseded_by, scope,
                author_id, author_type, channel_id, created_at
         FROM working_memory
         WHERE superseded_by IS NULL
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit as i64], row_to_memory_row)?
        .filter_map(|r| r.ok())
        .collect();

    Ok(rows)
}

/// List working memory entries by `source` (subject), newest first.
///
/// Used by the `MemoryBackend` bridge to implement `list(subject)`.
pub fn list_by_source(conn: &Connection, source: &str, limit: usize) -> Result<Vec<MemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, session_id, importance,
                metadata_json, veracity, memory_type, recall_count,
                last_recalled, valid_until, superseded_by, scope,
                author_id, author_type, channel_id, created_at
         FROM working_memory
         WHERE source = ?1 AND superseded_by IS NULL
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(params![source, limit as i64], row_to_memory_row)?
        .filter_map(|r| r.ok())
        .collect();

    Ok(rows)
}

/// Gather stats about the memory store.
pub fn get_stats(conn: &Connection) -> Result<MemoryStats> {
    let working_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM working_memory WHERE superseded_by IS NULL",
        [],
        |row| row.get(0),
    )?;

    let episodic_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM episodic_memory", [], |row| row.get(0))?;

    let embedding_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM memory_embeddings", [], |row| {
            row.get(0)
        })?;

    // Group by source
    let mut by_source = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT COALESCE(source, 'unknown') as src, COUNT(*) as cnt
             FROM working_memory WHERE superseded_by IS NULL
             GROUP BY source",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        for row in rows.flatten() {
            by_source.insert(row.0, row.1);
        }
    }

    // Group by session
    let mut by_session = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT session_id, COUNT(*) as cnt
             FROM working_memory WHERE superseded_by IS NULL
             GROUP BY session_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        for row in rows.flatten() {
            by_session.insert(row.0, row.1);
        }
    }

    Ok(MemoryStats {
        working_count: working_count as usize,
        episodic_count: episodic_count as usize,
        embedding_count: embedding_count as usize,
        by_source,
        by_session,
    })
}

// ── Row mapping ──────────────────────────────────────────────────────────

pub fn row_to_memory_row(row: &rusqlite::Row) -> rusqlite::Result<MemoryRow> {
    let metadata_json: Option<String> = row.get(6)?;
    let metadata = metadata_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let veracity_str: String = row.get(7).unwrap_or_else(|_| "unknown".to_string());
    let scope_str: String = row.get(13).unwrap_or_else(|_| "global".to_string());

    Ok(MemoryRow {
        id: row.get(0)?,
        content: row.get(1)?,
        source: row.get(2)?,
        timestamp: row.get(3)?,
        session_id: row.get(4)?,
        importance: row.get(5).unwrap_or(0.5),
        metadata,
        veracity: Veracity::from_str_lossy(&veracity_str),
        memory_type: row.get(8).ok(),
        recall_count: row.get(9).ok(),
        last_recalled: row.get(10).ok(),
        valid_until: row.get(11).ok(),
        superseded_by: row.get(12).ok(),
        scope: parse_scope(&scope_str),
        author_id: row.get(14).ok(),
        author_type: row.get(15).ok(),
        channel_id: row.get(16).ok(),
        created_at: row.get(17).unwrap_or_default(),
    })
}

fn parse_scope(s: &str) -> MemoryScope {
    match s {
        "global" => MemoryScope::Global,
        "session" => MemoryScope::Session,
        "channel" => MemoryScope::Channel,
        other => MemoryScope::Other(other.to_string()),
    }
}
