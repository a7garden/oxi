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
    // Begin → claim → return. The lease prevents two oxicode processes
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
/// `Some((token, lease))` when claimed; `None` when another oxicode
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

const DEFAULT_STAGE1_RETRY_SECS: i64 = 120;
const DEFAULT_PHASE2_RETRY_SECS: i64 = 180;

/// Mark a stage1 job as failed with a retry delay.
fn mark_stage1_failed(
    conn: &Connection,
    token: &str,
    retry_secs: i64,
    reason: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE memory_jobs
         SET claimed_at = NULL,
             lease_until = NULL,
             ownership_token = NULL,
             retry_at = ?1,
             last_error = ?2
         WHERE ownership_token = ?3",
        params![now + retry_secs, reason, token],
    )?;
    Ok(())
}

/// Mark stage1 as succeeded with no output (empty extraction).
fn mark_stage1_succeeded_no_output(
    conn: &Connection,
    token: &str,
    now: i64,
    _cwd: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE memory_jobs
         SET claimed_at = NULL,
             lease_until = NULL,
             ownership_token = NULL,
             status = 'done',
             last_success_watermark = ?1
         WHERE ownership_token = ?2",
        params![now, token],
    )?;
    Ok(())
}

/// Mark phase2 as failed with retry.
fn mark_global_phase2_failed(
    conn: &Connection,
    token: &str,
    retry_secs: i64,
    reason: &str,
    now: i64,
    _cwd: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE memory_jobs
         SET claimed_at = NULL,
             lease_until = NULL,
             ownership_token = NULL,
             retry_at = ?1,
             last_error = ?2
         WHERE ownership_token = ?3",
        params![now + retry_secs, reason, token],
    )?;
    Ok(())
}

/// Mark phase2 as succeeded using finish_phase2.
fn mark_global_phase2_succeeded(
    conn: &Connection,
    token: &str,
    now: i64,
    cwd: &str,
) -> rusqlite::Result<()> {
    finish_phase2(conn, cwd, token, now)
}

// ── Worker entry points ──────────────────────────────────────────

/// Run one Stage 1 iteration: claim a job, call the LLM, persist the
/// output. Returns `Ok(true)` if a job was processed; `Ok(false)`
/// when there is nothing to do.
pub async fn run_stage1_iteration(
    conn: &Connection,
    sessions_dir: &Path,
    cwd: &str,
    now: i64,
    llm_provider: Option<&Arc<dyn oxicode_ai::Provider>>,
    llm_model: Option<&oxicode_ai::Model>,
) -> rusqlite::Result<bool> {
    let (Some(provider), Some(model)) = (llm_provider, llm_model) else {
        return Ok(false);
    };
    init_schema(conn)?;
    let Some((thread_id, _cwd2, token)) = claim_stage1_job(conn, now)? else {
        return Ok(false);
    };

    // Read the session JSONL for this thread.
    let session_path = sessions_dir.join(format!("{thread_id}.jsonl"));
    let session_content = match std::fs::read_to_string(&session_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(thread_id, error = %e, "stage 1: failed to read session");
            mark_stage1_failed(conn, &token, DEFAULT_STAGE1_RETRY_SECS, &e.to_string(), now)?;
            return Ok(true);
        }
    };

    // Truncate to a reasonable size to avoid blowing context.
    let truncated = if session_content.len() > 50_000 {
        &session_content[..50_000]
    } else {
        &session_content
    };

    let user_prompt = render_stage1_user(&thread_id, truncated);
    let system_prompt = STAGE1_SYSTEM_PROMPT;

    // Call the LLM.
    let llm_result = call_llm(provider, model, system_prompt, &user_prompt).await;

    match llm_result {
        Ok(response) => {
            // Parse the JSON response.
            let parsed = parse_stage1_output(&response);
            match parsed {
                Some((raw_memory, rollout_summary, rollout_slug)) => {
                    if raw_memory.is_empty() && rollout_summary.is_empty() {
                        mark_stage1_succeeded_no_output(conn, &token, now, cwd)?;
                    } else {
                        write_stage1_output(
                            conn,
                            &thread_id,
                            cwd,
                            &rollout_summary,
                            rollout_slug.as_deref(),
                            &raw_memory,
                            now,
                            now,
                        )?;
                    }
                    tracing::info!(thread_id, "stage 1: extraction complete");
                }
                None => {
                    tracing::warn!(thread_id, "stage 1: failed to parse LLM output");
                    mark_stage1_failed(
                        conn,
                        &token,
                        DEFAULT_STAGE1_RETRY_SECS,
                        "unparseable LLM output",
                        now,
                    )?;
                }
            }
            Ok(true)
        }
        Err(e) => {
            tracing::warn!(thread_id, error = %e, "stage 1: LLM call failed");
            mark_stage1_failed(conn, &token, DEFAULT_STAGE1_RETRY_SECS, &e, now)?;
            Ok(true)
        }
    }
}

