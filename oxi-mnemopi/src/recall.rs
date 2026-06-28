//! Recall — 6-signal hybrid scoring (FTS5 + vector + keyword).
//!
//! Ported from omp `beam/recall.ts`. Implements the omp hybrid recall
//! formula with FTS, dense (vector), keyword (lexical), importance,
//! recency decay, and veracity signals.

use rusqlite::{Connection, params};

use crate::error::Result;
use crate::types::{RecallOptions, RecallResult, RecallTier, ScoreBreakdown, Veracity};

/// Default number of results when `limit` is not specified.
const DEFAULT_LIMIT: usize = 5;

/// Recall memories matching `query` using FTS5 + vector + keyword hybrid scoring.
///
/// Implements the omp 6-signal hybrid recall: FTS, dense (vector), keyword
/// (lexical), importance, recency decay, and veracity. When a query embedding
/// is provided via `options.query_embedding`, the dense signal is activated.
pub fn recall(
    conn: &Connection,
    query: &str,
    session_id: &str,
    options: &RecallOptions,
) -> Result<Vec<RecallResult>> {
    let limit = options.limit.unwrap_or(DEFAULT_LIMIT);
    let vec_weight = options.vec_weight.unwrap_or(0.5);
    let fts_weight = options.fts_weight.unwrap_or(0.3);
    let imp_weight = options.importance_weight.unwrap_or(0.2);
    let include_working = options.include_working.unwrap_or(true);

    // Tokenize query for lexical relevance
    let query_tokens = tokenize_query(query);

    // ── Collect candidate IDs from FTS ──────────────────────────────────
    let mut working_fts: Vec<(String, f64)> = Vec::new();
    if include_working {
        working_fts = fts_search_working(conn, query, session_id, limit * 4)?;
    }
    let episodic_fts = fts_search_episodic(conn, query, limit * 4)?;

    // ── Vector search (if query embedding provided) ─────────────────────
    let dense_scores: std::collections::HashMap<String, f32> = if let Some(ref qe) =
        options.query_embedding
    {
        let mut candidate_ids: Vec<String> = working_fts.iter().map(|(id, _)| id.clone()).collect();
        // Also search all visible working memory IDs as vector candidates
        let all_ids = all_working_ids(conn, session_id)?;
        for id in all_ids {
            if !candidate_ids.contains(&id) {
                candidate_ids.push(id);
            }
        }
        let hits = crate::vector_index::search_exact(conn, qe, &candidate_ids, limit * 4)?;
        hits.into_iter().map(|h| (h.memory_id, h.score)).collect()
    } else {
        std::collections::HashMap::new()
    };

    // Build normalized FTS score maps
    let working_fts_map: std::collections::HashMap<String, f32> = working_fts
        .iter()
        .map(|(id, bm25)| (id.clone(), normalize_bm25(*bm25)))
        .collect();
    let episodic_fts_map: std::collections::HashMap<i64, f32> = episodic_fts
        .iter()
        .map(|(rowid, bm25)| (*rowid, normalize_bm25(*bm25)))
        .collect();

    let mut results = Vec::new();

    // ── Score working memory candidates ─────────────────────────────────
    if include_working {
        // Collect unique candidate IDs from both FTS and vector results
        let mut seen_ids = std::collections::HashSet::new();
        let mut candidate_ids: Vec<String> = Vec::new();
        for id in working_fts_map.keys() {
            if seen_ids.insert(id.clone()) {
                candidate_ids.push(id.clone());
            }
        }
        for id in dense_scores.keys() {
            if seen_ids.insert(id.clone()) {
                candidate_ids.push(id.clone());
            }
        }

        for id in &candidate_ids {
            if let Some(row) = fetch_working_row(conn, id)? {
                let fts_norm = working_fts_map.get(id).copied().unwrap_or(0.0);
                let fts_matched = working_fts_map.contains_key(id);
                let dense = dense_scores.get(id).copied().unwrap_or(0.0);
                let lexical = lexical_relevance(&query_tokens, &row.content);
                let recency = recency_decay(&row.created_at, options);

                // omp working memory scoring formula:
                // keyword = max(lexical, fts * 0.6)
                // baseScore = keyword * kwShare + importance * importanceWeight + keyword² * 0.08
                // if dense > 0: baseScore = baseScore * 0.8 + dense * 0.2
                let keyword = lexical.max(fts_norm * 0.6);
                let kw_share = (1.0 - imp_weight) * 0.6;
                let mut base_score = keyword * kw_share
                    + (row.importance as f32) * imp_weight
                    + keyword * keyword * 0.08;
                if dense > 0.0 {
                    base_score = base_score * 0.8 + dense * 0.2;
                }

                let mut score = base_score * (0.7 + 0.3 * recency);
                score *= row.veracity.weight();

                results.push(RecallResult {
                    id: row.id,
                    content: row.content,
                    source: row.source,
                    timestamp: row.timestamp,
                    importance: row.importance,
                    veracity: row.veracity,
                    score,
                    tier: Some(RecallTier::Working),
                    signals: Some(ScoreBreakdown {
                        fts: fts_norm,
                        fts_matched,
                        dense,
                        keyword,
                        importance: row.importance as f32,
                        recency_decay: recency,
                        temporal: 0.0,
                    }),
                    metadata: row.metadata,
                });
            }
        }
    }

    // ── Score episodic memory candidates ────────────────────────────────
    {
        let mut seen_rowids = std::collections::HashSet::new();
        for rowid in episodic_fts_map.keys() {
            seen_rowids.insert(*rowid);
        }

        for rowid in &seen_rowids {
            if let Some(row) = fetch_episodic_row(conn, *rowid)? {
                let fts_norm = episodic_fts_map.get(rowid).copied().unwrap_or(0.0);
                let dense = dense_scores.get(&row.id).copied().unwrap_or(0.0);
                let lexical = lexical_relevance(&query_tokens, &row.content);
                let recency = recency_decay(&row.created_at, options);

                // omp episodic memory scoring formula:
                // baseScore = max(dense*vecWeight + fts*ftsWeight + importance*importanceWeight, lexical*0.8)
                let base_score = (dense * vec_weight
                    + fts_norm * fts_weight
                    + (row.importance as f32) * imp_weight)
                    .max(lexical * 0.8);

                let mut score = base_score * (0.7 + 0.3 * recency);
                score *= row.veracity.weight();
                score *= tier_weight(row.tier);

                results.push(RecallResult {
                    id: row.id,
                    content: row.content,
                    source: row.source,
                    timestamp: row.timestamp,
                    importance: row.importance,
                    veracity: row.veracity,
                    score,
                    tier: Some(RecallTier::Episodic),
                    signals: Some(ScoreBreakdown {
                        fts: fts_norm,
                        fts_matched: episodic_fts_map.contains_key(rowid),
                        dense,
                        keyword: lexical,
                        importance: row.importance as f32,
                        recency_decay: recency,
                        temporal: 0.0,
                    }),
                    metadata: None,
                });
            }
        }
    }

    // ── Dedupe + sort + trim ────────────────────────────────────────────
    dedupe_and_sort(&mut results);
    results.truncate(limit);

    // Update recall counts
    for r in &results {
        let _ = crate::store::touch_recall(conn, &r.id);
    }

    Ok(results)
}

