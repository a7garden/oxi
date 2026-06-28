//! Sleep / consolidation — ported from omp `beam/consolidate.ts`.
//!
//! The sleep function takes old working memories, groups them by source,
//! compresses each group into an episodic episode, and degrades old episodic
//! tiers. Tier degradation:
//!   tier 1 → 2 after 30 days (truncate to 800 chars)
//!   tier 2 → 3 after 180 days (extract key signal, 300 chars)

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::extraction::heuristic_extract;

// ── Config ───────────────────────────────────────────────────────────────

const DEFAULT_TIER2_DAYS: i64 = 30;
const DEFAULT_TIER3_DAYS: i64 = 180;
const DEGRADE_BATCH: i64 = 100;
const TIER3_MAX_CHARS: usize = 300;
const TIER2_MAX_CHARS: usize = 800;
const DEFAULT_MAX_EPISODE_CHARS: usize = 100_000;
const SLEEP_SEPARATOR: &str = " | ";
const TRUNCATION_MARKER: &str =
    "\n[... sleep_consolidation episode truncated by maxEpisodeChars ...]";

// ── Veracity weights for episodic consolidation ──────────────────────────

fn episodic_veracity_weight(v: &str) -> f64 {
    match v {
        "true" => 1.0,
        "stated" => 0.9,
        "unknown" => 0.8,
        "inferred" => 0.7,
        "imported" => 0.6,
        "tool" => 0.5,
        "false" => 0.0,
        _ => 0.8,
    }
}

