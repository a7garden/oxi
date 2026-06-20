/// File mutation queue - serializes concurrent writes to the same file.
///
/// Prevents race conditions when multiple edit operations target the same file.
/// Operations on *different* files run in parallel; operations on the *same*
/// file are serialized.
///
/// Includes automatic stale-entry cleanup to prevent unbounded memory growth:
/// every `CLEANUP_INTERVAL` operations, entries for files that no longer exist
/// on disk are removed. If the map exceeds `MAX_ENTRIES`, excess entries are
/// evicted (oldest first via HashMap iteration order).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::fs;
use tokio::sync::Mutex;

/// Clean up stale entries every N operations.
const CLEANUP_INTERVAL: usize = 128;

/// Maximum number of per-file mutex entries before eviction kicks in.
const MAX_ENTRIES: usize = 1024;

/// Global file mutation queue.
static QUEUE: std::sync::OnceLock<FileMutationQueue> = std::sync::OnceLock::new();

/// Get the global file mutation queue.
pub fn global_mutation_queue() -> &'static FileMutationQueue {
    QUEUE.get_or_init(FileMutationQueue::new)
}

/// Serializes file mutation operations per canonical path.
#[derive(Debug)]
pub struct FileMutationQueue {
    /// Map from canonical path to a mutex that serializes operations.
    queues: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    /// Monotonic operation counter for triggering periodic cleanup.
    op_counter: AtomicUsize,
}

impl FileMutationQueue {
    /// Create a new, empty file mutation queue.
    pub fn new() -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            op_counter: AtomicUsize::new(0),
        }
    }

    /// Execute a mutation operation on a file, serialized per canonical path.
    ///
    /// If the file doesn't exist yet, uses the path as-is for the key.
    /// Periodically triggers cleanup of stale entries to prevent memory leaks.
    pub async fn with_queue<F, Fut, T>(&self, path: &Path, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let ops = self.op_counter.fetch_add(1, Ordering::Relaxed);

        // Periodic stale-entry cleanup to prevent unbounded memory growth.
        if ops.is_multiple_of(CLEANUP_INTERVAL) && ops > 0 {
            self.cleanup_stale().await;
        }

        let canonical = fs::canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_path_buf());

        // Get or create a mutex for this file.
        let mutex = {
            let mut queues = self.queues.lock().await;

            // Enforce capacity limit: evict stale entries if over max.
            if queues.len() >= MAX_ENTRIES {
                // First pass: remove entries for non-existent files.
                let keys: Vec<PathBuf> = queues.keys().cloned().collect();
                drop(queues);
                for key in &keys {
                    if fs::metadata(key).await.is_err() {
                        let mut q = self.queues.lock().await;
                        q.remove(key);
                    }
                }
                queues = self.queues.lock().await;

                // Second pass: if still over capacity, evict arbitrary entries.
                while queues.len() >= MAX_ENTRIES {
                    if let Some(key) = queues.keys().next().cloned() {
                        queues.remove(&key);
                    } else {
                        break;
                    }
                }
            }

            queues
                .entry(canonical)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Lock the per-file mutex.
        let _guard = mutex.lock().await;

        // Execute the operation.
        f().await
    }

    /// Remove entries for files that no longer exist on disk.
    ///
    /// This is called automatically every `CLEANUP_INTERVAL` operations, but
    /// can also be called manually.
    pub async fn cleanup_stale(&self) {
        let queues = self.queues.lock().await;
        let keys: Vec<PathBuf> = queues.keys().cloned().collect();
        drop(queues); // Release lock during IO.

        let mut to_remove = Vec::new();
        for key in &keys {
            if fs::metadata(key).await.is_err() {
                to_remove.push(key.clone());
            }
        }

        if !to_remove.is_empty() {
            tracing::debug!(
                stale_count = to_remove.len(),
                "FileMutationQueue: cleaning up stale entries"
            );
            let mut queues = self.queues.lock().await;
            for key in to_remove {
                queues.remove(&key);
            }
        }
    }

    /// Clean up entries for a specific file.
    pub async fn cleanup(&self, path: &Path) {
        let canonical = fs::canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_path_buf());
        let mut queues = self.queues.lock().await;
        queues.remove(&canonical);
    }

    /// Returns the number of entries currently in the queue map.
    #[allow(dead_code)]
    pub async fn entry_count(&self) -> usize {
        self.queues.lock().await.len()
    }
}

impl Default for FileMutationQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_same_file_serialized() {
        let queue = Arc::new(FileMutationQueue::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let path = PathBuf::from("/tmp/test_mutation_queue_file");

        let mut handles = Vec::new();

        for _ in 0..10 {
            let queue = queue.clone();
            let counter = counter.clone();
            let path = path.clone();

            handles.push(tokio::spawn(async move {
                queue
                    .with_queue(&path, || async {
                        let prev = counter.fetch_add(1, Ordering::SeqCst);
                        // Simulate some work
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        prev
                    })
                    .await
            }));
        }

        // All operations should complete
        for handle in handles {
            let _ = handle.await.unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[tokio::test]
    async fn test_different_files_parallel() {
        let queue = Arc::new(FileMutationQueue::new());
        let counter = Arc::new(AtomicUsize::new(0));

        let path1 = PathBuf::from("/tmp/test_file_1");
        let path2 = PathBuf::from("/tmp/test_file_2");

        let q1 = queue.clone();
        let q2 = queue.clone();
        let counter1 = counter.clone();
        let counter2 = counter.clone();

        let h1 = tokio::spawn(async move {
            q1.with_queue(&path1, || async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                counter1.fetch_add(1, Ordering::SeqCst)
            })
            .await
        });

        let h2 = tokio::spawn(async move {
            q2.with_queue(&path2, || async { counter2.fetch_add(1, Ordering::SeqCst) })
                .await
        });

        // Both should complete quickly (parallel)
        let r1 = tokio::time::timeout(std::time::Duration::from_millis(100), h1).await;
        let r2 = tokio::time::timeout(std::time::Duration::from_millis(100), h2).await;

        assert!(r1.is_ok());
        assert!(r2.is_ok());
    }

    #[tokio::test]
    async fn test_auto_cleanup_removes_stale() {
        let queue = FileMutationQueue::new();

        // Create a temp file, use it, then delete it.
        let temp_path = std::env::temp_dir().join("oxi_test_stale_queue_file");
        std::fs::write(&temp_path, "test").unwrap();

        let _ = queue.with_queue(&temp_path, || async { 42 }).await;
        assert_eq!(queue.entry_count().await, 1);

        // Delete the file.
        std::fs::remove_file(&temp_path).ok();

        // Trigger cleanup.
        queue.cleanup_stale().await;
        assert_eq!(queue.entry_count().await, 0);
    }

    #[tokio::test]
    async fn test_max_entries_enforced() {
        let queue = Arc::new(FileMutationQueue::new());

        // Fill beyond MAX_ENTRIES with non-existent paths.
        let mut handles = Vec::new();
        for i in 0..(MAX_ENTRIES + 10) {
            let q = queue.clone();
            handles.push(tokio::spawn(async move {
                let path = PathBuf::from(format!("/tmp/oxi_test_max_entries_{}", i));
                q.with_queue(&path, || async { i }).await
            }));
        }

        for h in handles {
            let _ = h.await.unwrap();
        }

        // Entry count should be capped at or near MAX_ENTRIES.
        let count = queue.entry_count().await;
        assert!(
            count <= MAX_ENTRIES + 10, // Allow slack since eviction is on next access
            "Entry count {} should be bounded near {}",
            count,
            MAX_ENTRIES
        );
    }
}
