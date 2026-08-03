//! oxicode-mnemopi — local SQLite vector memory engine.
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
pub mod journal;
pub mod llm;
pub mod mcp;
pub mod mmr;
pub mod orchestrator;
pub mod path_layout;
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
pub mod watcher;

pub use chat_normalize::{ExtractionRate, extraction_rate, normalize_batch, normalize_chat};
pub use db::MnemopiDb;
#[cfg(feature = "remote-embeddings")]
pub use embeddings::RemoteEmbeddingProvider;
pub use embeddings::{EmbeddingProvider, NoopEmbeddingProvider};
pub use error::{MnemopiError, Result};
#[cfg(feature = "remote-llm")]
pub use llm::RemoteLlmBackend;
pub use llm::{CompleteOptions, LlmBackend, NoopLlmBackend, StubLlmBackend};
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
    /// Attach an embedding provider. Vectors will be generated for every
    /// stored memory and every recall query, activating the dense signal
    /// of the hybrid scoring formula.
    ///
    /// `model_name` is recorded alongside stored embeddings and surfaced
    /// in diagnostics — choose a stable identifier (e.g.
    /// `"text-embedding-3-small"`).
    pub fn with_embedding_provider(
        mut self,
        provider: Arc<dyn EmbeddingProvider>,
        model_name: impl Into<String>,
    ) -> Self {
        self.config.embedding_provider = Some(provider);
        self.config.embedding_model = Some(model_name.into());
        self
    }

    /// Attach an LLM backend. When set, fact extraction at `remember` time
    /// and consolidation at `sleep` time route through this backend;
    /// otherwise the heuristic / algorithmic fallbacks run.
    ///
    /// The backend is stored as `Arc<dyn LlmBackend>` inside the config
    /// and cloned cheaply across `spawn_blocking` tasks.
    pub fn with_llm_backend(mut self, backend: Arc<dyn LlmBackend>) -> Self {
        self.config.llm_backend = Some(backend);
        self
    }

    /// Extract atomic facts from `text` using the configured extractor.
    ///
    /// When [`MnemopiConfig::llm_backend`] is set, returns an
    /// [`crate::extraction::LlmExtractor`] wrapped over that backend;
    /// otherwise returns the always-available [`crate::extraction::HeuristicExtractor`].
    /// Hosts can call this from a `remember` prelude to split a long
    /// user message into atomic memories before storing each separately.
    ///
    /// The extractor is cheap to construct (no allocations beyond the
    /// prompt template); callers should not cache it.
    pub fn extractor(&self) -> Box<dyn extraction::FactExtractor + Send> {
        use extraction::{HeuristicExtractor, LlmExtractor};
        match self.config.llm_backend.clone() {
            Some(backend) => Box::new(LlmExtractor::new(backend, LlmExtractor::default_prompt())),
            None => Box::new(HeuristicExtractor),
        }
    }

    /// Extract atomic facts from `text` and store each as its own working
    /// memory entry. The original `text` is NOT stored by this method —
    /// call `remember(text, options)` separately if you need both.
    ///
    /// Each extracted fact is stored with the `source` and `subject`
    /// propagated from `options` (or defaults to `"extracted"` /
    /// `"unknown"`). Importance and veracity come from the extractor
    /// output.
    ///
    /// Returns the IDs of the newly stored memories. Empty if the
    /// extractor returned no facts.
    pub async fn extract_and_remember(
        &self,
        text: &str,
        options: RememberOptions,
    ) -> Result<Vec<String>> {
        let extractor = self.extractor();
        let facts = extractor.extract(text)?;
        if facts.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(facts.len());
        for fact in facts {
            let mut fact_opts = RememberOptions::from(&fact);
            // Inherit source from the caller's options so extracted facts
            // land in the same scope as the original.
            fact_opts.source = options.source.clone();
            ids.push(self.remember(&fact.content, fact_opts).await?);
        }
        Ok(ids)
    }
    /// Compute the dense embedding for a piece of text using the configured
    /// provider. Returns `None` when no provider is wired, the provider is
    /// unavailable, or the embed call fails. Failure is non-fatal — the
    /// caller proceeds in FTS5-only mode.
    #[allow(dead_code)] // kept for external callers using blocking Mnemopi APIs
    fn auto_embed(&self, text: &str) -> Option<Vec<f32>> {
        let provider = self.config.embedding_provider.as_ref()?;
        if !provider.available() {
            return None;
        }
        match provider.embed(&[text.to_string()]) {
            Ok(v) if !v.is_empty() => v.into_iter().next(),
            Ok(_) => {
                eprintln!("mnemopi: embedder returned empty result");
                None
            }
            Err(e) => {
                eprintln!("mnemopi: embedder failed: {e}");
                None
            }
        }
    }
    /// Run a synchronous closure against the underlying SQLite connection on
    /// the blocking pool. Use this for module-level operations (triples,
    /// episodic graph, scratchpad) that don't involve embeddings — the
    /// facade's `remember`/`recall` already embed internally; calling those
    /// via `spawn_blocking` would bypass the embedding step.
    ///
    /// The closure receives a `&rusqlite::Connection` and runs on a worker
    /// thread, so it may block freely.
    pub async fn spawn_blocking<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.with_conn(f))
            .await
            .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }
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
    ///
    /// When an embedding provider is wired (see
    /// [`Mnemopi::with_embedding_provider`]), `content` is embedded before
    /// the SQLite write and the resulting vector is stored in
    /// `memory_embeddings`. Without a provider, the memory is stored
    /// without a vector (FTS5-only recall).
    pub async fn remember(&self, content: &str, options: RememberOptions) -> Result<String> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();
        let content = content.to_string();

        // Compute embedding INSIDE spawn_blocking — providers use
        // `reqwest::blocking` (or ONNX), which panics if invoked from
        // inside a tokio runtime. Cloning the `Arc<dyn EmbeddingProvider>`
        // is cheap; the embedding I/O runs on the blocking pool.
        let provider = self.config.embedding_provider.clone();
        let model_name = self.config.embedding_model.clone();

        tokio::task::spawn_blocking(move || {
            let embedding = provider.as_ref().and_then(|p| {
                if p.available() {
                    match p.embed(std::slice::from_ref(&content)) {
                        Ok(v) if !v.is_empty() => v.into_iter().next(),
                        Ok(_) => {
                            eprintln!("mnemopi: embedder returned empty result");
                            None
                        }
                        Err(e) => {
                            eprintln!("mnemopi: embedder failed: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            });

            let mut options = options;
            options.embedding = embedding;
            if options.embedding.is_some()
                && let Some(model) = model_name
            {
                let entry = options.metadata.get_or_insert_with(Metadata::new);
                entry.insert(
                    "embedding_model".to_string(),
                    serde_json::Value::String(model),
                );
            }
            db.with_conn(|conn| {
                store::remember(
                    conn,
                    &content,
                    &session_id,
                    &options,
                    options.embedding.as_deref(),
                )
            })
        })
        .await
        .map_err(|e| MnemopiError::Other(format!("join error: {e}")))?
    }

    /// Recall memories matching `query`.
    ///
    /// When an embedding provider is wired, the query is embedded and the
    /// dense cosine-similarity signal is activated for hybrid scoring.
    /// Without a provider, recall degenerates to FTS5 + keyword +
    /// importance + recency + veracity.
    pub async fn recall(&self, query: &str, options: RecallOptions) -> Result<Vec<RecallResult>> {
        let db = self.db.clone();
        let session_id = self.config.session_id.clone();
        let query = query.to_string();

        // Compute query embedding INSIDE spawn_blocking — providers use
        // `reqwest::blocking` (or ONNX), which panics if invoked from
        // inside a tokio runtime.
        let provider = self.config.embedding_provider.clone();

        tokio::task::spawn_blocking(move || {
            let query_embedding = provider.as_ref().and_then(|p| {
                if p.available() {
                    match p.embed(std::slice::from_ref(&query)) {
                        Ok(v) if !v.is_empty() => v.into_iter().next(),
                        Ok(_) => {
                            eprintln!("mnemopi: query embedder returned empty result");
                            None
                        }
                        Err(e) => {
                            eprintln!("mnemopi: query embedder failed: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            });

            let mut options = options;
            options.query_embedding = query_embedding;
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
            .with_conn(|conn| store::remember(conn, content, session_id, &options, None))
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

#[cfg(test)]
mod dream_tests {
    use super::*;
    use crate::llm::StubLlmBackend;

    #[tokio::test]
    async fn extract_and_remember_with_stub_llm_stores_each_fact() {
        let mnemopi = Mnemopi::open_in_memory().expect("open");
        let stub = StubLlmBackend {
            response: "User prefers dark mode | 0.9\nBuild uses cargo\n".into(),
            name: "stub".into(),
        };
        let mnemopi = mnemopi.with_llm_backend(Arc::new(stub));

        let ids = mnemopi
            .extract_and_remember(
                "ignored by stub",
                RememberOptions {
                    source: Some("test".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("extract_and_remember");

        assert_eq!(ids.len(), 2, "two facts should be stored");
        // Verify both memories are retrievable.
        let stats = mnemopi.get_stats().await.expect("stats");
        assert_eq!(stats.working_count, 2);
    }

    #[tokio::test]
    async fn extract_and_remember_without_llm_uses_heuristic() {
        let mnemopi = Mnemopi::open_in_memory().expect("open");
        // No llm_backend configured — should fall back to HeuristicExtractor.
        let ids = mnemopi
            .extract_and_remember(
                "The user prefers Vim. This is critically important.",
                RememberOptions::default(),
            )
            .await
            .expect("extract_and_remember");
        assert!(
            !ids.is_empty(),
            "heuristic should extract at least one fact"
        );
    }

    #[tokio::test]
    async fn extract_and_remember_empty_input_returns_empty() {
        let mnemopi = Mnemopi::open_in_memory().expect("open");
        let ids = mnemopi
            .extract_and_remember("", RememberOptions::default())
            .await
            .expect("extract_and_remember");
        assert!(ids.is_empty());
    }

    #[test]
    fn extractor_returns_heuristic_when_no_backend() {
        let mnemopi = Mnemopi::open_in_memory().expect("open");
        // Just verify it doesn't panic and produces some facts.
        let ext = mnemopi.extractor();
        let facts = ext.extract("Rust is a systems language.").expect("extract");
        assert!(!facts.is_empty());
    }
}
