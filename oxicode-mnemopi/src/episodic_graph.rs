//! Episodic graph — ported from omp `episodic-graph.ts`.
//!
//! Stores gists (text summaries), facts (SPO triples), and edges
//! (memory-to-memory relationships) for polyphonic recall's graph voice.
//! On consolidation, each episode is ingested: a gist is extracted,
//! entities become facts, and similar memories are linked.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::extraction::heuristic_extract;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gist {
    pub id: String,
    pub text: String,
    pub timestamp: Option<String>,
    pub participants: Vec<String>,
    pub location: Option<String>,
    pub emotion: Option<String>,
    pub time_scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphFact {
    pub fact_id: String,
    pub session_id: Option<String>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub timestamp: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub weight: f64,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedMemory {
    pub memory_id: String,
    pub edge_type: String,
    pub weight: f64,
    pub depth: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphStats {
    pub gists: usize,
    pub facts: usize,
    pub edges: usize,
    pub total_nodes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestResult {
    pub memory_id: String,
    pub fact_count: usize,
    pub edge_count: usize,
}

// ── Ingest ───────────────────────────────────────────────────────────────

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Ingest a memory into the episodic graph.
///
/// Extracts facts (SPO triples) from the content, stores a gist,
/// and optionally links to existing memories with shared entities.
pub fn ingest_memory(
    conn: &Connection,
    memory_id: &str,
    content: &str,
    session_id: &str,
    link_existing: bool,
) -> Result<IngestResult> {
    let timestamp = now_iso();

    // Store gist
    let gist_id = Uuid::new_v4().simple().to_string();
    conn.execute(
        "INSERT INTO episodic_gists (id, text, timestamp, memory_id)
         VALUES (?1, ?2, ?3, ?4)",
        params![gist_id, content, timestamp, memory_id],
    )?;

    // Extract facts
    let extracted = heuristic_extract(content);
    let mut fact_count = 0;

    for fact in &extracted {
        let fact_id = Uuid::new_v4().simple().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO episodic_facts
             (fact_id, session_id, subject, predicate, object, timestamp, source_memory_id, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                fact_id,
                session_id,
                fact.content,
                fact.memory_type.as_deref().unwrap_or("unknown"),
                fact.content,
                timestamp,
                memory_id,
                fact.importance,
            ],
        )?;
        fact_count += 1;
    }

    // Link to existing memories with shared facts
    let mut edge_count = 0;
    if link_existing && fact_count > 0 {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT source_memory_id FROM episodic_facts
             WHERE subject IN (
                 SELECT subject FROM episodic_facts WHERE source_memory_id = ?1
             ) AND source_memory_id != ?1",
        )?;
        let related: Vec<String> = stmt
            .query_map(params![memory_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        for target_id in related {
            conn.execute(
                "INSERT INTO episodic_edges (source, target, edge_type, weight, timestamp)
                 VALUES (?1, ?2, 'shared_entity', 0.5, ?3)",
                params![memory_id, target_id, timestamp],
            )?;
            edge_count += 1;
        }
    }

    Ok(IngestResult {
        memory_id: memory_id.to_string(),
        fact_count,
        edge_count,
    })
}

// ── Query ────────────────────────────────────────────────────────────────

/// Find gists by participant (text contains participant name).
pub fn find_gists_by_participant(conn: &Connection, participant: &str) -> Result<Vec<Gist>> {
    let pattern = format!("%{participant}%");
    let mut stmt = conn.prepare(
        "SELECT id, text, timestamp, participants_json, location, emotion, time_scope
         FROM episodic_gists
         WHERE text LIKE ?1 OR participants_json LIKE ?1
         ORDER BY timestamp DESC LIMIT 50",
    )?;
    let rows = stmt.query_map(params![pattern], |row| {
        let participants_json: String = row.get(3).unwrap_or_else(|_| "[]".to_string());
        let participants: Vec<String> =
            serde_json::from_str(&participants_json).unwrap_or_default();
        Ok(Gist {
            id: row.get(0)?,
            text: row.get(1)?,
            timestamp: row.get(2)?,
            participants,
            location: row.get(4)?,
            emotion: row.get(5)?,
            time_scope: row.get(6)?,
        })
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Find facts by subject.
pub fn find_facts_by_subject(conn: &Connection, subject: &str) -> Result<Vec<GraphFact>> {
    let mut stmt = conn.prepare(
        "SELECT fact_id, session_id, subject, predicate, object, timestamp, confidence
         FROM episodic_facts WHERE subject = ?1 LIMIT 100",
    )?;
    let rows = stmt.query_map(params![subject], |row| {
        Ok(GraphFact {
            fact_id: row.get(0)?,
            session_id: row.get(1)?,
            subject: row.get(2)?,
            predicate: row.get(3)?,
            object: row.get(4)?,
            timestamp: row.get(5)?,
            confidence: row.get(6)?,
        })
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Find memories related to a given memory via BFS over edges.
pub fn find_related_memories(
    conn: &Connection,
    memory_id: &str,
    max_depth: usize,
) -> Result<Vec<RelatedMemory>> {
    let mut visited = std::collections::HashSet::new();
    visited.insert(memory_id.to_string());

    let mut frontier = vec![memory_id.to_string()];
    let mut result = Vec::new();

    for depth in 1..=max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier = Vec::new();
        for node in &frontier {
            let mut stmt = conn.prepare(
                "SELECT target, edge_type, weight FROM episodic_edges WHERE source = ?1
                 UNION
                 SELECT source, edge_type, weight FROM episodic_edges WHERE target = ?1",
            )?;
            let neighbors = stmt.query_map(params![node], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?;
            for n in neighbors {
                let (target, edge_type, weight) = n?;
                if visited.contains(&target) {
                    continue;
                }
                visited.insert(target.clone());
                result.push(RelatedMemory {
                    memory_id: target.clone(),
                    edge_type: edge_type.clone(),
                    weight,
                    depth,
                });
                next_frontier.push(target);
            }
        }
        frontier = next_frontier;
    }

    Ok(result)
}

/// Get graph stats.
pub fn graph_stats(conn: &Connection) -> Result<GraphStats> {
    let gists: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodic_gists", [], |row| row.get(0))
        .unwrap_or(0);
    let facts: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodic_facts", [], |row| row.get(0))
        .unwrap_or(0);
    let edges: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodic_edges", [], |row| row.get(0))
        .unwrap_or(0);
    Ok(GraphStats {
        gists: gists as usize,
        facts: facts as usize,
        edges: edges as usize,
        total_nodes: gists as usize,
    })
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
    fn test_ingest_memory() {
        let conn = test_db();
        let result = ingest_memory(
            &conn,
            "mem-1",
            "User prefers dark mode for coding",
            "session1",
            false,
        )
        .unwrap();
        assert!(result.fact_count > 0);

        let stats = graph_stats(&conn).unwrap();
        assert!(stats.gists > 0);
        assert!(stats.facts > 0);
    }

    #[test]
    fn test_find_facts_by_subject() {
        let conn = test_db();
        ingest_memory(
            &conn,
            "mem-1",
            "User prefers dark mode for coding",
            "session1",
            false,
        )
        .unwrap();
        // The heuristic extractor stores content as subject
        let facts = find_facts_by_subject(&conn, "User prefers dark mode for coding").unwrap();
        assert!(!facts.is_empty());
    }

    #[test]
    fn test_find_related_memories() {
        let conn = test_db();
        // Manually insert edges
        conn.execute(
            "INSERT INTO episodic_edges (source, target, edge_type, weight) VALUES ('a', 'b', 'shared_entity', 0.5)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO episodic_edges (source, target, edge_type, weight) VALUES ('b', 'c', 'shared_entity', 0.3)",
            [],
        ).unwrap();

        let related = find_related_memories(&conn, "a", 3).unwrap();
        assert_eq!(related.len(), 2); // b at depth 1, c at depth 2
    }
}
