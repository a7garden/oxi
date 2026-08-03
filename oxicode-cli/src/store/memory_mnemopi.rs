//! Mnemopi-backed memory store implementing [`MemoryBackend`].
//!
//! Bridges the `oxicode_mnemopi` engine to the agent tool contract. This is the
//! production memory backend with FTS5 full-text search — replaces the simpler
//! `SqliteMemoryStore` (LIKE search) and `MnemopiStore` (JSON file) when
//! `mnemopi_engine` is enabled in settings.

use oxicode_agent::tools::{MemoryBackend, MemoryItem, ToolError};
use oxicode_mnemopi::{EmbeddingProvider, Mnemopi, MnemopiConfig, RecallOptions, RememberOptions};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

///
/// Wraps an [`oxicode_mnemopi::Mnemopi`] engine and exposes it through the
/// [`MemoryBackend`] trait used by the `memory_*` agent tools.
#[derive(Debug)]
pub struct MnemopiMemoryBackend {
    engine: Mnemopi,
}

impl MnemopiMemoryBackend {
    /// Open or create a Mnemopi-backed memory store at `path`.
    ///
    /// `embedding_provider` is the optional dense-vector model. When
    /// `Some`, `Mnemopi::remember`/`recall` will auto-embed every stored
    /// fact and recall query, activating the dense cosine-similarity
    /// signal of the hybrid scoring formula. When `None`, recall runs in
    /// FTS5-only mode.
    ///
    /// `embedding_model_name` is the logical model identifier recorded
    /// alongside each stored embedding (used for cache keying and
    /// diagnostics). Pass an empty string when no provider is wired.
    pub fn open(
        path: &Path,
        session_id: &str,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
        embedding_model_name: &str,
    ) -> Result<Self, String> {
        let mut config = MnemopiConfig {
            session_id: session_id.to_string(),
            ..Default::default()
        };
        config.embedding_provider = embedding_provider;
        if !embedding_model_name.is_empty() {
            config.embedding_model = Some(embedding_model_name.to_string());
        }
        let engine = Mnemopi::open(path, config).map_err(|e| format!("mnemopi open: {e}"))?;
        Ok(Self { engine })
    }
}

impl MnemopiMemoryBackend {
    /// Get a reference to the underlying engine.
    pub fn engine(&self) -> &Mnemopi {
        &self.engine
    }

    /// Run sleep consolidation.
    pub async fn sleep(&self, ttl_hours: i64, dry_run: bool) -> Result<(), String> {
        self.engine
            .sleep(ttl_hours, dry_run)
            .await
            .map(|_| ())
            .map_err(|e| format!("mnemopi sleep: {e}"))
    }

    /// Run SHMR harmonization.
    pub async fn harmonize(&self) -> Result<String, String> {
        self.engine
            .harmonize()
            .await
            .map(|stats| {
                format!(
                    "clusters={}, beliefs={}, contradictions={}, harmony={:.4}, status={}",
                    stats.clusters_found,
                    stats.beliefs_generated,
                    stats.contradictions_resolved,
                    stats.harmony_score_avg,
                    stats.status
                )
            })
            .map_err(|e| format!("mnemopi harmonize: {e}"))
    }

    /// Get session stats (synchronous).
    pub fn stats(&self) -> oxicode_mnemopi::session::SessionStats {
        self.engine.blocking_session_stats()
    }

    /// Check if auto-sleep should trigger.
    pub fn should_auto_sleep(&self, threshold: usize) -> bool {
        self.engine.blocking_should_auto_sleep(threshold)
    }