/// Aggregate veracity from multiple source rows — majority vote with
/// lowest-weight-wins tie-breaking (mirrors omp `aggregateEpisodicVeracity`).
pub fn aggregate_episodic_veracity(veracities: &[&str]) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &v in veracities {
        if v == "unknown" {
            continue;
        }
        *counts.entry(v).or_default() += 1;
    }
    if counts.is_empty() {
        return "unknown".to_string();
    }
    let max = counts.values().copied().max().unwrap_or(0);
    let mut winner: Option<&&str> = None;
    for (v, &count) in counts.iter() {
        if count != max {
            continue;
        }
        if winner.is_none()
            || episodic_veracity_weight(v) < episodic_veracity_weight(winner.unwrap())
        {
            winner = Some(v);
        }
    }
    winner.copied().unwrap_or("unknown").to_string()
}

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SleepResult {
    pub dry_run: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub items_consolidated: usize,
    pub summaries_created: usize,
    pub llm_used: usize,
    pub method: String,
    pub consolidated_ids: Vec<String>,
    pub degradation: DegradeResult,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DegradeResult {
    pub status: String,
    pub tier1_to_tier2: usize,
    pub tier2_to_tier3: usize,
}

// ── Sleep chunking ───────────────────────────────────────────────────────

struct WmRow {
    id: String,
    content: String,
    source: String,
    #[allow(dead_code)]
    timestamp: String,
    #[allow(dead_code)]
    importance: f64,
    scope: String,
    valid_until: Option<String>,
    veracity: String,
}

struct SleepChunk {
    items: Vec<WmRow>,
    original_chars: usize,
}

fn split_sleep_items(items: &[WmRow], source: &str, max_chars: usize) -> Vec<SleepChunk> {
    let prefix_chars = format!("[{source}] ").len();
    let joined_limit = max_chars.saturating_sub(prefix_chars);
    let mut chunks = Vec::new();
    let mut current: Vec<WmRow> = Vec::new();
    let mut current_chars = 0usize;

    for item in items {
        let content_chars = item.content.len();
        let sep_chars = if current.is_empty() {
            0
        } else {
            SLEEP_SEPARATOR.len()
        };
        if !current.is_empty() && current_chars + sep_chars + content_chars > joined_limit {
            chunks.push(SleepChunk {
                items: std::mem::take(&mut current),
                original_chars: current_chars,
            });
            current_chars = 0;
        }
        let sep = if current.is_empty() {
            0
        } else {
            SLEEP_SEPARATOR.len()
        };
        current_chars += sep + content_chars;
        current.push(WmRow {
            id: item.id.clone(),
            content: item.content.clone(),
            source: item.source.clone(),
            timestamp: item.timestamp.clone(),
            importance: item.importance,
            scope: item.scope.clone(),
            valid_until: item.valid_until.clone(),
            veracity: item.veracity.clone(),
        });
    }
    if !current.is_empty() {
        chunks.push(SleepChunk {
            items: current,
            original_chars: current_chars,
        });
    }
    chunks
}

fn build_sleep_summary(source: &str, chunk: &SleepChunk, max_chars: usize) -> (String, bool) {
    let prefix = format!("[{source}] ");
    let joined: Vec<&str> = chunk.items.iter().map(|i| i.content.as_str()).collect();
    let joined_str = joined.join(SLEEP_SEPARATOR);
    let encoded = crate::aaak::encode(&joined_str);
    let uncapped = format!("{prefix}{encoded}");
    if uncapped.len() > max_chars {
        let body_chars = max_chars.saturating_sub(TRUNCATION_MARKER.len());
        let truncated = format!(
            "{}{}",
            &uncapped[..body_chars.min(uncapped.len())],
            TRUNCATION_MARKER
        );
        (truncated, true)
    } else {
        (uncapped, false)
    }
}

// ── Consolidation ────────────────────────────────────────────────────────

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn cutoff_iso_days(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
}

fn cutoff_iso_hours(hours: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339()
}

/// Insert a consolidated episode into `episodic_memory`.
#[allow(clippy::too_many_arguments)]
pub fn consolidate_to_episodic(
    conn: &Connection,
    summary: &str,
    source_wm_ids: &[String],
    source: &str,
    importance: f64,
    session_id: &str,
    scope: &str,
    valid_until: Option<&str>,
    veracity: &str,
    metadata_json: &str,
) -> Result<String> {
    let memory_id = Uuid::new_v4().simple().to_string();
    let timestamp = now_iso();

    conn.execute(
        "INSERT INTO episodic_memory
         (id, content, source, timestamp, session_id, importance, metadata_json,
          summary_of, valid_until, scope, veracity, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            memory_id,
            summary,
            source,
            timestamp,
            session_id,
            importance,
            metadata_json,
            source_wm_ids.join(","),
            valid_until,
            scope,
            veracity,
            timestamp,
        ],
    )?;

    // Extract facts from the summary using heuristic extractor
    let facts = heuristic_extract(summary);
    for fact in &facts {
        conn.execute(
            "INSERT OR IGNORE INTO episodic_facts
             (fact_id, session_id, subject, predicate, object, timestamp, source_memory_id, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Uuid::new_v4().simple().to_string(),
                session_id,
                fact.content,  // subject (simplified — omp uses key/value)
                fact.memory_type.as_deref().unwrap_or("unknown"),
                fact.content,
                timestamp,
                memory_id,
                fact.importance,
            ],
        )?;
    }

    Ok(memory_id)
}

// ── Eligible rows ────────────────────────────────────────────────────────

fn eligible_working_rows(
    conn: &Connection,
    session_id: &str,
    ttl_hours: i64,
) -> Result<Vec<WmRow>> {
    let cutoff = cutoff_iso_hours(ttl_hours / 2);
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, importance, scope, valid_until, veracity
         FROM working_memory
         WHERE COALESCE(session_id, 'default') = ?1
           AND timestamp < ?2
           AND consolidated_at IS NULL
         ORDER BY timestamp ASC LIMIT 5000",
    )?;
    let rows = stmt.query_map(params![session_id, cutoff], |row| {
        Ok(WmRow {
            id: row.get(0)?,
            content: row.get(1)?,
            source: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            timestamp: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            importance: row.get::<_, Option<f64>>(4)?.unwrap_or(0.5),
            scope: row
                .get::<_, Option<String>>(5)?
                .unwrap_or_else(|| "global".to_string()),
            valid_until: row.get(6)?,
            veracity: row
                .get::<_, Option<String>>(7)?
                .unwrap_or_else(|| "unknown".to_string()),
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// ── Tier degradation ─────────────────────────────────────────────────────

fn extract_key_signal(content: &str, max_chars: usize) -> String {
    let sentences: Vec<&str> = content
        .split(['.', '!', '?'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if sentences.is_empty() {
        return content.chars().take(max_chars).collect();
    }

    let scored: Vec<(usize, f64)> = sentences
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let caps = s
                .matches(|c: char| c.is_uppercase() && c.is_alphabetic())
                .count() as f64
                * 2.0;
            let keywords = [
                "prefer",
                "always",
                "never",
                "deadline",
                "release",
                "version",
                "decided",
                "important",
                "must",
                "should",
            ];
            let kw = keywords
                .iter()
                .filter(|kw| s.to_lowercase().contains(*kw))
                .count() as f64;
            (idx, caps + kw)
        })
        .collect();

    let mut sorted = scored.clone();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut selected: Vec<(usize, &str)> = Vec::new();
    let mut used = 0usize;
    for &(idx, _) in &sorted {
        let next = sentences[idx].trim();
        if used + next.len() + 1 > max_chars && !selected.is_empty() {
            continue;
        }
        selected.push((idx, sentences[idx]));
        used += next.len() + 1;
        if used >= max_chars {
            break;
        }
    }
    selected.sort_by_key(|(idx, _)| *idx);
    let text: Vec<&str> = selected.iter().map(|(_, s)| s.trim()).collect();
    let joined = text.join(" ");
    if joined.len() <= max_chars {
        joined
    } else {
        let cut = max_chars.saturating_sub(6);
        format!("{} [...]", &joined[..cut.min(joined.len())])
    }
}

/// Degrade episodic memories through tiers (1→2→3) based on age.
pub fn degrade_episodic(conn: &Connection, dry_run: bool) -> Result<DegradeResult> {
    let tier2_cutoff = cutoff_iso_days(DEFAULT_TIER2_DAYS);
    let tier3_cutoff = cutoff_iso_days(DEFAULT_TIER3_DAYS);

    // tier 1 → 2
    let tier1_ids: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, content FROM episodic_memory
             WHERE tier = 1 AND created_at < ?1 ORDER BY created_at ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![tier2_cutoff, DEGRADE_BATCH], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        v
    };

    // tier 2 → 3
    let tier2_ids: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, content FROM episodic_memory
             WHERE tier = 2 AND created_at < ?1 ORDER BY created_at ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![tier3_cutoff, DEGRADE_BATCH / 2], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        v
    };

    let tier1_count = tier1_ids.len();
    let tier2_count = tier2_ids.len();

    if dry_run {
        return Ok(DegradeResult {
            status: "dry_run".to_string(),
            tier1_to_tier2: tier1_count,
            tier2_to_tier3: tier2_count,
        });
    }

    let now = now_iso();
    for (id, content) in &tier1_ids {
        let compressed: String = content.chars().take(TIER2_MAX_CHARS).collect();
        conn.execute(
            "UPDATE episodic_memory SET content = ?1, tier = 2, degraded_at = ?2 WHERE id = ?3",
            params![compressed, now, id],
        )?;
        if compressed != *content {
            conn.execute(
                "DELETE FROM memory_embeddings WHERE memory_id = ?1",
                params![id],
            )?;
        }
    }

    for (id, content) in &tier2_ids {
        let compressed = if content.len() > TIER3_MAX_CHARS {
            extract_key_signal(content, TIER3_MAX_CHARS)
        } else {
            content.clone()
        };
        conn.execute(
            "UPDATE episodic_memory SET content = ?1, tier = 3, degraded_at = ?2 WHERE id = ?3",
            params![compressed, now, id],
        )?;
        if compressed != *content {
            conn.execute(
                "DELETE FROM memory_embeddings WHERE memory_id = ?1",
                params![id],
            )?;
        }
    }

    Ok(DegradeResult {
        status: "degraded".to_string(),
        tier1_to_tier2: tier1_count,
        tier2_to_tier3: tier2_count,
    })
}

// ── Main sleep function ──────────────────────────────────────────────────

/// Run the sleep/consolidation cycle.
///
/// 1. Find working memories older than TTL/2 that haven't been consolidated.
/// 2. Claim them (set `consolidated_at`).
/// 3. Group by source, split into chunks, compress to episodic episodes.
/// 4. Degrade old episodic tiers.
pub fn sleep(
    conn: &Connection,
    session_id: &str,
    ttl_hours: i64,
    max_episode_chars: Option<usize>,
    dry_run: bool,
) -> Result<SleepResult> {
    let max_chars = max_episode_chars.unwrap_or(DEFAULT_MAX_EPISODE_CHARS);

    let mut rows = eligible_working_rows(conn, session_id, ttl_hours)?;
    if rows.is_empty() {
        return Ok(SleepResult {
            dry_run,
            status: "no_op".to_string(),
            message: Some("No old working memories to consolidate".to_string()),
            items_consolidated: 0,
            summaries_created: 0,
            llm_used: 0,
            method: "aaak".to_string(),
            consolidated_ids: Vec::new(),
            degradation: degrade_episodic(conn, dry_run)?,
        });
    }

    // Claim rows (set consolidated_at)
    if !dry_run {
        let claim_ts = now_iso();
        for row in &rows {
            conn.execute(
                "UPDATE working_memory SET consolidated_at = ?1
                 WHERE id = ?2 AND consolidated_at IS NULL",
                params![claim_ts, row.id],
            )?;
        }
    }

    // Group by source
    let mut grouped: HashMap<String, Vec<WmRow>> = HashMap::new();
    for row in rows.drain(..) {
        grouped.entry(row.source.clone()).or_default().push(row);
    }

    let mut consolidated_ids = Vec::new();
    let mut summaries_created = 0;

    for (source, items) in &grouped {
        for chunk in split_sleep_items(items, source, max_chars) {
            let ids: Vec<String> = chunk.items.iter().map(|i| i.id.clone()).collect();

            // Determine scope and valid_until from items
            let scope = chunk
                .items
                .iter()
                .find(|i| i.scope == "global")
                .map(|_| "global")
                .unwrap_or("session");
            let valid_until = chunk
                .items
                .iter()
                .filter_map(|i| i.valid_until.as_deref())
                .min();

            let veracities: Vec<&str> = chunk.items.iter().map(|i| i.veracity.as_str()).collect();
            let veracity = aggregate_episodic_veracity(&veracities);

            let (summary, truncated) = build_sleep_summary(source, &chunk, max_chars);
            let mut metadata = serde_json::Map::new();
            metadata.insert("original_count".into(), chunk.items.len().into());
            metadata.insert("source".into(), source.clone().into());
            metadata.insert("llm_used".into(), false.into());
            if truncated {
                metadata.insert("truncated".into(), true.into());
                metadata.insert("original_chars".into(), chunk.original_chars.into());
                metadata.insert("max_chars".into(), max_chars.into());
            }
            let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();

            if !dry_run {
                let _ = consolidate_to_episodic(
                    conn,
                    &summary,
                    &ids,
                    "sleep_consolidation",
                    0.6,
                    session_id,
                    scope,
                    valid_until,
                    &veracity,
                    &metadata_json,
                )?;
            }
            consolidated_ids.extend(ids);
            summaries_created += 1;
        }
    }

    // Log consolidation
    if !dry_run {
        conn.execute(
            "INSERT INTO consolidation_log (session_id, items_consolidated, summary_preview, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session_id,
                consolidated_ids.len() as i64,
                format!("{summaries_created} summaries from {} items", consolidated_ids.len()),
                now_iso(),
            ],
        )?;
    }

    let degradation = degrade_episodic(conn, dry_run)?;

    Ok(SleepResult {
        dry_run,
        status: if dry_run { "dry_run" } else { "consolidated" }.to_string(),
        message: None,
        items_consolidated: consolidated_ids.len(),
        summaries_created,
        llm_used: 0,
        method: "aaak".to_string(),
        consolidated_ids,
        degradation,
    })
}

/// Get the consolidation log.
#[allow(clippy::type_complexity)]
pub fn get_consolidation_log(
    conn: &Connection,
    session_id: &str,
    limit: i64,
) -> Result<Vec<(i64, String, i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, items_consolidated, summary_preview, created_at
         FROM consolidation_log WHERE session_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![session_id, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::init_schema;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_sleep_no_op() {
        let conn = test_db();
        let result = sleep(&conn, "default", 24, None, false).unwrap();
        assert_eq!(result.status, "no_op");
    }

    #[test]
    fn test_sleep_consolidates() {
        let conn = test_db();

        // Insert old working memories
        for i in 0..5 {
            let id = format!("wm-test-{i}");
            let old_ts = (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
            conn.execute(
                "INSERT INTO working_memory (id, content, source, timestamp, session_id, importance, scope, veracity)
                 VALUES (?1, ?2, ?3, ?4, 'default', 0.7, 'session', 'stated')",
                params![id, format!("Memory item {i}"), "test", old_ts],
            ).unwrap();
        }

        let result = sleep(&conn, "default", 24, None, false).unwrap();
        assert_eq!(result.status, "consolidated");
        assert_eq!(result.items_consolidated, 5);
        assert!(result.summaries_created > 0);
        assert_eq!(result.method, "aaak");

        // Verify episodic memory was created
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM episodic_memory", [], |row| row.get(0))
            .unwrap_or(0);
        assert!(count > 0);

        // Verify working memory was claimed
        let claimed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM working_memory WHERE consolidated_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        assert_eq!(claimed, 5);
    }

    #[test]
    fn test_aggregate_veracity() {
        assert_eq!(
            aggregate_episodic_veracity(&["stated", "true", "unknown"]),
            "stated"
        );
        assert_eq!(aggregate_episodic_veracity(&["unknown"]), "unknown");
        assert_eq!(aggregate_episodic_veracity(&[]), "unknown");
    }

    #[test]
    fn test_extract_key_signal() {
        let content =
            "The user prefers dark mode. Important: deadline is Friday. Run the tests now.";
        let signal = extract_key_signal(content, 50);
        assert!(signal.len() <= 56); // 50 + " [...]"
    }

    #[test]
    fn test_degrade_episodic_dry_run() {
        let conn = test_db();
        // Insert a tier-1 episodic memory with old timestamp
        let old_ts = (chrono::Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        conn.execute(
            "INSERT INTO episodic_memory (id, content, source, timestamp, session_id, importance, tier, created_at)
             VALUES ('ep1', 'Old episode content that is quite long and should be degraded', 'test', ?1, 'default', 0.5, 1, ?1)",
            params![old_ts],
        ).unwrap();

        let result = degrade_episodic(&conn, true).unwrap();
        assert_eq!(result.status, "dry_run");
        assert!(result.tier1_to_tier2 >= 1);
    }

    #[test]
    fn test_sleep_dry_run() {
        let conn = test_db();
        for i in 0..3 {
            let old_ts = (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
            conn.execute(
                "INSERT INTO working_memory (id, content, source, timestamp, session_id, importance, scope, veracity)
                 VALUES (?1, ?2, ?3, ?4, 'default', 0.5, 'session', 'unknown')",
                params![format!("wm-dry-{i}"), format!("Dry run item {i}"), "test", old_ts],
            ).unwrap();
        }

        let result = sleep(&conn, "default", 24, None, true).unwrap();
        assert_eq!(result.status, "dry_run");
        assert_eq!(result.items_consolidated, 3);

        // Verify nothing was actually consolidated
        let ep_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM episodic_memory", [], |row| row.get(0))
            .unwrap_or(0);
        assert_eq!(ep_count, 0);
    }
}
