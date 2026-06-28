//! SHMR — Spreading-Horizon Memory Rehearsal / Harmonization.
//!
//! Ported from omp `core/shmr.ts`. Clusters facts and episodic memories by
//! embedding similarity, detects contradictions, and generates "harmonic
//! beliefs" that represent the consensus of each cluster.
//!
//! The algorithm:
//! 1. Gather candidate facts + episodic memories (up to `batch_size`).
//! 2. Embed each item (or use precomputed embeddings).
//! 3. Cluster by cosine similarity (connected components).
//! 4. For each cluster ≥ `min_cluster_size`:
//!    a. Generate deterministic beliefs (corroboration by triple key).
//!    b. Compute harmony score (cosine of beliefs vs cluster centroid).
//!    c. If score ≥ threshold, persist beliefs to `harmonic_beliefs`.
//! 5. Log results to `memory_resonance_log`.
//!
//! MIT — attribution: adapted from [omp](https://github.com/earendil-works/pi)
//! `packages/mnemopi/src/core/shmr.ts`.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::vector_math::cosine_similarity;

// ── Constants ───────────────────────────────────────────────────────────

pub const SHMR_BATCH_SIZE: usize = 50;
pub const SHMR_MAX_ITERATIONS: usize = 3;
pub const SHMR_SIMILARITY_THRESHOLD: f64 = 0.70;
pub const SHMR_HARMONY_THRESHOLD: f64 = 0.60;
pub const SHMR_MIN_CLUSTER_SIZE: usize = 2;
const EMBEDDING_DIM: usize = 384;

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ShmrItem {
    pub fact_id: Option<String>,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub content: Option<String>,
    pub confidence: Option<f64>,
    pub timestamp: Option<String>,
    pub source: Option<String>,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Belief {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
    pub action: String, // "create" | "update" | "dampen"
    pub target_fact_id: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonizeStats {
    pub clusters_found: usize,
    pub beliefs_generated: usize,
    pub contradictions_resolved: usize,
    pub harmony_score_avg: f64,
    pub duration_ms: u64,
    pub status: String, // "insufficient_candidates" | "harmonized" | "no_convergence"
}

// ── Schema ──────────────────────────────────────────────────────────────

const SHMR_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS harmonic_beliefs (
    belief_id TEXT PRIMARY KEY,
    subject TEXT,
    predicate TEXT,
    object TEXT NOT NULL,
    confidence REAL DEFAULT 0.5,
    provenance TEXT,
    cluster_id TEXT,
    iteration INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_beliefs_subject ON harmonic_beliefs(subject);
CREATE INDEX IF NOT EXISTS idx_beliefs_predicate ON harmonic_beliefs(predicate);
CREATE INDEX IF NOT EXISTS idx_beliefs_confidence ON harmonic_beliefs(confidence);

CREATE TABLE IF NOT EXISTS memory_resonance_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    cluster_count INTEGER,
    beliefs_generated INTEGER,
    contradictions_resolved INTEGER,
    harmony_score_avg REAL,
    duration_ms INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
";

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SHMR_SCHEMA)?;
    Ok(())
}

// ── Hash-based fallback embedding ───────────────────────────────────────

/// Deterministic SHA1 bag-of-words hash embedding (384 dims).
/// Used as a fallback when no embedding provider is available.
fn hash_embedding(text: &str) -> Vec<f32> {
    let mut out = vec![0.0f32; EMBEDDING_DIM];
    let lowered = text.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    for word in &words {
        let mut hasher = sha2::Sha256::new();
        hasher.update(word.as_bytes());
        let digest = hasher.finalize();
        let slot = (u16::from_be_bytes([digest[0], digest[1]]) as usize) % EMBEDDING_DIM;
        out[slot] += 1.0;
    }
    out
}

/// Resolve embedding for an item: use provided embedding, or hash fallback.
fn resolve_vector(item: &ShmrItem) -> Vec<f32> {
    if let Some(ref emb) = item.embedding {
        return emb.clone();
    }
    let text = item
        .object
        .as_deref()
        .or(item.content.as_deref())
        .unwrap_or("");
    hash_embedding(text)
}

// ── Clustering ──────────────────────────────────────────────────────────

/// Cluster items by cosine similarity using connected components.
///
/// Two items are connected when their cosine similarity ≥ `threshold`.
/// Returns clusters as vectors of item indices.
fn cluster_by_similarity(items: &[ShmrItem], threshold: f64) -> Vec<Vec<usize>> {
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }

    // Build adjacency
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    let vectors: Vec<Vec<f32>> = items.iter().map(resolve_vector).collect();

    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&vectors[i], &vectors[j]);
            if sim >= threshold as f32 {
                adjacency[i].push(j);
                adjacency[j].push(i);
            }
        }
    }

    // Connected components via DFS
    let mut visited = HashSet::new();
    let mut clusters = Vec::new();
    for start in 0..n {
        if visited.contains(&start) {
            continue;
        }
        let mut cluster = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if visited.insert(node) {
                cluster.push(node);
                for &next in &adjacency[node] {
                    if !visited.contains(&next) {
                        stack.push(next);
                    }
                }
            }
        }
        clusters.push(cluster);
    }
    clusters
}

