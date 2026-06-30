//! SQLite-backed job queue + Phase 1 / Phase 2 workers for the
//! autonomous memory pipeline (`memory_summary.rs`).
//!
//! This module is split out so the memory artifact surface
//! (prompts, paths, redaction) stays in `memory_summary.rs` while
//! the runtime machinery (SQLite schema, lease/heartbeat,
//! LLM-backed extraction + consolidation) lives here.
//!
//! **Status**: skeletons only. The runtime spawn hook
//! (`services::start_memory_pipeline`) will instantiate and drive
//! these in the follow-up PR. Each worker is a pure function over
//! the SQLite connection so it's unit-testable in isolation.

#![allow(missing_docs)]
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

#[allow(unused_imports)]
use super::memory_summary::{
    CONSOLIDATION_SYSTEM_PROMPT, CONSOLIDATION_USER_TEMPLATE, DEFAULT_GLOBAL_LEASE_SECONDS,
    STAGE_ONE_SYSTEM_PROMPT, STAGE_ONE_USER_TEMPLATE,
};

// ── Database schema ──────────────────────────────────────────

/// Initialize the SQLite schema (idempotent). Safe to call on every
/// open. Adds three tables:
///
/// - `memory_threads` — registry of threads we've ever observed
/// - `memory_stage1_outputs` — one row per (thread, run) LLM extraction
/// - `memory_jobs` — per-project consolidation queue (with lease)
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memory_threads (
            thread_id TEXT PRIMARY KEY,
            cwd TEXT NOT NULL,
            source_updated_at INTEGER NOT NULL,
            last_extracted_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS memory_stage1_outputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            cwd TEXT NOT NULL,
            rollout_summary TEXT NOT NULL,
            rollout_slug TEXT,
            raw_memory TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            source_updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_stage1_cwd
            ON memory_stage1_outputs(cwd, created_at DESC);

        CREATE TABLE IF NOT EXISTS memory_jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cwd TEXT NOT NULL,
            kind TEXT NOT NULL,                    -- 'stage1' | 'global'
            thread_id TEXT,                        -- NULL for global
            ownership_token TEXT,
            claimed_at INTEGER,
            lease_until INTEGER,
            last_error TEXT,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_jobs_kind
            ON memory_jobs(kind, created_at ASC);
        ",
    )
}

// ── Phase 1: per-session extraction ──────────────────────────

/// A single session (thread) that is eligible for Phase 1
/// processing.
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub cwd: String,
    pub source_updated_at: i64,
}

