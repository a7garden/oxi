//! SDK-provided [`MemoryBackend`] adapter bridging the SDK's
//! [`MemoryStore`] + optional [`EmbeddingProvider`] ports into the agent's
//! `memory_*` tools.
//!
//! This is the memory analog of [`crate::url_resolver::SdkUrlResolver`]: the SDK ships a
//! standard adapter so a *pure*-`oxicode-sdk` consumer (one that does not write a
//! bespoke `MemoryBackend`, as oxicode-cli does with its Mnemopi backend) can
//! make the agent's memory tools functional simply by registering the ports.
//!
//! # Why this exists
//!
//! `oxicode-agent`'s [`MemoryBackend`] (the trait the `memory_*` tools call) and
//! the SDK's [`MemoryStore`] port are *different* traits with different shapes
//! — the port is the SDK consumer's storage contract; the backend is the
//! agent tool contract. They do not map 1:1, so an adapter is required.
//! Without it, `OxicodeBuilder::with_memory(store)` stores a port that the agent
//! loop never reads (the agent uses `ToolContext.memory`, which stayed
//! `None`). This adapter closes that gap.
//!
//! # Wiring
//!
//! Register the ports on the engine, then bridge them onto the agent:
//!
//! ```no_run
//! use std::sync::Arc;
//! use oxicode_sdk::{OxicodeBuilder, inmem::InMemoryMemoryStore};
//!
//! let oxicode = OxicodeBuilder::new()
//!     .with_builtins()
//!     .with_memory(Arc::new(InMemoryMemoryStore::new()))
//!     .build();
//! let agent = oxicode.agent(oxicode_agent::AgentConfig {
//!     model_id: "anthropic/claude-sonnet-4-20250514".into(),
//!     ..Default::default()
//! })
//! .with_port_memory() // bridges MemoryStore (+ EmbeddingProvider) → tools
//! .build()
//! .unwrap();
//! ```
//!
//! Without an [`EmbeddingProvider`], `put` / `list` / `delete` work but
//! semantic `search` returns an error (the port's search is vector-based).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use oxicode_agent::tools::{MemoryBackend, MemoryItem, ToolError};

use crate::ports::{EmbeddingProvider, MemoryEntry, MemoryStore};

/// Adapter that exposes an SDK [`MemoryStore`] (plus an optional
/// [`EmbeddingProvider`]) as an [`oxicode_agent::tools::MemoryBackend`] so the
/// agent's `memory_*` tools can read/write it.
///
/// Construct directly ([`Self::new`] / [`Self::from_ports`]) or bridge it
/// onto an agent via [`crate::AgentBuilder::with_port_memory`].
pub struct PortMemoryBackend {
    store: Arc<dyn MemoryStore>,
    embeddings: Option<Arc<dyn EmbeddingProvider>>,
}

impl PortMemoryBackend {
    /// Wrap a [`MemoryStore`] with no embedding provider.
    ///
    /// `put` / `list` / `delete` work; semantic `search` is unavailable until
    /// an [`EmbeddingProvider`] is attached via [`Self::with_embeddings`].
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self {
            store,
            embeddings: None,
        }
    }

    /// Attach an embedding provider, enabling semantic search.
    #[must_use]
    pub fn with_embeddings(mut self, embeddings: Arc<dyn EmbeddingProvider>) -> Self {
        self.embeddings = Some(embeddings);
        self
    }

    /// Build from both ports at once.
    ///
    /// Pass `None` for `embeddings` to disable semantic search (the port's
    /// `search` is vector-based and cannot run without embeddings).
    #[must_use]
    pub fn from_ports(
        store: Arc<dyn MemoryStore>,
        embeddings: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self { store, embeddings }
    }
}

impl std::fmt::Debug for PortMemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortMemoryBackend")
            .field("store", &"<dyn MemoryStore>")
            .field("has_embeddings", &self.embeddings.is_some())
            .finish()
    }
}

/// Recover a `String` from a stored [`PortValue`](serde_json::Value).
///
/// Round-trips losslessly: [`PortMemoryBackend`] always stores content as
/// [`serde_json::Value::String`], which this recovers directly. Other JSON
/// variants (only possible if a foreign writer touched the store) serialize
/// back to text.
fn content_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Map a stored [`MemoryEntry`] to the agent-tool [`MemoryItem`].
fn entry_to_item(e: MemoryEntry) -> MemoryItem {
    MemoryItem {
        id: e.id,
        kind: e.kind,
        content: content_to_string(&e.content),
        subject: e.subject,
    }
}

impl MemoryBackend for PortMemoryBackend {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let id = uuid::Uuid::new_v4().to_string();
            let embedding = if let Some(emb) = &self.embeddings {
                Some(emb.embed(content).await.map_err(|e| e.to_string())?)
            } else {
                None
            };
            let entry = MemoryEntry {
                id: id.clone(),
                subject: subject.to_string(),
                kind: kind.to_string(),
                embedding,
                content: serde_json::Value::String(content.to_string()),
                created_at: chrono::Utc::now(),
            };
            self.store.put(entry).await.map_err(|e| e.to_string())?;
            Ok(id)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(emb) = &self.embeddings else {
                return Err(
                    "Semantic search requires an EmbeddingProvider. Register one via \
                     OxicodeBuilder::with_embeddings() (or PortMemoryBackend::with_embeddings) \
                     to enable memory_recall."
                        .to_string(),
                );
            };
            let vec = emb.embed(query).await.map_err(|e| e.to_string())?;
            let entries = self
                .store
                .search(&vec, k)
                .await
                .map_err(|e| e.to_string())?;
            Ok(entries.into_iter().map(entry_to_item).collect())
        })
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let entries = self.store.list(subject).await.map_err(|e| e.to_string())?;
            Ok(entries.into_iter().map(entry_to_item).collect())
        })
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        Box::pin(async move { self.store.delete(id).await.map_err(|e| e.to_string()) })
    }

    fn memory_info(&self) -> Option<String> {
        let mode = if self.embeddings.is_some() {
            "semantic"
        } else {
            "store-only (no embeddings → search disabled)"
        };
        Some(format!("PortMemoryBackend ({mode})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmem::InMemoryMemoryStore;

    fn backend() -> PortMemoryBackend {
        PortMemoryBackend::new(Arc::new(InMemoryMemoryStore::new()))
    }

    #[tokio::test]
    async fn put_list_delete_roundtrip() {
        let b = backend();
        let id = b.put("likes rust", "preference", "user-1").await.unwrap();
        let listed = b.list("user-1").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].content, "likes rust");
        assert_eq!(listed[0].kind, "preference");
        b.delete(&id).await.unwrap();
        assert!(b.list("user-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_without_embeddings_errors_clearly() {
        let b = backend();
        b.put("some fact", "fact", "s").await.unwrap();
        let err = b.search("fact", 5).await.unwrap_err();
        assert!(
            err.contains("EmbeddingProvider"),
            "expected guidance about embeddings, got: {err}"
        );
    }

    #[test]
    fn memory_info_reflects_embeddings_state() {
        let b = backend();
        assert!(b.memory_info().unwrap().contains("store-only"));
    }
}