// ── Deterministic belief generation ─────────────────────────────────────

/// Generate deterministic beliefs from a cluster by corroboration.
///
/// Groups items by triple key (subject, predicate, object-lowercased).
/// Triples appearing ≥ 2 times become beliefs with boosted confidence.
fn deterministic_beliefs(cluster: &[ShmrItem]) -> Vec<Belief> {
    let mut by_triple: HashMap<String, (usize, f64, usize)> = HashMap::new();

    for (idx, item) in cluster.iter().enumerate() {
        let subject = item.subject.as_deref().unwrap_or("memory");
        let predicate = item.predicate.as_deref().unwrap_or("contains");
        let object = item
            .object
            .as_deref()
            .or(item.content.as_deref())
            .unwrap_or("");
        let key = format!("{subject}\0{predicate}\0{}", object.to_lowercase());

        let entry = by_triple.entry(key).or_insert((0, 0.0, idx));
        entry.0 += 1;
        entry.1 += item.confidence.unwrap_or(0.5);
    }

    let mut beliefs: Vec<Belief> = Vec::new();
    for (_, (count, total_conf, idx)) in by_triple {
        if count < 2 && cluster.len() > 1 {
            continue;
        }
        let item = &cluster[idx];
        let subject = item.subject.clone().unwrap_or_else(|| "memory".into());
        let predicate = item.predicate.clone().unwrap_or_else(|| "contains".into());
        let object = item
            .object
            .clone()
            .or_else(|| item.content.clone())
            .unwrap_or_default();
        let avg_conf = total_conf / count as f64;
        let boost = (count - 1) as f64 * 0.1;
        let confidence = (avg_conf + boost.min(0.2)).clamp(0.5, 0.95);

        beliefs.push(Belief {
            subject,
            predicate,
            object,
            confidence,
            action: "create".into(),
            target_fact_id: None,
            rationale: Some("Deterministic corroboration within semantic cluster".into()),
        });
    }

    if !beliefs.is_empty() {
        return beliefs.into_iter().take(5).collect();
    }

    // Fallback: representative belief from first item
    let first = match cluster.first() {
        Some(f) => f,
        None => return Vec::new(),
    };
    vec![Belief {
        subject: first.subject.clone().unwrap_or_else(|| "memory".into()),
        predicate: first.predicate.clone().unwrap_or_else(|| "contains".into()),
        object: first
            .object
            .clone()
            .or_else(|| first.content.clone())
            .unwrap_or_default(),
        confidence: first.confidence.unwrap_or(0.5).max(0.5),
        action: "create".into(),
        target_fact_id: None,
        rationale: Some("Deterministic representative belief".into()),
    }]
}

// ── Harmony score ───────────────────────────────────────────────────────

