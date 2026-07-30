//! In-memory `MemoryStore` for tests and ephemeral sessions.
//!
//! Use this when:
//! - You need an in-process memory store with no I/O
//! - You're writing tests and want a fast fake
//! - You're prototyping and persistence doesn't matter yet
//!
//! For durable memory, implement `oxi_sdk::ports::MemoryStore` against
//! SQLite/Redis/vector DB.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::SdkError;
use crate::ports::{MemoryEntry, MemoryStore};

/// Thread-safe in-memory memory store.
pub struct InMemoryMemoryStore {
    inner: Mutex<HashMap<String, MemoryEntry>>,
}

impl std::fmt::Debug for InMemoryMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryMemoryStore").finish()
    }
}

impl Default for InMemoryMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryMemoryStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl MemoryStore for InMemoryMemoryStore {
    fn put(
        &self,
        entry: MemoryEntry,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        self.inner.lock().insert(entry.id.clone(), entry);
        Box::pin(async { Ok(()) })
    }

    fn list(
        &self,
        subject: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + '_>> {
        let g = self.inner.lock();
        let result: Vec<MemoryEntry> = g
            .values()
            .filter(|e| e.subject == subject)
            .cloned()
            .collect();
        Box::pin(async { Ok(result) })
    }

    fn search(
        &self,
        query: &[f32],
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryEntry>, SdkError>> + Send + '_>> {
        let g = self.inner.lock();
        let mut scored: Vec<_> = g
            .values()
            .filter_map(|e| e.embedding.as_ref().map(|emb| (e, cosine(query, emb))))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let result: Vec<MemoryEntry> = scored.into_iter().take(k).map(|(e, _)| e.clone()).collect();
        Box::pin(async { Ok(result) })
    }
    fn delete(&self, id: &str) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        self.inner.lock().remove(id);
        Box::pin(async { Ok(()) })
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, subject: &str, emb: Option<Vec<f32>>) -> MemoryEntry {
        MemoryEntry {
            id: id.into(),
            subject: subject.into(),
            kind: "episodic".into(),
            embedding: emb,
            content: json!({}),
            created_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn put_and_list() {
        let s = InMemoryMemoryStore::new();
        s.put(entry("a", "agent-1", None)).await.unwrap();
        s.put(entry("b", "agent-2", None)).await.unwrap();
        s.put(entry("c", "agent-1", None)).await.unwrap();
        let list = s.list("agent-1").await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn cosine_search_returns_top_k() {
        let s = InMemoryMemoryStore::new();
        s.put(entry("a", "x", Some(vec![1.0, 0.0, 0.0])))
            .await
            .unwrap();
        s.put(entry("b", "x", Some(vec![0.0, 1.0, 0.0])))
            .await
            .unwrap();
        s.put(entry("c", "x", Some(vec![0.9, 0.1, 0.0])))
            .await
            .unwrap();
        let top = s.search(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].id, "a");
        assert_eq!(top[1].id, "c");
    }
}
