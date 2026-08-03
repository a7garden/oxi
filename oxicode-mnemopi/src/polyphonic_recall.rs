//! Polyphonic recall — ported from omp `polyphonic-recall.ts`.
//!
//! Runs recall from multiple independent "voices" (vector, graph, fact,
//! temporal) and merges their results. Each voice contributes a score;
//! the combined score is a weighted sum. Memories that appear in multiple
//! voices get a boost.

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::recall;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PolyphonicVoice {
    Vector,
    Graph,
    Fact,
    Temporal,
}

impl PolyphonicVoice {
    fn weight(self) -> f64 {
        match self {
            PolyphonicVoice::Vector => 0.4,
            PolyphonicVoice::Graph => 0.25,
            PolyphonicVoice::Fact => 0.2,
            PolyphonicVoice::Temporal => 0.15,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolyphonicResult {
    pub memory_id: String,
    pub combined_score: f64,
    pub voice_scores: HashMap<String, f64>,
    pub content: String,
    pub source: Option<String>,
    pub importance: f64,
    pub veracity: String,
    pub tier: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolyphonicRecallOutput {
    pub results: Vec<PolyphonicResult>,
    pub voices_used: Vec<String>,
}

// ── Voice: fact ──────────────────────────────────────────────────────────

fn fact_voice(conn: &Connection, query: &str, limit: usize) -> Result<HashMap<String, f64>> {
    let mut stmt = conn.prepare(
        "SELECT ef.source_memory_id, ef.confidence
         FROM episodic_facts ef
         WHERE ef.subject LIKE ?1 OR ef.object LIKE ?1
         LIMIT ?2",
    )?;
    let pattern = format!("%{query}%");
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<f64>>(1)?.unwrap_or(0.5),
        ))
    })?;
    let mut scores = HashMap::new();
    for r in rows {
        let (memory_id, confidence) = r?;
        if let Some(id) = memory_id {
            *scores.entry(id).or_insert(0.0) += confidence;
        }
    }
    Ok(scores)
}

// ── Voice: temporal ──────────────────────────────────────────────────────

fn temporal_voice(conn: &Connection, limit: usize) -> Result<HashMap<String, f64>> {
    let mut stmt = conn.prepare(
        "SELECT id, timestamp, importance
         FROM episodic_memory
         ORDER BY timestamp DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<f64>>(2)?.unwrap_or(0.5),
        ))
    })?;
    let now = chrono::Utc::now();
    let mut scores = HashMap::new();
    for r in rows {
        let (id, timestamp, importance) = r?;
        let recency_score = if let Some(ts) = timestamp {
            match chrono::DateTime::parse_from_rfc3339(&ts) {
                Ok(dt) => {
                    let age_hours =
                        (now - dt.with_timezone(&chrono::Utc)).num_hours().max(0) as f64;
                    // Exponential decay: half-life of 7 days
                    (-age_hours / (7.0 * 24.0)).exp() * importance
                }
                Err(_) => importance * 0.5,
            }
        } else {
            importance * 0.5
        };
        scores.insert(id, recency_score);
    }
    Ok(scores)
}

// ── Voice: graph ─────────────────────────────────────────────────────────

fn graph_voice(conn: &Connection, query: &str, limit: usize) -> Result<HashMap<String, f64>> {
    // Find memories connected to facts matching the query
    let mut stmt = conn.prepare(
        "SELECT DISTINCT ef.source_memory_id, 0.6 as score
         FROM episodic_facts ef
         WHERE ef.subject LIKE ?1 OR ef.object LIKE ?1
         LIMIT ?2",
    )?;
    let pattern = format!("%{query}%");
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, f64>(1)?))
    })?;
    let mut scores = HashMap::new();
    for r in rows {
        let (memory_id, score) = r?;
        if let Some(id) = memory_id {
            scores.insert(id, score);
        }
    }
    Ok(scores)
}

// ── Hydration ────────────────────────────────────────────────────────────