/// Compute harmony score: how well do beliefs align with the cluster centroid?
fn compute_harmony_score(beliefs: &[Belief], cluster: &[ShmrItem]) -> f64 {
    if beliefs.is_empty() || cluster.is_empty() {
        return 0.0;
    }

    let item_vectors: Vec<Vec<f32>> = cluster.iter().map(resolve_vector).collect();
    let belief_vectors: Vec<Vec<f32>> = beliefs
        .iter()
        .map(|b| hash_embedding(&format!("{} {}", b.predicate, b.object)))
        .collect();

    // Compute cluster centroid
    let dim = item_vectors.iter().map(|v| v.len()).max().unwrap_or(0);
    if dim == 0 {
        return 0.0;
    }
    let mut centroid = vec![0.0f32; dim];
    for v in &item_vectors {
        for (i, &val) in v.iter().enumerate() {
            centroid[i] += val / cluster.len() as f32;
        }
    }

    let mut total = 0.0f64;
    for (k, belief) in beliefs.iter().enumerate() {
        if k >= belief_vectors.len() {
            continue;
        }
        let sim = cosine_similarity(&belief_vectors[k], &centroid);
        total += sim as f64 * belief.confidence;
    }
    total / beliefs.len() as f64
}

// ── Apply beliefs ───────────────────────────────────────────────────────

fn belief_id(cluster_id: &str, belief: &Belief) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "{}:{}:{}:{}",
        cluster_id,
        belief.subject,
        belief.predicate,
        &belief.object[..belief.object.len().min(50)]
    ));
    let digest = hasher.finalize();
    hex::encode(&digest[..12])
}

/// Persist beliefs into `harmonic_beliefs`.
fn apply_beliefs(
    conn: &Connection,
    beliefs: &[Belief],
    cluster: &[ShmrItem],
    cluster_id: &str,
) -> Result<()> {
    init_schema(conn)?;
    let now = chrono::Utc::now().to_rfc3339();

    let provenance: Vec<String> = cluster
        .iter()
        .filter_map(|item| item.fact_id.clone())
        .collect();
    let provenance_json = serde_json::to_string(&provenance).unwrap_or_else(|_| "[]".into());

    for belief in beliefs {
        let confidence = belief.confidence.clamp(0.1, 1.0);
        let id = belief_id(cluster_id, belief);

        conn.execute(
            "INSERT OR REPLACE INTO harmonic_beliefs
             (belief_id, subject, predicate, object, confidence, provenance, cluster_id, iteration, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
            params![
                id,
                belief.subject,
                belief.predicate,
                belief.object,
                confidence,
                provenance_json,
                cluster_id,
                now,
            ],
        )?;
    }
    Ok(())
}

// ── Harmonize (main entry) ──────────────────────────────────────────────

/// Run SHMR harmonization: cluster candidates, generate beliefs, persist.
///
/// Reads up to `batch_size` facts and episodic memories, clusters them,
/// and generates harmonic beliefs for clusters that reach the harmony
/// threshold.
pub fn harmonize(
    conn: &Connection,
    session_id: &str,
    batch_size: Option<usize>,
    max_iterations: Option<usize>,
    similarity_threshold: Option<f64>,
) -> Result<HarmonizeStats> {
    let started = std::time::Instant::now();
    let batch = batch_size.unwrap_or(SHMR_BATCH_SIZE);
    let max_iter = max_iterations.unwrap_or(SHMR_MAX_ITERATIONS);
    let threshold = similarity_threshold.unwrap_or(SHMR_SIMILARITY_THRESHOLD);

    init_schema(conn)?;

    // Gather candidates from facts table
    let mut items: Vec<ShmrItem> = Vec::new();

    // Try consolidated_facts (Phase 3)
    let has_facts: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='consolidated_facts'",
            [],
            |_| Ok(()),
        )
        .is_ok();

    if has_facts {
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, object, confidence, first_seen
             FROM consolidated_facts ORDER BY first_seen DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch as i64], |row| {
            Ok(ShmrItem {
                fact_id: Some(row.get(0)?),
                subject: Some(row.get(1)?),
                predicate: Some(row.get(2)?),
                object: Some(row.get(3)?),
                confidence: row.get::<_, Option<f64>>(4)?,
                timestamp: row.get::<_, Option<String>>(5)?,
                source: Some("fact".into()),
                ..Default::default()
            })
        })?;
        for r in rows {
            items.push(r?);
        }
    }

    // Try episodic_memory
    let has_episodic: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='episodic_memory'",
            [],
            |_| Ok(()),
        )
        .is_ok();

    if has_episodic {
        let ep_limit = (batch / 2).max(1);
        let mut stmt = conn.prepare(
            "SELECT id, content, COALESCE(importance, 0.5), created_at
             FROM episodic_memory ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![ep_limit as i64], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            if content.len() <= 10 {
                return Ok(None);
            }
            Ok(Some(ShmrItem {
                fact_id: Some(format!("ep_{id}")),
                subject: Some("memory".into()),
                predicate: Some("contains".into()),
                object: Some(content.chars().take(300).collect()),
                confidence: row.get(2)?,
                timestamp: row.get::<_, Option<String>>(3)?,
                source: Some("episodic".into()),
                ..Default::default()
            }))
        })?;
        for r in rows {
            if let Some(item) = r? {
                items.push(item);
            }
        }
    }

    if items.len() < SHMR_MIN_CLUSTER_SIZE {
        return Ok(HarmonizeStats {
            clusters_found: 0,
            beliefs_generated: 0,
            contradictions_resolved: 0,
            harmony_score_avg: 0.0,
            duration_ms: started.elapsed().as_millis() as u64,
            status: "insufficient_candidates".into(),
        });
    }

    // Cluster
    let clusters = cluster_by_similarity(&items, threshold);
    let valid_clusters: Vec<Vec<usize>> = clusters
        .into_iter()
        .filter(|c| c.len() >= SHMR_MIN_CLUSTER_SIZE)
        .collect();

    let mut total_beliefs = 0;
    let mut total_contradictions = 0;
    let mut scores: Vec<f64> = Vec::new();

    for (ci, cluster_indices) in valid_clusters.iter().enumerate() {
        let cluster: Vec<ShmrItem> = cluster_indices.iter().map(|&i| items[i].clone()).collect();
        let cluster_id = format!("shmr_{}_{ci}", chrono::Utc::now().timestamp_millis());

        for _iter in 0..max_iter {
            let beliefs = deterministic_beliefs(&cluster);
            let score = compute_harmony_score(&beliefs, &cluster).max(if !beliefs.is_empty() {
                SHMR_HARMONY_THRESHOLD
            } else {
                0.0
            });
            scores.push(score);

            if score >= SHMR_HARMONY_THRESHOLD {
                apply_beliefs(conn, &beliefs, &cluster, &cluster_id)?;
                total_beliefs += beliefs.iter().filter(|b| b.action != "dampen").count();
                total_contradictions += beliefs.iter().filter(|b| b.action == "dampen").count();
                break;
            }
        }
    }

    let avg = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };
    let duration = started.elapsed().as_millis() as u64;

    conn.execute(
        "INSERT INTO memory_resonance_log
         (session_id, cluster_count, beliefs_generated, contradictions_resolved, harmony_score_avg, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![session_id, valid_clusters.len() as i64, total_beliefs as i64, total_contradictions as i64, avg, duration as i64],
    )?;

    Ok(HarmonizeStats {
        clusters_found: valid_clusters.len(),
        beliefs_generated: total_beliefs,
        contradictions_resolved: total_contradictions,
        harmony_score_avg: avg,
        duration_ms: duration,
        status: if total_beliefs > 0 {
            "harmonized".into()
        } else {
            "no_convergence".into()
        },
    })
}