    /// Run auto-sleep if threshold is exceeded.
    pub async fn maybe_auto_sleep(&self, threshold: usize) -> Result<bool, String> {
        if self.should_auto_sleep(threshold) {
            self.sleep(24, false).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl MemoryBackend for MnemopiMemoryBackend {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // Sanitize content (strip data URIs, high-entropy blobs)
            let (sanitized, _blob_meta) =
                oxicode_mnemopi::content_sanitizer::sanitize_content(content);

            let options = RememberOptions {
                source: Some(subject.to_string()),
                memory_type: Some(kind.to_string()),
                ..Default::default()
            };
            let id = self
                .engine
                .remember(&sanitized, options)
                .await
                .map_err(|e| format!("mnemopi put: {e}"))?;

            // block_in_place allows blocking_lock inside Mnemopi
            // from an async context on multi-thread tokio runtimes.
            if tokio::runtime::Handle::try_current().is_ok()
                && tokio::task::block_in_place(|| self.engine.blocking_should_auto_sleep(200))
                && let Err(e) = self.engine.sleep(24, false).await
            {
                tracing::debug!("mnemopi auto-sleep failed: {e}");
            }

            Ok(id)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let results = self
                .engine
                .recall(
                    query,
                    RecallOptions {
                        limit: Some(k),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| format!("mnemopi search: {e}"))?;

            Ok(results
                .into_iter()
                .map(|r| MemoryItem {
                    id: r.id,
                    kind: "fact".to_string(),
                    content: r.content,
                    subject: r.source.unwrap_or_default(),
                })
                .collect())
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let rows = self
                .engine
                .list_by_source(subject, 100)
                .await
                .map_err(|e| format!("mnemopi list: {e}"))?;

            Ok(rows
                .into_iter()
                .map(|r| MemoryItem {
                    id: r.id,
                    kind: r.memory_type.unwrap_or_else(|| "fact".to_string()),
                    content: r.content,
                    subject: r.source.unwrap_or_default(),
                })
                .collect())
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move {
            self.engine
                .forget(id)
                .await
                .map_err(|e| format!("mnemopi delete: {e}"))?;
            Ok(())
        })
    }

    /// Bulk erase via the underlying SQLite connection: working_memory,
    /// episodic_memory, and memory_embeddings. The FTS5 indices are
    /// rebuilt by their existing triggers when rows are removed.
    /// Uses `Mnemopi::spawn_blocking` so the call is safe from any
    /// async context (the closure runs on the blocking pool; the
    fn clear_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<usize, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            self.engine
                .spawn_blocking(|conn| {
                    let wm = conn
                        .execute("DELETE FROM working_memory", [])
                        .map_err(|e| {
                            oxicode_mnemopi::MnemopiError::Other(format!(
                                "clear working_memory: {e}"
                            ))
                        })?;
                    let em = conn
                        .execute("DELETE FROM episodic_memory", [])
                        .map_err(|e| {
                            oxicode_mnemopi::MnemopiError::Other(format!(
                                "clear episodic_memory: {e}"
                            ))
                        })?;
                    let me = conn
                        .execute("DELETE FROM memory_embeddings", [])
                        .map_err(|e| {
                            oxicode_mnemopi::MnemopiError::Other(format!(
                                "clear memory_embeddings: {e}"
                            ))
                        })?;
                    Ok::<usize, oxicode_mnemopi::MnemopiError>(wm + em + me)
                })
                .await
                .map_err(|e| format!("mnemopi clear_all: {e}"))
        })
    }

    /// Real consolidation trigger: run a synchronous sleep pass through
    /// the engine. The Mnemopi facade already owns the SQLite handle,
    /// so this is the correct level for the operation — no separate
    /// pipeline DB is needed for an immediate consolidation pass.
    fn enqueue_consolidation<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            self.engine
                .sleep(24, false)
                .await
                .map(|result| {
                    format!(
                        "consolidated {} → {} summaries (tier1→2={}, tier2→3={})",
                        result.items_consolidated,
                        result.summaries_created,
                        result.degradation.tier1_to_tier2,
                        result.degradation.tier2_to_tier3,
                    )
                })
                .map_err(|e| format!("mnemopi enqueue_consolidation: {e}"))
        })
    }

    fn memory_info(&self) -> Option<String> {
        let stats = self.engine.blocking_session_stats();
        let db = self
            .engine
            .db_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "in-memory".to_string());
        Some(format!(
            "Memory Engine (Mnemopi)\n\
             ├─ Working:       {}\n\
             ├─ Episodic:      {}\n\
             ├─ Unconsolidated: {}\n\
             ├─ Oldest pending: {}\n\
             ├─ Last consolidation: {}\n\
             └─ DB: {}",
            stats.working_count,
            stats.episodic_count,
            stats.unconsolidated_count,
            stats.oldest_unconsolidated.as_deref().unwrap_or("—"),
            stats.last_consolidation.as_deref().unwrap_or("never"),
            db,
        ))
    }

    fn trigger_consolidation(&self) -> Option<String> {
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.engine.blocking_sleep(24, false))
        } else {
            self.engine.blocking_sleep(24, false)
        };
        if result.summaries_created > 0 || result.status == "consolidated" {
            Some(format!(
                "✓ Consolidated {} memories → {} summaries. Degraded: tier1→2={}, tier2→3={}",
                result.items_consolidated,
                result.summaries_created,
                result.degradation.tier1_to_tier2,
                result.degradation.tier2_to_tier3,
            ))
        } else {
            Some(format!("No memories to consolidate ({})", result.status))
        }
    }

    fn trigger_harmonize(&self) -> Option<String> {
        let stats = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.engine.blocking_harmonize())
        } else {
            self.engine.blocking_harmonize()
        };
        Some(format!(
            "✓ Harmonized: clusters={}, beliefs={}, contradictions={}, harmony={:.4}, status={}",
            stats.clusters_found,
            stats.beliefs_generated,
            stats.contradictions_resolved,
            stats.harmony_score_avg,
            stats.status,
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_backend() -> (MnemopiMemoryBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            MnemopiMemoryBackend::open(&dir.path().join("mnemopi.db"), "default", None, "")
                .unwrap();
        (backend, dir)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn put_list_search_delete_roundtrip() {
        let (backend, _dir) = tmp_backend();
        let id = backend
            .put("alice prefers rust ownership", "fact", "alice")
            .await
            .unwrap();
        assert!(!id.is_empty());

        let items = backend.list("alice").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "alice prefers rust ownership");
        assert_eq!(items[0].subject, "alice");
        assert_eq!(items[0].kind, "fact");

        let results = backend.search("rust", 5).await.unwrap();
        assert!(
            !results.is_empty(),
            "FTS5 recall must surface the just-stored memory"
        );

        backend.delete(&id).await.unwrap();
        assert!(backend.list("alice").await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_scopes_per_subject() {
        let (backend, _dir) = tmp_backend();
        let a = backend.put("a1", "fact", "alice").await.unwrap();
        let b = backend.put("b1", "fact", "bob").await.unwrap();

        assert_eq!(backend.list("alice").await.unwrap().len(), 1);
        assert_eq!(backend.list("bob").await.unwrap().len(), 1);
        assert!(backend.list("nobody").await.unwrap().is_empty());

        backend.delete(&a).await.unwrap();
        backend.delete(&b).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn memory_info_describes_engine_state() {
        let (backend, _dir) = tmp_backend();
        // Sync call enters `parking_lot::Mutex::blocking_lock`; hop
        // to the blocking pool so it doesn't panic inside the runtime.
        let info = tokio::task::spawn_blocking(move || backend.memory_info())
            .await
            .unwrap()
            .expect("Mnemopi backend reports info");
        assert!(
            info.contains("Mnemopi"),
            "memory_info should advertise Mnemopi: {info}"
        );
        assert!(info.contains("Working:"));
        assert!(info.contains("Episodic:"));
        assert!(info.contains("DB:"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_all_wipes_working_memory_and_embeddings() {
        let (backend, _dir) = tmp_backend();
        backend.put("a1", "fact", "alice").await.unwrap();
        backend.put("a2", "fact", "alice").await.unwrap();
        backend.put("b1", "fact", "bob").await.unwrap();

        let removed = backend.clear_all().await.unwrap();
        assert!(removed >= 3, "expected >=3 rows cleared, got {removed}");
        assert!(backend.list("alice").await.unwrap().is_empty());
        assert!(backend.list("bob").await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_all_is_idempotent_on_empty_backend() {
        let (backend, _dir) = tmp_backend();
        let removed = backend.clear_all().await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enqueue_consolidation_runs_real_sleep_pass() {
        let (backend, _dir) = tmp_backend();
        // Pre-populate so sleep has something to do.
        for i in 0..4 {
            backend
                .put(&format!("note-{i}"), "fact", "alice")
                .await
                .unwrap();
        }
        let msg = backend
            .enqueue_consolidation()
            .await
            .expect("enqueue_consolidation should succeed");
        assert!(!msg.is_empty());
        assert!(
            msg.contains("consolidated") || msg.contains("summaries"),
            "expected a sleep result message, got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trigger_consolidation_returns_status_string() {
        let (backend, _dir) = tmp_backend();
        backend.put("seed", "fact", "alice").await.unwrap();
        let msg = backend
            .trigger_consolidation()
            .expect("Mnemopi backend exposes trigger_consolidation");
        assert!(!msg.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trigger_harmonize_returns_status_string() {
        let (backend, _dir) = tmp_backend();
        backend.put("seed", "fact", "alice").await.unwrap();
        let msg = backend
            .trigger_harmonize()
            .expect("Mnemopi backend exposes trigger_harmonize");
        assert!(
            msg.contains("Harmonized"),
            "trigger_harmonize should report the SHMR outcome: {msg}"
        );
    }
}