/// Run one Stage 2 iteration: claim the global job, collect Stage 1
/// outputs, call the consolidation LLM, write artifacts.
pub async fn run_stage2_iteration(
    conn: &Connection,
    memory_root: &Path,
    cwd: &str,
    now: i64,
    llm_provider: Option<&Arc<dyn oxicode_ai::Provider>>,
    llm_model: Option<&oxicode_ai::Model>,
) -> rusqlite::Result<bool> {
    let (Some(provider), Some(model)) = (llm_provider, llm_model) else {
        return Ok(false);
    };
    let Some((token, _lease)) = try_claim_phase2(conn, cwd, now, DEFAULT_GLOBAL_LEASE_SECONDS)?
    else {
        return Ok(false);
    };

    // Collect Stage 1 outputs for this cwd.
    let outputs = list_stage1_outputs(conn, cwd, 200)?;
    if outputs.is_empty() {
        mark_global_phase2_failed(
            conn,
            &token,
            DEFAULT_PHASE2_RETRY_SECS,
            "no stage1 outputs",
            now,
            cwd,
        )?;
        return Ok(true);
    }

    let raw_memories: Vec<String> = outputs.iter().map(|o| o.raw_memory.clone()).collect();
    let rollout_summaries: Vec<String> =
        outputs.iter().map(|o| o.rollout_summary.clone()).collect();
    let user_prompt = render_stage2_user(
        &raw_memories.join("\n---\n"),
        &rollout_summaries.join("\n---\n"),
    );
    let llm_result = call_llm(
        provider,
        model,
        CONSOLIDATION_SYSTEM_PROMPT_EXPORT,
        &user_prompt,
    )
    .await;

    match llm_result {
        Ok(response) => {
            let parsed = parse_consolidation_output(&response);
            match parsed {
                Some(consolidated) => {
                    // Write artifacts atomically.
                    let _ = std::fs::create_dir_all(memory_root);
                    write_artifact(memory_root, "MEMORY.md", &consolidated.memory_md);
                    write_artifact(
                        memory_root,
                        "memory_summary.md",
                        &consolidated.memory_summary,
                    );
                    for skill in &consolidated.skills {
                        let skill_dir = memory_root.join("skills").join(&skill.name);
                        let _ = std::fs::create_dir_all(&skill_dir);
                        write_artifact(&skill_dir, "SKILL.md", &skill.content);
                    }
                    mark_global_phase2_succeeded(conn, &token, now, cwd)?;
                    tracing::info!(cwd, "stage 2: consolidation complete");
                }
                None => {
                    mark_global_phase2_failed(
                        conn,
                        &token,
                        DEFAULT_PHASE2_RETRY_SECS,
                        "unparseable consolidation output",
                        now,
                        cwd,
                    )?;
                }
            }
            Ok(true)
        }
        Err(e) => {
            mark_global_phase2_failed(conn, &token, DEFAULT_PHASE2_RETRY_SECS, &e, now, cwd)?;
            Ok(true)
        }
    }
}

// ── LLM call helper ──────────────────────────────────────────────

async fn call_llm(
    provider: &Arc<dyn oxicode_ai::Provider>,
    model: &oxicode_ai::Model,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, String> {
    use futures::StreamExt;
    use oxicode_ai::{Context, Message, UserMessage};

    let mut context = Context::new();
    context.set_system_prompt(system_prompt);
    context.add_message(Message::User(UserMessage::new(user_prompt)));
    let mut text = String::new();
    let mut stream = provider
        .stream(model, &context, None)
        .await
        .map_err(|e| format!("provider stream error: {e}"))?;
    while let Some(event) = stream.next().await {
        match event {
            oxicode_ai::ProviderEvent::TextDelta { delta, .. } => text.push_str(&delta),
            oxicode_ai::ProviderEvent::Done { message, .. } if text.is_empty() => {
                for block in &message.content {
                    if let oxicode_ai::ContentBlock::Text(t) = block {
                        text = t.text.clone();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(text.trim().to_string())
}

// ── Output parsers ──────────────────────────────────────────────

/// Parse Stage 1 JSON output: { rollout_summary, rollout_slug, raw_memory }.
fn parse_stage1_output(text: &str) -> Option<(String, String, Option<String>)> {
    // Strip markdown code fence if present.
    let json_str = text
        .strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```\n"))
        .and_then(|s| s.strip_suffix("\n```"))
        .unwrap_or(text);

    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let raw_memory = v.get("raw_memory")?.as_str()?.to_string();
    let rollout_summary = v.get("rollout_summary")?.as_str()?.to_string();
    let rollout_slug = v
        .get("rollout_slug")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    Some((raw_memory, rollout_summary, rollout_slug))
}

struct ConsolidatedOutput {
    memory_md: String,
    memory_summary: String,
    skills: Vec<ConsolidationSkill>,
}

struct ConsolidationSkill {
    name: String,
    content: String,
}

/// Parse Stage 2 consolidation JSON output.
fn parse_consolidation_output(text: &str) -> Option<ConsolidatedOutput> {
    let json_str = text
        .strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```\n"))
        .and_then(|s| s.strip_suffix("\n```"))
        .unwrap_or(text);

    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let memory_md = v.get("memory_md")?.as_str()?.to_string();
    let memory_summary = v.get("memory_summary")?.as_str()?.to_string();

    let skills = v
        .get("skills")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|skill| {
                    Some(ConsolidationSkill {
                        name: skill.get("name")?.as_str()?.to_string(),
                        content: skill.get("content")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(ConsolidatedOutput {
        memory_md,
        memory_summary,
        skills,
    })
}

/// Atomically write a file (temp + rename pattern).
fn write_artifact(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, content).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Owned `MemoryDb` connection helper. Used by `services` to spawn
/// the worker without exposing the raw `Connection`.
///
/// `pipeline.db` is a rebuildable job-queue cache — on network filesystems
/// we use the per-host sibling (`pipeline.h-<host>.db`) so old binaries
/// that would flip a shared DB back to WAL cannot corrupt the no-WAL
/// invariant. Each host starts fresh; the pipeline rebuilds state from
/// durable stores on next run.
pub fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let mode = oxicode_mnemopi::journal::JournalMode::for_db_path(path);
    let effective = mode.per_host_db_path(path);
    let conn = Connection::open(&effective)?;
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = {};
         PRAGMA busy_timeout = {};
         PRAGMA foreign_keys = ON;",
        mode.as_str(),
        mode.busy_timeout_ms()
    ))?;
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