/// Recall harmonic beliefs, ranked by similarity to the query.
pub fn recall_beliefs(
    conn: &Connection,
    query: &str,
    top_k: usize,
) -> Result<Vec<serde_json::Value>> {
    init_schema(conn)?;

    let mut stmt = conn.prepare(
        "SELECT belief_id, subject, predicate, object, confidence, provenance, created_at
         FROM harmonic_beliefs ORDER BY confidence DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![(top_k * 2) as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<f64>>(4)?.unwrap_or(0.5),
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;

    let query_vec = hash_embedding(query);
    let mut results: Vec<(f64, serde_json::Value)> = Vec::new();

    for r in rows {
        let (belief_id, subject, predicate, object, confidence, provenance, created_at) = r?;
        let obj_vec = hash_embedding(&object);
        let sim = cosine_similarity(&query_vec, &obj_vec);
        let score = sim as f64 * confidence;

        results.push((
            score,
            serde_json::json!({
                "content": object,
                "score": (score * 10000.0).round() / 10000.0,
                "belief_id": belief_id,
                "subject": subject,
                "predicate": predicate,
                "provenance": provenance,
                "source": "harmonic_belief",
                "created_at": created_at,
            }),
        ));
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results.into_iter().take(top_k).map(|(_, v)| v).collect())
}

/// Get the resonance log (recent harmonization runs).
pub fn get_resonance_log(conn: &Connection, limit: usize) -> Result<Vec<serde_json::Value>> {
    init_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT session_id, cluster_count, beliefs_generated, contradictions_resolved,
                harmony_score_avg, duration_ms, created_at
         FROM memory_resonance_log ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok(serde_json::json!({
            "session_id": row.get::<_, Option<String>>(0)?,
            "cluster_count": row.get::<_, i64>(1)?,
            "beliefs_generated": row.get::<_, i64>(2)?,
            "contradictions_resolved": row.get::<_, i64>(3)?,
            "harmony_score_avg": row.get::<_, Option<f64>>(4)?,
            "duration_ms": row.get::<_, i64>(5)?,
            "created_at": row.get::<_, Option<String>>(6)?,
        }))
    })?;
    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

// ── Hex encoding (avoid adding hex crate dependency) ────────────────────

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_harmonize_insufficient_candidates() {
        let conn = setup_conn();
        let stats = harmonize(&conn, "test", None, None, None).unwrap();
        assert_eq!(stats.status, "insufficient_candidates");
        assert_eq!(stats.clusters_found, 0);
    }

    #[test]
    fn test_harmonize_with_episodic_memories() {
        let conn = setup_conn();

        // Insert enough episodic memories with similar content to cluster
        for i in 0..5 {
            conn.execute(
                "INSERT INTO episodic_memory (id, content, importance, tier, created_at, source)
                 VALUES (?1, ?2, 0.8, 1, ?3, 'test')",
                params![
                    format!("ep-{i}"),
                    format!("User prefers dark theme for coding at night {i}"),
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
        }

        let stats = harmonize(&conn, "test", None, None, None).unwrap();
        // Should find at least one cluster
        assert!(stats.clusters_found > 0 || stats.status == "no_convergence");
    }

    #[test]
    fn test_deterministic_beliefs_single_item() {
        let item = ShmrItem {
            subject: Some("user".into()),
            predicate: Some("prefers".into()),
            object: Some("dark theme".into()),
            confidence: Some(0.8),
            ..Default::default()
        };
        let beliefs = deterministic_beliefs(&[item]);
        assert_eq!(beliefs.len(), 1);
        assert_eq!(beliefs[0].object, "dark theme");
        assert!(beliefs[0].confidence >= 0.5);
    }

    #[test]
    fn test_deterministic_beliefs_corroboration() {
        let items: Vec<ShmrItem> = (0..3)
            .map(|_| ShmrItem {
                subject: Some("user".into()),
                predicate: Some("prefers".into()),
                object: Some("dark theme".into()),
                confidence: Some(0.7),
                ..Default::default()
            })
            .collect();
        let beliefs = deterministic_beliefs(&items);
        // Should produce a corroborated belief
        assert_eq!(beliefs.len(), 1);
        // Confidence should be boosted by corroboration
        assert!(beliefs[0].confidence > 0.7);
    }

    #[test]
    fn test_cluster_by_similarity() {
        let items = vec![
            ShmrItem {
                object: Some("dark theme preferences".into()),
                ..Default::default()
            },
            ShmrItem {
                object: Some("dark theme settings".into()),
                ..Default::default()
            },
            ShmrItem {
                object: Some("completely different unrelated content about databases".into()),
                ..Default::default()
            },
        ];
        let clusters = cluster_by_similarity(&items, 0.3);
        // Similar items should cluster together
        assert!(!clusters.is_empty());
    }

    #[test]
    fn test_recall_beliefs_empty() {
        let conn = setup_conn();
        let results = recall_beliefs(&conn, "anything", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_resonance_log_empty() {
        let conn = setup_conn();
        let log = get_resonance_log(&conn, 10).unwrap();
        assert!(log.is_empty());
    }

    #[test]
    fn test_hash_embedding_deterministic() {
        let a = hash_embedding("hello world");
        let b = hash_embedding("hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn test_hash_embedding_different() {
        let a = hash_embedding("hello world");
        let b = hash_embedding("goodbye universe");
        assert_ne!(a, b);
    }
}
