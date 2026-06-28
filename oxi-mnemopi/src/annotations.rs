//! Annotations — ported from omp `annotations.ts`.
//!
//! Stores metadata annotations on memories: mentions (entities found in
//! content), facts (structured claims), occurred_on (temporal), has_source
//! (provenance). Annotations power faceted search and entity linking.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ── Types ────────────────────────────────────────────────────────────────

pub const ANNOTATION_KINDS: &[&str] = &["mentions", "fact", "occurred_on", "has_source"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationRow {
    pub id: i64,
    pub memory_id: String,
    pub kind: String,
    pub value: String,
    pub source: String,
    pub confidence: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnnotationInput {
    pub memory_id: String,
    pub kind: String,
    pub value: String,
    pub source: Option<String>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnnotationImportStats {
    pub inserted: usize,
    pub skipped: usize,
}

/// Query options for annotations.
#[derive(Debug, Clone, Default)]
pub struct AnnotationQuery {
    pub memory_id: Option<String>,
    pub kind: Option<String>,
    pub value: Option<String>,
}

// ── Stop words for filtering noisy mentions ─────────────────────────────

const ENTITY_STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "must", "shall",
    "can", "need", "this", "that", "these", "those", "i", "you", "he", "she", "it", "we", "they",
    "what", "which", "who", "when", "where", "why", "how", "all", "each", "every", "some", "any",
    "no", "not", "as", "at", "by", "for", "with", "about", "against", "between", "into", "through",
    "during", "before", "after", "above", "below", "to", "from", "up", "down", "in", "out", "on",
    "off", "over", "under", "again", "further", "then", "once",
];

fn is_noisy_mention(value: &str) -> bool {
    let lower = value.trim().to_lowercase();
    if lower.len() < 3 {
        return true;
    }
    ENTITY_STOP_WORDS.contains(&lower.as_str())
}

// ── Store operations ─────────────────────────────────────────────────────

/// Add an annotation. Returns the annotation ID.
pub fn add_annotation(conn: &Connection, input: &AnnotationInput) -> Result<i64> {
    // Filter noisy mentions
    if input.kind == "mentions" && is_noisy_mention(&input.value) {
        return Ok(0); // skipped
    }

    conn.execute(
        "INSERT INTO memory_annotations (memory_id, kind, value, source, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            input.memory_id,
            input.kind,
            input.value,
            input.source.as_deref().unwrap_or(""),
            input.confidence.unwrap_or(1.0),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Bulk add annotations, filtering noisy mentions.
pub fn add_annotations_bulk(
    conn: &Connection,
    inputs: &[AnnotationInput],
) -> Result<AnnotationImportStats> {
    let mut stats = AnnotationImportStats::default();
    for input in inputs {
        let id = add_annotation(conn, input)?;
        if id > 0 {
            stats.inserted += 1;
        } else {
            stats.skipped += 1;
        }
    }
    Ok(stats)
}

/// Query annotations with optional filters.
pub fn query_annotations(conn: &Connection, query: &AnnotationQuery) -> Result<Vec<AnnotationRow>> {
    let mut sql = String::from(
        "SELECT id, memory_id, kind, value, source, confidence, created_at
         FROM memory_annotations WHERE 1=1",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ref memory_id) = query.memory_id {
        sql.push_str(" AND memory_id = ?");
        params_vec.push(Box::new(memory_id.clone()));
    }
    if let Some(ref kind) = query.kind {
        sql.push_str(" AND kind = ?");
        params_vec.push(Box::new(kind.clone()));
    }
    if let Some(ref value) = query.value {
        sql.push_str(" AND value LIKE ?");
        params_vec.push(Box::new(format!("%{value}%")));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT 500");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(AnnotationRow {
            id: row.get(0)?,
            memory_id: row.get(1)?,
            kind: row.get(2)?,
            value: row.get(3)?,
            source: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            confidence: row.get(5)?,
            created_at: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    })?;

    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Delete all annotations for a memory.
pub fn delete_for_memory(conn: &Connection, memory_id: &str) -> Result<usize> {
    let count = conn.execute(
        "DELETE FROM memory_annotations WHERE memory_id = ?1",
        params![memory_id],
    )?;
    Ok(count)
}

/// Count annotations by kind.
pub fn count_by_kind(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*) as count FROM memory_annotations GROUP BY kind ORDER BY count DESC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Filter out noisy mentions from a list of values.
pub fn filter_clean_mentions(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|v| !is_noisy_mention(v))
        .cloned()
        .collect()
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
    fn test_add_and_query_annotation() {
        let conn = test_db();
        let id = add_annotation(
            &conn,
            &AnnotationInput {
                memory_id: "mem-1".into(),
                kind: "fact".into(),
                value: "User prefers dark mode".into(),
                source: Some("chat".into()),
                confidence: Some(0.9),
            },
        )
        .unwrap();
        assert!(id > 0);

        let results = query_annotations(
            &conn,
            &AnnotationQuery {
                memory_id: Some("mem-1".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "User prefers dark mode");
    }

    #[test]
    fn test_filter_noisy_mentions() {
        let conn = test_db();
        // "the" should be filtered
        let id = add_annotation(
            &conn,
            &AnnotationInput {
                memory_id: "mem-1".into(),
                kind: "mentions".into(),
                value: "the".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(id, 0); // skipped

        // Real entity should pass
        let id = add_annotation(
            &conn,
            &AnnotationInput {
                memory_id: "mem-1".into(),
                kind: "mentions".into(),
                value: "PostgreSQL".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_delete_for_memory() {
        let conn = test_db();
        add_annotation(
            &conn,
            &AnnotationInput {
                memory_id: "mem-1".into(),
                kind: "fact".into(),
                value: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        add_annotation(
            &conn,
            &AnnotationInput {
                memory_id: "mem-1".into(),
                kind: "mentions".into(),
                value: "PostgreSQL".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let deleted = delete_for_memory(&conn, "mem-1").unwrap();
        assert_eq!(deleted, 2);
    }
}
