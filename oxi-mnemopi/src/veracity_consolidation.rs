//! Veracity consolidation — ported from omp `veracity-consolidation.ts`.
//!
//! Merges duplicate facts (same subject+predicate+object) into a
//! `consolidated_facts` table, tracking mention counts, source diversity,
//! and veracity voting. Conflict detection flags facts where the same
//! subject+predicate has contradicting objects.

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ── Veracity weights ─────────────────────────────────────────────────────

/// Weight per veracity level — higher = more trustworthy.
pub const VERACITY_WEIGHTS: &[(&str, f64)] = &[
    ("true", 1.0),
    ("stated", 0.9),
    ("likely_true", 0.8),
    ("unknown", 0.5),
    ("inferred", 0.4),
    ("imported", 0.3),
    ("tool", 0.2),
    ("contested", 0.1),
    ("false", 0.0),
];

pub fn veracity_weight(v: &str) -> f64 {
    VERACITY_WEIGHTS
        .iter()
        .find(|(name, _)| *name == v)
        .map(|(_, w)| *w)
        .unwrap_or(0.5)
}

/// Clamp an unknown veracity string to a known value.
pub fn clamp_veracity(raw: &str) -> String {
    let norm = raw.trim().to_lowercase();
    if norm.is_empty() {
        return "unknown".to_string();
    }
    let valid: Vec<&str> = VERACITY_WEIGHTS.iter().map(|(n, _)| *n).collect();
    if valid.contains(&norm.as_str()) {
        norm
    } else {
        "unknown".to_string()
    }
}

