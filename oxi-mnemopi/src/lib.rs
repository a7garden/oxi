//! oxi-mnemopi — local SQLite vector memory engine.
//!
//! Rust port of omp [Mnemopi](https://github.com/can1357/oh-my-pi)
//! (`packages/mnemopi/`, MIT). Provides working-memory
//! remember / forget / update / get / get_context / get_stats plus recall
//! with FTS5 + vector scoring.
//!
//! All store operations are synchronous (blocking SQLite). In async
//! contexts, the [`Mnemopi`] facade wraps each call in `spawn_blocking`.

pub mod aaak;
pub mod annotations;
pub mod banks;
pub mod chat_normalize;
pub mod consolidate;
pub mod content_sanitizer;
pub mod db;
pub mod embeddings;
pub mod entities;
pub mod episodic_graph;
pub mod error;
pub mod extraction;
pub mod mmr;
pub mod orchestrator;
pub mod patterns;
pub mod polyphonic_recall;
pub mod query_cache;
pub mod query_intent;
pub mod recall;
pub mod recall_diagnostics;
pub mod schema;
pub mod session;
pub mod shmr;
pub mod store;
pub mod synonyms;
pub mod temporal;
pub mod token_counter;
pub mod triples;
pub mod types;
pub mod vector_index;
pub mod vector_math;
pub mod veracity_consolidation;
pub mod weibull;

pub use chat_normalize::{ExtractionRate, extraction_rate, normalize_batch, normalize_chat};
pub use db::MnemopiDb;
pub use error::{MnemopiError, Result};
pub use recall::recall as recall_query;
use std::path::Path;
use std::sync::Arc;
pub use store::{
    forget, get, get_context, get_stats, invalidate, list_by_source, remember, update,
};
pub use types::*;

/// Mnemopi memory engine facade.
///
/// Owns a [`MnemopiDb`] and a [`MnemopiConfig`], providing a high-level
/// async API for remember / recall / forget / update / get / stats.
///
/// All SQLite operations are wrapped in `spawn_blocking` — safe to call
/// from async contexts.
#[derive(Clone)]
pub struct Mnemopi {
    db: Arc<MnemopiDb>,
    config: MnemopiConfig,
}

impl std::fmt::Debug for Mnemopi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mnemopi")
            .field("db", &self.db)
            .field("config", &self.config)
            .finish()
    }
}

impl Mnemopi {
    /// Open or create a Mnemopi engine at `path`.
    pub fn open(path: &Path, config: MnemopiConfig) -> Result<Self> {
        let db = MnemopiDb::open(path)?;
        Ok(Self {
            db: Arc::new(db),
            config,
        })
    }