/// Get all working memory IDs for a session (for vector search fallback).
fn all_working_ids(conn: &Connection, session_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM working_memory
         WHERE session_id = ?1 AND superseded_by IS NULL
         ORDER BY timestamp DESC LIMIT 500",
    )?;
    let rows = stmt
        .query_map(params![session_id], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// FTS5 search on `fts_working`, returning (id, bm25_score) pairs.
///
/// bm25() returns negative values (lower = better). We negate for
/// "higher = better" convention.
fn fts_search_working(
    conn: &Connection,
    query: &str,
    session_id: &str,
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let fts_query = build_fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT fts_working.id, bm25(fts_working) as score
         FROM fts_working
         JOIN working_memory ON fts_working.id = working_memory.id
         WHERE fts_working MATCH ?1
           AND working_memory.session_id = ?2
           AND working_memory.superseded_by IS NULL
         ORDER BY score
         LIMIT ?3",
    )?;

    let rows = stmt
        .query_map(params![fts_query, session_id, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(rows)
}

/// FTS5 search on `fts_episodes`.
fn fts_search_episodic(conn: &Connection, query: &str, limit: usize) -> Result<Vec<(i64, f64)>> {
    let fts_query = build_fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT episodic_memory.rowid, bm25(fts_episodes) as score
         FROM fts_episodes
         JOIN episodic_memory ON fts_episodes.rowid = episodic_memory.rowid
         WHERE fts_episodes MATCH ?1
         ORDER BY score
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(params![fts_query, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(rows)
}

/// Build an FTS5 MATCH query from user input.
///
/// Quotes each token to avoid FTS5 syntax errors, then joins with OR.
fn build_fts_query(query: &str) -> String {
    let tokens: Vec<&str> = query
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .take(20) // cap to avoid overly complex queries
        .collect();

    if tokens.is_empty() {
        return String::new();
    }

    tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Normalize bm25 score to 0–1 range.
///
/// FTS5 bm25 returns negative values; more negative = better match.
/// We negate and apply a sigmoid-like normalization.
fn normalize_bm25(bm25: f64) -> f32 {
    let neg = -bm25 as f32; // higher = better
    // Clamp and normalize: a score of 0 means no match, 1 means perfect.
    (neg / (neg + 1.0)).clamp(0.0, 1.0)
}

// ── Keyword / lexical relevance — ported from omp recall.ts ────────────

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "i", "in", "is", "it",
    "of", "on", "or", "that", "the", "this", "to", "was", "what", "when", "where", "who", "with",
];

/// Tokenize text into lowercase alphanumeric tokens (len >= 2).
fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|t| t.len() >= 2)
        .collect()
}

/// Tokenize query, filtering stop words.
fn tokenize_query(query: &str) -> Vec<String> {
    tokenize(query)
        .into_iter()
        .filter(|t| !STOP_WORDS.contains(&t.as_str()))
        .collect()
}

/// Clamp a float to [0, 1].
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Lexical (keyword) relevance between query tokens and content.
///
/// Ported from omp `lexicalRelevance`. Uses exact + partial (substring)
/// token matching. Returns 0–1.
fn lexical_relevance(query_tokens: &[String], content: &str) -> f32 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let content_lower = content.to_lowercase();
    let content_tokens: Vec<String> = tokenize(&content_lower);
    let content_token_set: std::collections::HashSet<&str> =
        content_tokens.iter().map(|s| s.as_str()).collect();

    // Single-token query: count occurrences
    if query_tokens.len() == 1 {
        let token = &query_tokens[0];
        if token.is_empty() || !content_lower.contains(token.as_str()) {
            return 0.0;
        }
        let count = content_lower.matches(token.as_str()).count();
        return clamp01(0.7 + (count.saturating_sub(1).min(3) as f32) * 0.1);
    }

    // Multi-token: exact + partial matching
    let mut exact = 0;
    let mut partial = 0;
    for token in query_tokens {
        if content_token_set.contains(token.as_str()) || content_lower.contains(token.as_str()) {
            exact += 1;
            continue;
        }
        if token.len() >= 4 {
            for ct in &content_tokens {
                if ct.len() >= 4 && (ct.contains(token.as_str()) || token.contains(ct.as_str())) {
                    partial += 1;
                    break;
                }
            }
        }
    }

    clamp01((exact as f32 + partial as f32 * 0.5) / query_tokens.len() as f32)
}

/// Exponential recency decay based on memory age vs. halflife.
///
/// `decay = 0.5 ^ (age_hours / halflife_hours)`
/// Fresh memory → 1.0, one halflife old → 0.5, two halflives → 0.25, etc.
fn recency_decay(created_at: &str, _options: &RecallOptions) -> f32 {
    let halflife_hours = 72.0f64; // default recency_halflife

    let parsed = chrono::DateTime::parse_from_rfc3339(created_at).or_else(|_| {
        chrono::NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S")
            .map(|dt| dt.and_utc().fixed_offset())
    });

    let Ok(dt) = parsed else {
        return 0.5; // unknown age → neutral
    };

    let age_hours = (chrono::Utc::now().signed_duration_since(dt).num_seconds() as f64) / 3600.0;
    let decay = 0.5f64.powf(age_hours / halflife_hours);
    decay as f32
}

/// Tier weight for episodic memory scoring.
fn tier_weight(tier: u8) -> f32 {
    match tier {
        1 => 1.0,
        2 => 0.85,
        3 => 0.7,
        _ => 0.5,
    }
}

/// Deduplicate results by content similarity and sort by score descending.
fn dedupe_and_sort(results: &mut Vec<RecallResult>) {
    // Dedupe by exact content match (keep highest-scoring).
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.dedup_by(|a, b| a.content == b.content);
}

// ── Row fetching ─────────────────────────────────────────────────────────

fn fetch_working_row(conn: &Connection, id: &str) -> Result<Option<crate::types::MemoryRow>> {
    use rusqlite::OptionalExtension;
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, session_id, importance,
                metadata_json, veracity, memory_type, recall_count,
                last_recalled, valid_until, superseded_by, scope,
                author_id, author_type, channel_id, created_at
         FROM working_memory WHERE id = ?1",
    )?;

    let row = stmt
        .query_row(params![id], crate::store::row_to_memory_row)
        .optional()?;
    Ok(row)
}