/// Aggregate veracity from multiple sources — majority vote, lowest-weight
/// wins on ties (conservative).
pub fn aggregate_veracity(veracities: &[&str]) -> String {
    if veracities.is_empty() {
        return "unknown".to_string();
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for v in veracities {
        let clamped = clamp_veracity(v);
        *counts.entry(clamped).or_default() += 1;
    }
    let non_unknown: HashMap<String, usize> = counts
        .iter()
        .filter(|(k, _)| k.as_str() != "unknown")
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let candidates = if non_unknown.is_empty() {
        &counts
    } else {
        &non_unknown
    };
    let max = candidates.values().copied().max().unwrap_or(0);
    let mut winner: Option<String> = None;
    for (v, &count) in candidates {
        if count != max {
            continue;
        }
        if winner.is_none()
            || veracity_weight(v) < veracity_weight(winner.as_deref().unwrap_or("unknown"))
        {
            winner = Some(v.clone());
        }
    }
    winner.unwrap_or_else(|| "unknown".to_string())
}

// ── Fact ID ──────────────────────────────────────────────────────────────

/// Compute a deterministic fact ID from subject+predicate+object.
pub fn compute_fact_id(subject: &str, predicate: &str, object: &str) -> String {
    let input = format!("{subject}\0{predicate}\0{object}");
    let hash = <sha2::Sha256 as sha2::Digest>::digest(input.as_bytes());
    format!("{:x}", hash)
}

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedFact {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
    pub mention_count: i64,
    pub sources: Vec<String>,
    pub veracity: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub conflict_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub subject: String,
    pub predicate: String,
    pub objects: Vec<String>,
    pub sources: Vec<String>,
    pub veracities: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsolidationStats {
    pub merged: usize,
    pub created: usize,
    pub conflicts: usize,
    pub conflicts_found: Vec<Conflict>,
}

// ── Consolidator ─────────────────────────────────────────────────────────

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse_sources(raw: Option<&str>) -> Vec<String> {
    match raw {
        Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Consolidate a single fact into `consolidated_facts`.
///
/// If the fact (by subject+predicate+object) already exists, increments
/// mention count, merges sources, and re-votes veracity. Otherwise creates
/// a new entry.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn consolidate_fact(
    conn: &Connection,
    subject: &str,
    predicate: &str,
    object: &str,
    veracity: &str,
    source: &str,
    confidence: f64,
) -> Result<ConsolidatedFact> {
    let fact_id = compute_fact_id(subject, predicate, object);
    let now = now_iso();

    // Check if fact already exists
    let existing: Option<(i64, f64, String, String, Option<String>, Option<String>, i64)> = conn
        .query_row(
            "SELECT mention_count, confidence, sources_json, veracity, first_seen, last_seen, conflict_count
             FROM consolidated_facts WHERE id = ?1",
            params![fact_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .ok();

    if let Some((
        mention_count,
        existing_conf,
        existing_sources_json,
        existing_veracity,
        first_seen,
        _last_seen,
        conflict_count,
    )) = existing
    {
        // Merge — increment mention count, merge sources, re-vote veracity
        let mut sources = parse_sources(Some(&existing_sources_json));
        if !source.is_empty() && !sources.contains(&source.to_string()) {
            sources.push(source.to_string());
        }
        let sources_json = serde_json::to_string(&sources).unwrap_or_default();

        let mut all_veracities = vec![existing_veracity.as_str()];
        if veracity != "unknown" {
            all_veracities.push(veracity);
        }
        let merged_veracity = aggregate_veracity(&all_veracities);

        // Average confidence
        let new_count = mention_count + 1;
        let merged_conf = (existing_conf * mention_count as f64 + confidence) / new_count as f64;

        conn.execute(
            "UPDATE consolidated_facts
             SET mention_count = ?1, confidence = ?2, sources_json = ?3,
                 veracity = ?4, last_seen = ?5
             WHERE id = ?6",
            params![
                new_count,
                merged_conf,
                sources_json,
                merged_veracity,
                now,
                fact_id
            ],
        )?;

        Ok(ConsolidatedFact {
            id: fact_id,
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            confidence: merged_conf,
            mention_count: new_count,
            sources,
            veracity: merged_veracity,
            first_seen,
            last_seen: Some(now),
            conflict_count,
        })
    } else {
        // Create new
        let sources = if source.is_empty() {
            Vec::new()
        } else {
            vec![source.to_string()]
        };
        let sources_json = serde_json::to_string(&sources).unwrap_or_default();
        let clamped_veracity = clamp_veracity(veracity);

        conn.execute(
            "INSERT INTO consolidated_facts
             (id, subject, predicate, object, confidence, mention_count,
              sources_json, veracity, first_seen, last_seen, conflict_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, 0, ?8)",
            params![
                fact_id,
                subject,
                predicate,
                object,
                confidence,
                sources_json,
                clamped_veracity,
                now,
                now
            ],
        )?;

        Ok(ConsolidatedFact {
            id: fact_id,
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            confidence,
            mention_count: 1,
            sources,
            veracity: clamped_veracity,
            first_seen: Some(now.clone()),
            last_seen: Some(now),
            conflict_count: 0,
        })
    }
}

/// Detect conflicts: same subject+predicate but different objects.
pub fn detect_conflicts(conn: &Connection) -> Result<Vec<Conflict>> {
    let mut stmt = conn.prepare(
        "SELECT subject, predicate, GROUP_CONCAT(DISTINCT object) as objects,
                GROUP_CONCAT(DISTINCT sources_json) as sources,
                GROUP_CONCAT(DISTINCT veracity) as veracities
         FROM consolidated_facts
         GROUP BY subject, predicate
         HAVING COUNT(DISTINCT object) > 1
         ORDER BY subject",
    )?;

    let rows = stmt.query_map([], |row| {
        let objects_str: String = row.get(2).unwrap_or_default();
        let sources_str: String = row.get(3).unwrap_or_default();
        let veracities_str: String = row.get(4).unwrap_or_default();
        Ok(Conflict {
            subject: row.get(0)?,
            predicate: row.get(1)?,
            objects: objects_str.split(',').map(String::from).collect(),
            sources: sources_str
                .split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            veracities: veracities_str.split(',').map(String::from).collect(),
        })
    })?;

    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Run full consolidation from `episodic_facts` into `consolidated_facts`.
pub fn run_consolidation(conn: &Connection) -> Result<ConsolidationStats> {
    let mut stmt = conn.prepare(
        "SELECT subject, predicate, object, confidence, source_memory_id
         FROM episodic_facts",
    )?;

    let facts: Vec<(String, String, String, f64, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, Option<f64>>(3)?.unwrap_or(0.5),
                row.get(4)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    drop(stmt);

    let mut stats = ConsolidationStats::default();
    for (subject, predicate, object, confidence, source) in &facts {
        let result = consolidate_fact(
            conn,
            subject,
            predicate,
            object,
            "unknown",
            source.as_deref().unwrap_or("extraction"),
            *confidence,
        )?;
        if result.mention_count > 1 {
            stats.merged += 1;
        } else {
            stats.created += 1;
        }
    }

    stats.conflicts_found = detect_conflicts(conn)?;
    stats.conflicts = stats.conflicts_found.len();

    Ok(stats)
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
    fn test_clamp_veracity() {
        assert_eq!(clamp_veracity("TRUE"), "true");
        assert_eq!(clamp_veracity("stated"), "stated");
        assert_eq!(clamp_veracity("nonsense"), "unknown");
        assert_eq!(clamp_veracity(""), "unknown");
    }

    #[test]
    fn test_aggregate_veracity() {
        assert_eq!(aggregate_veracity(&["true", "stated"]), "stated"); // tie, lowest wins
        assert_eq!(aggregate_veracity(&["true", "true", "stated"]), "true");
        assert_eq!(aggregate_veracity(&["unknown", "unknown"]), "unknown");
        assert_eq!(aggregate_veracity(&[]), "unknown");
    }

    #[test]
    fn test_consolidate_fact_new() {
        let conn = test_db();
        let fact =
            consolidate_fact(&conn, "user", "prefers", "dark mode", "stated", "chat", 0.8).unwrap();
        assert_eq!(fact.mention_count, 1);
        assert_eq!(fact.veracity, "stated");
    }

    #[test]
    fn test_consolidate_fact_merge() {
        let conn = test_db();
        consolidate_fact(&conn, "user", "prefers", "dark mode", "stated", "chat", 0.8).unwrap();
        let fact = consolidate_fact(
            &conn,
            "user",
            "prefers",
            "dark mode",
            "true",
            "session",
            0.9,
        )
        .unwrap();
        assert_eq!(fact.mention_count, 2);
        assert_eq!(fact.sources.len(), 2);
        assert_eq!(fact.veracity, "stated"); // tie between true and stated, stated is lower weight
    }

    #[test]
    fn test_detect_conflicts() {
        let conn = test_db();
        consolidate_fact(&conn, "user", "prefers", "dark mode", "stated", "chat", 0.8).unwrap();
        consolidate_fact(
            &conn,
            "user",
            "prefers",
            "light mode",
            "stated",
            "other",
            0.8,
        )
        .unwrap();

        let conflicts = detect_conflicts(&conn).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].subject, "user");
        assert!(conflicts[0].objects.len() >= 2);
    }

    #[test]
    fn test_compute_fact_id() {
        let id1 = compute_fact_id("user", "prefers", "dark");
        let id2 = compute_fact_id("user", "prefers", "dark");
        let id3 = compute_fact_id("user", "prefers", "light");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
}