    /// Create an in-memory Mnemopi engine (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let db = MnemopiDb::open_in_memory()?;
        Ok(Self {
            db: Arc::new(db),
            config: MnemopiConfig::default(),
        })
    }

    /// Open an in-memory engine with a custom config (for tests).
    pub fn open_in_memory_with_config(config: MnemopiConfig) -> Result<Self> {
        let db = MnemopiDb::open_in_memory()?;
        Ok(Self {
            db: Arc::new(db),
            config,
        })
    }

    /// Store a new memory. Returns its ID.
    pub async fn remember(&self, content: &str, options: RememberOptions) -> Result<String> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();
        let content = content.to_string();

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| store::remember(conn, &content, &session_id, &options))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Recall memories matching `query`.
    pub async fn recall(&self, query: &str, options: RecallOptions) -> Result<Vec<RecallResult>> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();
        let query = query.to_string();

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| recall::recall(conn, &query, &session_id, &options))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Recall memories by text query (synchronous, for tool bridge).
    pub fn recall_blocking(&self, query: &str, options: RecallOptions) -> Vec<RecallResult> {
        self.db
            .with_conn(|conn| recall::recall(conn, query, &self.config.session_id, &options))
            .expect("recall_blocking failed")
    }

    /// Delete a memory by ID.
    pub async fn forget(&self, id: &str) -> Result<bool> {
        let db = self.db.clone();
        let id = id.to_string();

        tokio::task::spawn_blocking(move || db.with_conn(|conn| store::forget(conn, &id)))
            .await
            .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Update a memory's content and/or importance.
    pub async fn update(
        &self,
        id: &str,
        content: Option<&str>,
        importance: Option<f64>,
    ) -> Result<bool> {
        let db = self.db.clone();
        let id = id.to_string();
        let content = content.map(|s| s.to_string());

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| store::update(conn, &id, content.as_deref(), importance))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Fetch a memory by ID.
    pub async fn get(&self, id: &str) -> Result<Option<MemoryRow>> {
        let db = self.db.clone();
        let id = id.to_string();

        tokio::task::spawn_blocking(move || db.with_conn(|conn| store::get(conn, &id)))
            .await
            .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Get recent working memory entries (newest first).
    pub async fn get_context(&self, limit: usize) -> Result<Vec<MemoryRow>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || db.with_conn(|conn| store::get_context(conn, limit)))
            .await
            .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Get memory store statistics.
    pub async fn get_stats(&self) -> Result<MemoryStats> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || db.with_conn(store::get_stats))
            .await
            .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Invalidate a memory (mark as superseded).
    pub async fn invalidate(&self, id: &str, replacement_id: Option<&str>) -> Result<bool> {
        let db = self.db.clone();
        let id = id.to_string();
        let replacement = replacement_id.map(|s| s.to_string());

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| store::invalidate(conn, &id, replacement.as_deref()))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    // ── Synchronous (blocking) variants ────────────────────────────────

    /// Store a new memory (synchronous). Returns its ID.
    pub fn blocking_remember(&self, content: &str, options: RememberOptions) -> String {
        let session_id = &self.config.session_id;
        self.db
            .with_conn(|conn| store::remember(conn, content, session_id, &options))
            .expect("blocking_remember failed")
    }

    /// Delete a memory by ID (synchronous).
    pub fn blocking_forget(&self, id: &str) -> bool {
        self.db
            .with_conn(|conn| store::forget(conn, id))
            .expect("blocking_forget failed")
    }

    /// Update a memory (synchronous).
    pub fn blocking_update(
        &self,
        id: &str,
        content: Option<&str>,
        importance: Option<f64>,
    ) -> bool {
        self.db
            .with_conn(|conn| store::update(conn, id, content, importance))
            .expect("blocking_update failed")
    }

    /// Fetch a memory by ID (synchronous).
    pub fn blocking_get(&self, id: &str) -> Option<MemoryRow> {
        self.db
            .with_conn(|conn| store::get(conn, id))
            .expect("blocking_get failed")
    }

    /// Get memory store statistics (synchronous).
    pub fn blocking_get_stats(&self) -> MemoryStats {
        self.db
            .with_conn(store::get_stats)
            .expect("blocking_get_stats failed")
    }

    /// Invalidate a memory (synchronous).
    pub fn blocking_invalidate(&self, id: &str, replacement_id: Option<&str>) -> bool {
        self.db
            .with_conn(|conn| store::invalidate(conn, id, replacement_id))
            .expect("blocking_invalidate failed")
    }

    /// List memories by source/subject (async).
    pub async fn list_by_source(&self, source: &str, limit: usize) -> Result<Vec<MemoryRow>> {
        let db = self.db.clone();
        let source = source.to_string();

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| store::list_by_source(conn, &source, limit))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// List memories by source/subject (synchronous).
    pub fn blocking_list_by_source(&self, source: &str, limit: usize) -> Vec<MemoryRow> {
        self.db
            .with_conn(|conn| store::list_by_source(conn, source, limit))
            .expect("blocking_list_by_source failed")
    }

    // ── Phase 3: Consolidation, Orchestration, Harmonization ───────────

    /// Run sleep consolidation (working → episodic compression).
    ///
    /// Moves old working memories into episodic summaries, degrades old
    /// episodic tiers, and extracts facts. When `dry_run` is true, no
    /// modifications are made.
    pub async fn sleep(&self, ttl_hours: i64, dry_run: bool) -> Result<consolidate::SleepResult> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| consolidate::sleep(conn, &session_id, ttl_hours, None, dry_run))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Synchronous sleep consolidation.
    pub fn blocking_sleep(&self, ttl_hours: i64, dry_run: bool) -> consolidate::SleepResult {
        self.db
            .with_conn(|conn| {
                consolidate::sleep(conn, &self.config.session_id, ttl_hours, None, dry_run)
            })
            .expect("blocking_sleep failed")
    }

    /// Run orchestrated recall (linear or polyphonic dispatch).
    pub async fn orchestrate_recall(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<orchestrator::OrchestratedRecallResult>> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();
        let query = query.to_string();

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| {
                orchestrator::orchestrate_recall(
                    conn,
                    &query,
                    top_k,
                    &orchestrator::OrchestrateRecallOptions {
                        session_id,
                        ..Default::default()
                    },
                )
            })
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Synchronous orchestrated recall.
    pub fn blocking_orchestrate_recall(
        &self,
        query: &str,
        top_k: usize,
    ) -> Vec<orchestrator::OrchestratedRecallResult> {
        self.db
            .with_conn(|conn| {
                orchestrator::orchestrate_recall(
                    conn,
                    query,
                    top_k,
                    &orchestrator::OrchestrateRecallOptions {
                        session_id: self.config.session_id.clone(),
                        ..Default::default()
                    },
                )
            })
            .expect("blocking_orchestrate_recall failed")
    }

    /// Run SHMR harmonization (cluster + belief generation).
    pub async fn harmonize(&self) -> Result<shmr::HarmonizeStats> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();

        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| shmr::harmonize(conn, &session_id, None, None, None))
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Synchronous SHMR harmonization.
    pub fn blocking_harmonize(&self) -> shmr::HarmonizeStats {
        self.db
            .with_conn(|conn| shmr::harmonize(conn, &self.config.session_id, None, None, None))
            .expect("blocking_harmonize failed")
    }

    /// Get session stats including consolidation info.
    pub fn blocking_session_stats(&self) -> session::SessionStats {
        self.db
            .with_conn(|conn| session::session_stats(conn, &self.config.session_id))
            .expect("blocking_session_stats failed")
    }

    /// Check whether auto-sleep should trigger.
    pub fn blocking_should_auto_sleep(&self, threshold: usize) -> bool {
        self.db
            .with_conn(|conn| {
                Ok(session::should_auto_sleep(
                    conn,
                    &self.config.session_id,
                    threshold,
                ))
            })
            .unwrap_or(false)
    }

    /// Get the database path (None if in-memory).
    pub fn db_path(&self) -> Option<&Path> {
        self.db.db_path.as_deref()
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &MnemopiConfig {
        &self.config
    }
}