fn fetch_episodic_row(conn: &Connection, rowid: i64) -> Result<Option<EpisodicFetch>> {
    use rusqlite::OptionalExtension;
    let mut stmt = conn.prepare(
        "SELECT id, content, source, timestamp, session_id, importance,
                veracity, tier, created_at
         FROM episodic_memory WHERE rowid = ?1",
    )?;

    let row = stmt
        .query_row(params![rowid], |row| {
            let veracity_str: String = row.get(6).unwrap_or_else(|_| "unknown".to_string());
            Ok(EpisodicFetch {
                id: row.get(0)?,
                content: row.get(1)?,
                source: row.get(2)?,
                timestamp: row.get(3)?,
                session_id: row.get(4)?,
                importance: row.get(5).unwrap_or(0.5),
                veracity: Veracity::from_str_lossy(&veracity_str),
                tier: row.get::<_, i64>(7).unwrap_or(1) as u8,
                created_at: row.get(8).unwrap_or_default(),
            })
        })
        .optional()?;
    Ok(row)
}

#[allow(dead_code)]
struct EpisodicFetch {
    id: String,
    content: String,
    source: Option<String>,
    timestamp: Option<String>,
    session_id: String,
    importance: f64,
    veracity: Veracity,
    tier: u8,
    created_at: String,
}