/// Collect eligible threads from `sessions_dir` for the given cwd.
/// `now` is the current Unix timestamp in seconds.
pub fn collect_threads(
    conn: &Connection,
    sessions_dir: &Path,
    cwd: &str,
    now: i64,
    max_age_days: i64,
    min_idle_hours: i64,
) -> rusqlite::Result<Vec<ThreadInfo>> {
    // omp's equivalent: walks `<cwd>/<session_id>.jsonl`, parses
    // each session header, applies the age/idle/limit filters,
    // upserts into `memory_threads`. We re-export the deterministic
    // SQL surface only — the JSONL walker lives in the next PR.
    let max_age = now - max_age_days * 24 * 3600;
    let min_idle = now - min_idle_hours * 3600;
    let mut stmt = conn.prepare(
        "SELECT thread_id, cwd, source_updated_at
         FROM memory_threads
         WHERE cwd = ?1
           AND source_updated_at >= ?2
           AND (last_extracted_at IS NULL OR last_extracted_at <= ?3)
         ORDER BY source_updated_at DESC",
    )?;
    let rows = stmt
        .query_map(params![cwd, max_age, min_idle], |r| {
            Ok(ThreadInfo {
                thread_id: r.get(0)?,
                cwd: r.get(1)?,
                source_updated_at: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let _ = sessions_dir; // consumed in the next PR
    Ok(rows)
}

/// Insert / update the row for a single thread observation.
pub fn upsert_thread(conn: &Connection, info: &ThreadInfo) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO memory_threads (thread_id, cwd, source_updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(thread_id) DO UPDATE SET
           cwd = excluded.cwd,
           source_updated_at = MAX(source_updated_at, excluded.source_updated_at)",
        params![info.thread_id, info.cwd, info.source_updated_at],
    )?;
    Ok(())
}

/// Atomically claim one Stage 1 job. Returns the (thread_id, cwd)
/// pair plus the ownership token if a job was claimed.
pub fn claim_stage1_job(
    conn: &Connection,
    now: i64,
) -> rusqlite::Result<Option<(String, String, String)>> {
    // Begin → claim → return. The lease prevents two oxi processes
    // from double-extracting the same thread.
    conn.execute_batch("BEGIN")?;
    let candidate: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT id, thread_id, cwd
             FROM memory_jobs
             WHERE kind = 'stage1'
               AND (claimed_at IS NULL OR lease_until < ?1)
             ORDER BY created_at ASC
             LIMIT 1",
            params![now],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((id, thread_id, cwd)) = candidate else {
        conn.execute_batch("ROLLBACK")?;
        return Ok(None);
    };
    let token = format!("{:x}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
    conn.execute(
        "UPDATE memory_jobs
         SET claimed_at = ?1,
             lease_until = ?2,
             ownership_token = ?3,
             attempts = attempts + 1
         WHERE id = ?4",
        params![now, now + 60, token, id],
    )?;
    conn.execute_batch("COMMIT")?;
    Ok(Some((thread_id, cwd, token)))
}

/// Insert the Stage 1 output for a thread and mark it extracted.
#[allow(clippy::too_many_arguments)]
pub fn write_stage1_output(
    conn: &Connection,
    thread_id: &str,
    cwd: &str,
    rollout_summary: &str,
    rollout_slug: Option<&str>,
    raw_memory: &str,
    now: i64,
    source_updated_at: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO memory_stage1_outputs
            (thread_id, cwd, rollout_summary, rollout_slug, raw_memory,
             created_at, source_updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            thread_id,
            cwd,
            rollout_summary,
            rollout_slug,
            raw_memory,
            now,
            source_updated_at,
        ],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "UPDATE memory_threads
         SET last_extracted_at = ?1
         WHERE thread_id = ?2",
        params![now, thread_id],
    )?;
    Ok(id)
}

// ── Phase 2: cross-session consolidation ─────────────────────

/// Try to claim the global Phase 2 job for `cwd`. Returns
/// `Some((token, lease))` when claimed; `None` when another oxi
/// process already owns it.
pub fn try_claim_phase2(
    conn: &Connection,
    cwd: &str,
    now: i64,
    lease_seconds: i64,
) -> rusqlite::Result<Option<(String, i64)>> {
    let token = format!("{:x}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
    let lease = now + lease_seconds;
    let updated = conn.execute(
        "UPDATE memory_jobs
         SET ownership_token = ?1,
             claimed_at = ?2,
             lease_until = ?3,
             attempts = attempts + 1
         WHERE kind = 'global'
           AND cwd = ?4
           AND (lease_until IS NULL OR lease_until < ?5)",
        params![token, now, lease, cwd, now],
    )?;
    if updated == 0 {
        return Ok(None);
    }
    Ok(Some((token, lease)))
}

/// Refresh the heart-beat on a held Phase 2 lease.
pub fn heartbeat_phase2(
    conn: &Connection,
    cwd: &str,
    token: &str,
    lease_seconds: i64,
    now: i64,
) -> rusqlite::Result<bool> {
    let updated = conn.execute(
        "UPDATE memory_jobs
         SET lease_until = ?1
         WHERE kind = 'global'
           AND cwd = ?2
           AND ownership_token = ?3",
        params![now + lease_seconds, cwd, token],
    )?;
    Ok(updated > 0)
}

/// Release the Phase 2 lease (success).
pub fn finish_phase2(conn: &Connection, cwd: &str, token: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE memory_jobs
         SET lease_until = NULL, claimed_at = NULL, ownership_token = NULL,
             last_error = NULL
         WHERE kind = 'global' AND cwd = ?1 AND ownership_token = ?2",
        params![cwd, token],
    )?;
    let _ = now; // currently unused; reserved for future stamp
    Ok(())
}

/// Load all Stage 1 outputs for `cwd`, newest first, capped at `limit`.
pub fn list_stage1_outputs(
    conn: &Connection,
    cwd: &str,
    limit: usize,
) -> rusqlite::Result<Vec<Stage1Row>> {
    let mut stmt = conn.prepare(
        "SELECT id, thread_id, rollout_summary, raw_memory, created_at
         FROM memory_stage1_outputs
         WHERE cwd = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![cwd, limit as i64], |r| {
            Ok(Stage1Row {
                id: r.get(0)?,
                thread_id: r.get(1)?,
                rollout_summary: r.get(2)?,
                raw_memory: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One row from `memory_stage1_outputs`.
#[derive(Debug, Clone)]
pub struct Stage1Row {
    pub id: i64,
    pub thread_id: String,
    pub rollout_summary: String,
    pub raw_memory: String,
    pub created_at: i64,
}

/// Build the user-turn text for Stage 1 by templating
/// `STAGE_ONE_USER_TEMPLATE` with the session metadata + persistable
/// items JSON.
pub fn render_stage1_user(thread_id: &str, response_items_json: &str) -> String {
    STAGE_ONE_USER_TEMPLATE
        .replace("{{thread_id}}", thread_id)
        .replace("{{response_items_json}}", response_items_json)
}

/// Build the user-turn text for Stage 2 from accumulated Stage 1 rows.
pub fn render_stage2_user(raw_memories: &str, rollout_summaries: &str) -> String {
    CONSOLIDATION_USER_TEMPLATE
        .replace("{{raw_memories}}", raw_memories)
        .replace("{{rollout_summaries}}", rollout_summaries)
}

/// Re-exported prompt constants for callers that prefer using the
/// raw const directly (e.g. custom user templates).
pub use super::memory_summary::STAGE_ONE_SYSTEM_PROMPT as STAGE1_SYSTEM_PROMPT;

pub use super::memory_summary::CONSOLIDATION_SYSTEM_PROMPT as CONSOLIDATION_SYSTEM_PROMPT_EXPORT;

// ── Worker entry points (skeletons; LLM call sits in next PR) ──

/// Run one Stage 1 iteration: claim a job, call the LLM, persist the
/// output. Returns `Ok(true)` if a job was processed; `Ok(false)`
/// when there is nothing to do.
///
/// The actual `Oxi` call is out of scope for this version: pass
/// `None` for the LLM client and the worker becomes a no-op that
/// logs once and skips, so the rest of the integration can be wired
/// and tested.
pub async fn run_stage1_iteration(
    conn: &Connection,
    sessions_dir: &Path,
    cwd: &str,
    now: i64,
    llm_provider: Option<&Arc<dyn oxi_ai::Provider>>,
    llm_model: Option<&oxi_ai::Model>,
) -> rusqlite::Result<bool> {
    let (Some(_provider), Some(_model)) = (llm_provider, llm_model) else {
        tracing::debug!("memory_summary: stage 1 skipped (no LLM wired)");
        let _ = init_schema(conn);
        return Ok(false);
    };
    init_schema(conn)?;
    let Some((thread_id, _cwd2, _token)) = claim_stage1_job(conn, now)? else {
        return Ok(false);
    };
    let _ = (sessions_dir, cwd);
    tracing::debug!(thread_id, "stage 1: claimed job (LLM call pending wiring)");
    Ok(false)
}

pub async fn run_stage2_iteration(
    conn: &Connection,
    memory_root: &Path,
    cwd: &str,
    now: i64,
    llm_provider: Option<&Arc<dyn oxi_ai::Provider>>,
    llm_model: Option<&oxi_ai::Model>,
) -> rusqlite::Result<bool> {
    let (Some(_provider), Some(_model)) = (llm_provider, llm_model) else {
        tracing::debug!("memory_summary: stage 2 skipped (no LLM wired)");
        return Ok(false);
    };
    let Some((_token, _lease)) = try_claim_phase2(conn, cwd, now, DEFAULT_GLOBAL_LEASE_SECONDS)?
    else {
        return Ok(false);
    };
    let _ = memory_root;
    tracing::debug!(cwd, "stage 2: claimed lease (LLM call pending wiring)");
    Ok(false)
}

/// Owned `MemoryDb` connection helper. Used by `services` to spawn
/// the worker without exposing the raw `Connection`.
pub fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;",
    )?;
    init_schema(&conn)?;
    Ok(conn)
}

/// A lazily-evaluated path for the pipeline's working DB.
pub static PIPELINE_DB_PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| None);

/// Resolve where the pipeline DB should live. Mirrors
/// `MemoryBackend` placement: `<home>/memory/pipeline.db`.
pub fn pipeline_db_path(home: &Path) -> PathBuf {
    home.join("memory").join("pipeline.db")
}
