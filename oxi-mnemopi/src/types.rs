//! Core type definitions — ported from omp `beam/types.ts` and `types.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Veracity (truth confidence) of a memory entry.
///
/// Ported from omp `Veracity`. Controls recall score weighting:
/// `stated`/`true` = 1.0, `unknown` = 0.8, `inferred` = 0.7,
/// `imported` = 0.6, `tool` = 0.5, `false` = 0.0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Veracity {
    #[default]
    Unknown,
    LikelyTrue,
    True,
    False,
    Stated,
    Inferred,
    Tool,
    Imported,
    Contested,
}

impl Veracity {
    /// Parse from a string, defaulting to `Unknown` for unrecognized values.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "true" => Self::True,
            "false" => Self::False,
            "stated" => Self::Stated,
            "inferred" => Self::Inferred,
            "tool" => Self::Tool,
            "imported" => Self::Imported,
            "unknown" => Self::Unknown,
            "likely_true" => Self::LikelyTrue,
            "contested" => Self::Contested,
            _ => Self::Unknown,
        }
    }

    /// Weight applied during recall scoring.
    pub fn weight(&self) -> f32 {
        match self {
            Self::True | Self::Stated | Self::LikelyTrue => 1.0,
            Self::Contested => 0.9,
            Self::Unknown => 0.8,
            Self::Inferred => 0.7,
            Self::Imported => 0.6,
            Self::Tool => 0.5,
            Self::False => 0.0,
        }
    }

    /// Serialize as lowercase string for SQLite storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::LikelyTrue => "likely_true",
            Self::True => "true",
            Self::False => "false",
            Self::Stated => "stated",
            Self::Inferred => "inferred",
            Self::Tool => "tool",
            Self::Imported => "imported",
            Self::Contested => "contested",
        }
    }
}

/// Memory scope — controls visibility within banks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryScope {
    #[default]
    Global,
    Session,
    Channel,
    Other(String),
}

impl MemoryScope {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Global => "global",
            Self::Session => "session",
            Self::Channel => "channel",
            Self::Other(s) => s,
        }
    }
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Metadata stored as JSON alongside each memory.
pub type Metadata = HashMap<String, serde_json::Value>;

/// A stored memory row — mirrors omp `MemoryRow` / `WorkingMemoryRow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    pub content: String,
    pub source: Option<String>,
    pub timestamp: Option<String>,
    pub session_id: String,
    pub importance: f64,
    pub metadata: Option<Metadata>,
    pub veracity: Veracity,
    pub memory_type: Option<String>,
    pub recall_count: Option<i64>,
    pub last_recalled: Option<String>,
    pub valid_until: Option<String>,
    pub superseded_by: Option<String>,
    pub scope: MemoryScope,
    pub author_id: Option<String>,
    pub author_type: Option<String>,
    pub channel_id: Option<String>,
    pub created_at: String,
}

impl Default for MemoryRow {
    fn default() -> Self {
        Self {
            id: String::new(),
            content: String::new(),
            source: None,
            timestamp: None,
            session_id: "default".to_string(),
            importance: 0.5,
            metadata: None,
            veracity: Veracity::Unknown,
            memory_type: None,
            recall_count: Some(0),
            last_recalled: None,
            valid_until: None,
            superseded_by: None,
            scope: MemoryScope::Global,
            author_id: None,
            author_type: None,
            channel_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// An episodic memory row (consolidated from working memory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicMemoryRow {
    pub rowid: i64,
    pub id: String,
    pub content: String,
    pub source: Option<String>,
    pub timestamp: Option<String>,
    pub session_id: String,
    pub importance: f64,
    pub veracity: Veracity,
    pub summary_of: String,
    /// 1 = detail, 2 = compressed, 3 = heavily compressed.
    pub tier: u8,
    pub degraded_at: Option<String>,
    pub created_at: String,
}

/// A single recall result with scoring breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub id: String,
    pub content: String,
    pub source: Option<String>,
    pub timestamp: Option<String>,
    pub importance: f64,
    pub veracity: Veracity,
    /// Final hybrid score (0–1 range, higher = more relevant).
    pub score: f32,
    /// Which tier this result came from.
    pub tier: Option<RecallTier>,
    /// Per-signal breakdown for diagnostics.
    pub signals: Option<ScoreBreakdown>,
    pub metadata: Option<Metadata>,
}

/// Which memory tier a recall result originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallTier {
    Working,
    Episodic,
    Fact,
}

/// Per-signal scoring breakdown for a recall candidate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub fts: f32,
    pub fts_matched: bool,
    pub dense: f32,
    pub keyword: f32,
    pub importance: f32,
    pub recency_decay: f32,
    pub temporal: f32,
}

/// Options for `remember()`.
#[derive(Debug, Clone, Default)]
pub struct RememberOptions {
    pub source: Option<String>,
    pub importance: Option<f64>,
    pub metadata: Option<Metadata>,
    pub veracity: Option<Veracity>,
    pub memory_type: Option<String>,
    pub scope: Option<MemoryScope>,
    pub timestamp: Option<String>,
    pub extract: bool,
    pub extract_entities: bool,
}

/// Options for `recall()`.
#[derive(Debug, Clone, Default)]
pub struct RecallOptions {
    pub limit: Option<usize>,
    pub vec_weight: Option<f32>,
    pub fts_weight: Option<f32>,
    pub importance_weight: Option<f32>,
    pub include_working: Option<bool>,
    pub query_embedding: Option<Vec<f32>>,
}

/// Configuration for the Mnemopi engine.
#[derive(Debug, Clone)]
pub struct MnemopiConfig {
    /// Session identifier for this Mnemopi instance.
    pub session_id: String,
    /// Recency halflife in hours (default: 72).
    pub recency_halflife_hours: f64,
    /// Working memory max items (default: 10000).
    pub working_memory_limit: usize,
    /// Working memory TTL in hours (default: 24).
    pub working_memory_ttl_hours: f64,
    /// Vector weight in hybrid scoring (default: 0.5).
    pub vec_weight: f32,
    /// FTS weight in hybrid scoring (default: 0.3).
    pub fts_weight: f32,
    /// Importance weight in hybrid scoring (default: 0.2).
    pub importance_weight: f32,
    /// Max episode chars for consolidation (default: 100_000).
    pub max_episode_chars: usize,
}

impl Default for MnemopiConfig {
    fn default() -> Self {
        Self {
            session_id: "default".to_string(),
            recency_halflife_hours: 72.0,
            working_memory_limit: 10_000,
            working_memory_ttl_hours: 24.0,
            vec_weight: 0.5,
            fts_weight: 0.3,
            importance_weight: 0.2,
            max_episode_chars: 100_000,
        }
    }
}

/// Stats about a memory store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryStats {
    pub working_count: usize,
    pub episodic_count: usize,
    pub embedding_count: usize,
    pub by_source: HashMap<String, usize>,
    pub by_session: HashMap<String, usize>,
}
