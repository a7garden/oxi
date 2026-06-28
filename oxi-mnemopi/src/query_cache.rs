//! LRU query embedding cache — ported from omp `embeddings.ts` queryCache.
//!
//! Caches query → embedding mappings to avoid redundant API/ONNX calls
//! for repeated queries. Thread-safe via `parking_lot::Mutex`.

use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;

/// Default cache capacity (matches omp `QUERY_CACHE_MAX = 512`).
const DEFAULT_CAPACITY: usize = 512;

/// Thread-safe LRU cache for embedding queries.
pub struct QueryCache {
    cache: Mutex<LruCache<String, Vec<f32>>>,
}

impl QueryCache {
    /// Create a cache with the default capacity (512 entries).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a cache with a custom capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(512).unwrap()),
            )),
        }
    }

    /// Look up a cached embedding.
    pub fn get(&self, key: &str) -> Option<Vec<f32>> {
        self.cache.lock().get(key).cloned()
    }

    /// Insert a new embedding into the cache.
    pub fn put(&self, key: String, value: Vec<f32>) {
        self.cache.lock().put(key, value);
    }

    /// Get or compute an embedding.
    ///
    /// If the key is cached, returns the cached value. Otherwise calls `f`
    /// to compute the embedding, stores it, and returns it.
    pub fn get_or_compute<F>(&self, key: &str, f: F) -> crate::error::Result<Vec<f32>>
    where
        F: FnOnce() -> crate::error::Result<Vec<f32>>,
    {
        if let Some(cached) = self.get(key) {
            return Ok(cached);
        }
        let value = f()?;
        self.put(key.to_string(), value.clone());
        Ok(value)
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCache")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_miss() {
        let cache = QueryCache::with_capacity(4);
        assert!(cache.get("test").is_none());
        cache.put("test".to_string(), vec![1.0, 2.0, 3.0]);
        assert_eq!(cache.get("test"), Some(vec![1.0, 2.0, 3.0]));
    }

    #[test]
    fn cache_eviction() {
        let cache = QueryCache::with_capacity(2);
        cache.put("a".to_string(), vec![1.0]);
        cache.put("b".to_string(), vec![2.0]);
        cache.put("c".to_string(), vec![3.0]);
        // "a" should be evicted (LRU)
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn get_or_compute_caches() {
        let cache = QueryCache::new();
        let mut call_count = 0;
        let result = cache
            .get_or_compute("test", || {
                call_count += 1;
                Ok(vec![1.0, 2.0])
            })
            .unwrap();
        assert_eq!(result, vec![1.0, 2.0]);

        // Second call should hit cache
        let result2 = cache
            .get_or_compute("test", || {
                call_count += 1;
                Ok(vec![3.0, 4.0])
            })
            .unwrap();
        assert_eq!(result2, vec![1.0, 2.0]); // cached value, not recomputed
        assert_eq!(call_count, 1);
    }
}
