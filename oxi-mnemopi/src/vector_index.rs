//! Exact vector index — ported from omp `beam/helpers.ts` vector search.
//!
//! Loads all embeddings from `memory_embeddings` table, computes cosine
//! similarity against the query vector, and returns top-k matches.
//! This is the "exact" (brute-force) approach — sufficient for thousands
//! of memories. omp uses the same approach.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;
use crate::vector_math::cosine_similarity;

/// A single vector search hit.
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub memory_id: String,
    pub score: f32,
}

/// Search for memories whose embeddings are most similar to `query_vector`.
///
/// Loads all embeddings for `candidate_ids` from the `memory_embeddings`
/// table and computes brute-force cosine similarity. Returns the top-`k`
/// hits sorted by descending similarity.
///
/// Mirrors omp's `vectorSimilarities` + manual sort approach.
pub fn search_exact(
    conn: &Connection,
    query_vector: &[f32],
    candidate_ids: &[String],
    k: usize,
) -> Result<Vec<VectorHit>> {
    if query_vector.is_empty() || candidate_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut similarities: HashMap<String, f32> = HashMap::new();

    for chunk in candidate_ids.chunks(500) {
        let placeholders = (0..chunk.len()).map(|_| "?").collect::<Vec<_>>().join(",");

        let sql = format!(
            "SELECT memory_id, embedding_json FROM memory_embeddings WHERE memory_id IN ({placeholders})"
        );

        let mut stmt = conn.prepare(&sql)?;
        let id_params: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

        let rows = stmt.query_map(id_params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (memory_id, embedding_json) = row?;
            if let Ok(stored_vec) = parse_embedding(&embedding_json) {
                let sim = cosine_similarity(query_vector, &stored_vec);
                if sim > 0.0 {
                    similarities.insert(memory_id, sim);
                }
            }
        }
    }

    let mut hits: Vec<VectorHit> = similarities
        .into_iter()
        .map(|(memory_id, score)| VectorHit { memory_id, score })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);

    Ok(hits)
}

/// Parse a JSON-encoded embedding vector.
fn parse_embedding(json: &str) -> std::result::Result<Vec<f32>, serde_json::Error> {
    let parsed: Vec<f64> = serde_json::from_str(json)?;
    Ok(parsed.into_iter().map(|f| f as f32).collect())
}

/// Store an embedding for a memory.
pub fn store_embedding(
    conn: &Connection,
    memory_id: &str,
    embedding: &[f32],
    model: &str,
) -> Result<()> {
    let embedding_json = serde_json::to_string(embedding)?;
    conn.execute(
        "INSERT OR REPLACE INTO memory_embeddings (memory_id, embedding_json, model)
         VALUES (?1, ?2, ?3)",
        params![memory_id, embedding_json, model],
    )?;
    Ok(())
}

/// Get the embedding for a single memory.
pub fn get_embedding(conn: &Connection, memory_id: &str) -> Result<Option<Vec<f32>>> {
    let mut stmt =
        conn.prepare("SELECT embedding_json FROM memory_embeddings WHERE memory_id = ?1")?;

    let result = stmt
        .query_row(params![memory_id], |row| row.get::<_, String>(0))
        .optional()?;

    match result {
        Some(json) => Ok(Some(parse_embedding(&json)?)),
        None => Ok(None),
    }
}

/// Delete an embedding for a memory.
pub fn delete_embedding(conn: &Connection, memory_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM memory_embeddings WHERE memory_id = ?1",
        params![memory_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn store_and_search() {
        let conn = setup_db();

        let id = crate::store::remember(
            &conn,
            "test memory",
            "default",
            &crate::types::RememberOptions::default(),
        )
        .unwrap();

        let embedding = vec![0.1, 0.2, 0.3, 0.4];
        store_embedding(&conn, &id, &embedding, "test-model").unwrap();

        let query = vec![0.1, 0.2, 0.3, 0.5];
        let hits = search_exact(&conn, &query, std::slice::from_ref(&id), 10).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory_id, id);
        assert!(hits[0].score > 0.9);
    }

    #[test]
    fn search_empty_query() {
        let conn = setup_db();
        let hits = search_exact(&conn, &[], &["id1".to_string()], 10).unwrap();
        assert!(hits.is_empty());
    }
}