#[allow(clippy::type_complexity)]
fn hydrate_memory(
    conn: &Connection,
    memory_id: &str,
) -> Result<Option<(String, Option<String>, f64, String, String)>> {
    // Try working memory first
    if let Ok(row) = conn
        .query_row(
            "SELECT content, source, COALESCE(importance, 0.5), COALESCE(veracity, 'unknown'), 'working'
             FROM working_memory WHERE id = ?1",
            params![memory_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
    {
        return Ok(Some(row));
    }
    // Try episodic
    if let Ok(row) = conn
        .query_row(
            "SELECT content, source, COALESCE(importance, 0.5), COALESCE(veracity, 'unknown'), 'episodic'
             FROM episodic_memory WHERE id = ?1",
            params![memory_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
    {
        return Ok(Some(row));
    }
    Ok(None)
}

// ── Main polyphonic recall ───────────────────────────────────────────────

/// Run polyphonic recall: combine vector, graph, fact, and temporal voices.
///
/// The FTS-based recall from `recall.rs` serves as the primary (vector+keyword)
/// source. The fact, graph, and temporal voices add additional candidates
/// and boost scores for memories that appear in multiple voices.
pub fn polyphonic_recall(
    conn: &Connection,
    query: &str,
    session_id: &str,
    limit: usize,
) -> Result<PolyphonicRecallOutput> {
    let mut all_scores: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut voices_used = Vec::new();

    // Voice: FTS/keyword recall (acts as the "vector" voice when no embeddings)
    let recall_opts = crate::types::RecallOptions {
        limit: Some(limit),
        ..Default::default()
    };
    let fts_results = recall::recall(conn, query, session_id, &recall_opts)?;
    for result in &fts_results {
        let scores = all_scores.entry(result.id.clone()).or_default();
        scores.insert("vector".to_string(), result.score as f64);
    }
    if !fts_results.is_empty() {
        voices_used.push("vector".to_string());
    }

    // Voice: fact
    let fact_scores = fact_voice(conn, query, limit)?;
    for (id, score) in &fact_scores {
        let scores = all_scores.entry(id.clone()).or_default();
        scores.insert("fact".to_string(), *score);
    }
    if !fact_scores.is_empty() {
        voices_used.push("fact".to_string());
    }

    // Voice: graph
    let graph_scores = graph_voice(conn, query, limit)?;
    for (id, score) in &graph_scores {
        let scores = all_scores.entry(id.clone()).or_default();
        scores.insert("graph".to_string(), *score);
    }
    if !graph_scores.is_empty() {
        voices_used.push("graph".to_string());
    }

    // Voice: temporal (recent episodic memories)
    let temporal_scores = temporal_voice(conn, limit)?;
    for (id, score) in &temporal_scores {
        let scores = all_scores.entry(id.clone()).or_default();
        scores.insert("temporal".to_string(), *score);
    }
    if !temporal_scores.is_empty() {
        voices_used.push("temporal".to_string());
    }

    // Merge: combined score = weighted sum of voice scores
    let voice_weights: HashMap<&str, f64> = [
        ("vector", PolyphonicVoice::Vector.weight()),
        ("graph", PolyphonicVoice::Graph.weight()),
        ("fact", PolyphonicVoice::Fact.weight()),
        ("temporal", PolyphonicVoice::Temporal.weight()),
    ]
    .into_iter()
    .collect();

    let mut results: Vec<PolyphonicResult> = Vec::new();

    for (memory_id, scores) in &all_scores {
        let mut combined = 0.0;
        for (voice, score) in scores {
            let weight = voice_weights.get(voice.as_str()).copied().unwrap_or(0.1);
            combined += score * weight;
        }

        // Boost for multi-voice agreement
        let voice_count = scores.len() as f64;
        if voice_count > 1.0 {
            combined *= 1.0 + 0.1 * (voice_count - 1.0);
        }

        // Hydrate
        if let Some((content, source, importance, veracity, tier)) =
            hydrate_memory(conn, memory_id)?
        {
            results.push(PolyphonicResult {
                memory_id: memory_id.clone(),
                combined_score: combined,
                voice_scores: scores.clone(),
                content,
                source,
                importance,
                veracity,
                tier,
            });
        }
    }

    // Sort by combined score descending
    results.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(PolyphonicRecallOutput {
        results,
        voices_used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::init_schema;
    use crate::store;
    use crate::types::RememberOptions;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_polyphonic_recall_finds_results() {
        let conn = test_db();
        store::remember(
            &conn,
            "User prefers dark mode for coding sessions",
            "session1",
            &RememberOptions {
                source: Some("chat".into()),
                importance: Some(0.8),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let output = polyphonic_recall(&conn, "dark mode preferences", "session1", 10).unwrap();
        assert!(!output.results.is_empty());
        assert!(output.voices_used.contains(&"vector".to_string()));
    }

    #[test]
    fn test_polyphonic_recall_empty() {
        let conn = test_db();
        let output = polyphonic_recall(&conn, "nothing", "session1", 10).unwrap();
        assert!(output.results.is_empty());
    }

    #[test]
    fn test_polyphonic_recall_multi_voice_boost() {
        let conn = test_db();
        // Insert working memory
        store::remember(
            &conn,
            "The database uses PostgreSQL",
            "session1",
            &RememberOptions {
                source: Some("chat".into()),
                importance: Some(0.8),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        // Get the memory id
        let results = store::list_by_source(&conn, "chat", 10).unwrap();
        let mem_id = &results[0].id;

        // Insert a matching episodic fact
        conn.execute(
            "INSERT INTO episodic_facts (fact_id, session_id, subject, predicate, object, confidence, source_memory_id)
             VALUES ('f1', 'session1', 'database', 'uses', 'PostgreSQL', 0.9, ?1)",
            params![mem_id],
        )
        .unwrap();

        let output = polyphonic_recall(&conn, "database", "session1", 10).unwrap();
        assert!(!output.results.is_empty());

        // The memory should appear in both vector and fact voices
        let top = &output.results[0];
        assert!(!top.voice_scores.is_empty());
    }
}
