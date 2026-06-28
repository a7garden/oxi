//! Mnemopi-backed memory store implementing [`MemoryBackend`].
//!
//! Bridges the `oxi_mnemopi` engine to the agent tool contract. This is the
//! production memory backend with FTS5 full-text search — replaces the simpler
//! `SqliteMemoryStore` (LIKE search) and `MnemopiStore` (JSON file) when
//! `mnemopi_engine` is enabled in settings.

use std::path::Path;
use std::pin::Pin;

use oxi_agent::tools::{MemoryBackend, MemoryItem, ToolError};
use oxi_mnemopi::{Mnemopi, MnemopiConfig, RecallOptions, RememberOptions};

/// Mnemopi-backed memory backend.
///
/// Wraps an [`oxi_mnemopi::Mnemopi`] engine and exposes it through the
/// [`MemoryBackend`] trait used by the `memory_*` agent tools.
#[derive(Debug)]
pub struct MnemopiMemoryBackend {
    engine: Mnemopi,
}

impl MnemopiMemoryBackend {
    /// Open or create a Mnemopi-backed memory store at `path`.
    pub fn open(path: &Path, session_id: &str) -> Result<Self, String> {
        let config = MnemopiConfig {
            session_id: session_id.to_string(),
            ..Default::default()
        };
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
    pub fn stats(&self) -> oxi_mnemopi::session::SessionStats {
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
            let (sanitized, _blob_meta) = oxi_mnemopi::content_sanitizer::sanitize_content(content);

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

            if self.engine.blocking_should_auto_sleep(200)
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
        let result = self.engine.blocking_sleep(24, false);
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
        let stats = self.engine.blocking_harmonize();
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
