//! Triples — ported from omp `triples.ts`.
//!
//! SPO (subject-predicate-object) triple store for the knowledge graph.
//! Supports temporal validity (valid_from / valid_until), confidence,
//! and content-snapshot deduplication.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripleRow {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub source: Option<String>,
    pub confidence: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct TripleWriteOptions {
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub source: Option<String>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct TripleQuery {
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub valid_only: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TripleImportStats {
    pub inserted: usize,
    pub skipped: usize,
}

// ── Store operations ─────────────────────────────────────────────────────

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Add a triple. Returns the triple ID.
pub fn add_triple(
    conn: &Connection,
    subject: &str,
    predicate: &str,
    object: &str,
    opts: &TripleWriteOptions,
) -> Result<i64> {
    // Dedup check: same subject+predicate+object+valid_until (if present)
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM memory_triples
             WHERE subject = ?1 AND predicate = ?2 AND object = ?3
             AND (valid_until IS ?4 OR valid_until = COALESCE(?4, valid_until))
             LIMIT 1",
            params![subject, predicate, object, opts.valid_until.as_deref()],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO memory_triples (subject, predicate, object, valid_from, valid_until, source, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            subject,
            predicate,
            object,
            opts.valid_from.as_deref().unwrap_or(&now_iso()[..10]),
            opts.valid_until.as_deref(),
            opts.source.as_deref().unwrap_or(""),
            opts.confidence.unwrap_or(0.5),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Query triples with optional filters.
pub fn query_triples(conn: &Connection, query: &TripleQuery) -> Result<Vec<TripleRow>> {
    let mut sql = String::from(
        "SELECT id, subject, predicate, object, valid_from, valid_until, source, confidence, created_at
         FROM memory_triples WHERE 1=1",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ref subject) = query.subject {
        sql.push_str(" AND subject = ?");
        params_vec.push(Box::new(subject.clone()));
    }
    if let Some(ref predicate) = query.predicate {
        sql.push_str(" AND predicate = ?");
        params_vec.push(Box::new(predicate.clone()));
    }
    if let Some(ref object) = query.object {
        sql.push_str(" AND object LIKE ?");
        params_vec.push(Box::new(format!("%{object}%")));
    }
    if query.valid_only {
        sql.push_str(" AND (valid_until IS NULL OR valid_until > strftime('%Y-%m-%d', 'now'))");
    }
    sql.push_str(" ORDER BY created_at DESC");
    if let Some(limit) = query.limit {
        sql.push_str(&format!(" LIMIT {limit}"));
    } else {
        sql.push_str(" LIMIT 500");
    }

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(TripleRow {
            id: row.get(0)?,
            subject: row.get(1)?,
            predicate: row.get(2)?,
            object: row.get(3)?,
            valid_from: row.get(4)?,
            valid_until: row.get(5)?,
            source: row.get(6)?,
            confidence: row.get(7)?,
            created_at: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
        })
    })?;

    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Invalidate a triple by setting its valid_until to today.
pub fn invalidate_triple(conn: &Connection, id: i64) -> Result<bool> {
    let today = &now_iso()[..10];
    let count = conn.execute(
        "UPDATE memory_triples SET valid_until = ?1 WHERE id = ?2 AND valid_until IS NULL",
        params![today, id],
    )?;
    Ok(count > 0)
}

/// Count triples.
pub fn count_triples(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row("SELECT COUNT(*) FROM memory_triples", [], |row| row.get(0))
        .unwrap_or(0))
}

/// Bulk import triples.
pub fn import_triples(
    conn: &Connection,
    triples: &[(String, String, String, TripleWriteOptions)],
) -> Result<TripleImportStats> {
    let mut stats = TripleImportStats::default();
    for (subject, predicate, object, opts) in triples {
        let id = add_triple(conn, subject, predicate, object, opts)?;
        if id > 0 {
            stats.inserted += 1;
        } else {
            stats.skipped += 1;
        }
    }
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
    fn test_add_and_query_triple() {
        let conn = test_db();
        let id = add_triple(
            &conn,
            "user",
            "prefers",
            "dark mode",
            &TripleWriteOptions {
                source: Some("chat".into()),
                confidence: Some(0.9),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(id > 0);

        let results = query_triples(
            &conn,
            &TripleQuery {
                subject: Some("user".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].object, "dark mode");
    }

    #[test]
    fn test_dedup() {
        let conn = test_db();
        let id1 = add_triple(&conn, "user", "prefers", "dark", &Default::default()).unwrap();
        let id2 = add_triple(&conn, "user", "prefers", "dark", &Default::default()).unwrap();
        assert_eq!(id1, id2); // same triple, same ID returned
    }

    #[test]
    fn test_invalidate() {
        let conn = test_db();
        let id = add_triple(&conn, "project", "uses", "PostgreSQL", &Default::default()).unwrap();
        assert!(invalidate_triple(&conn, id).unwrap());

        let valid = query_triples(
            &conn,
            &TripleQuery {
                subject: Some("project".into()),
                valid_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(valid.len(), 0);
    }

    #[test]
    fn test_count() {
        let conn = test_db();
        add_triple(&conn, "a", "b", "c", &Default::default()).unwrap();
        add_triple(&conn, "d", "e", "f", &Default::default()).unwrap();
        assert_eq!(count_triples(&conn).unwrap(), 2);
    }
}
