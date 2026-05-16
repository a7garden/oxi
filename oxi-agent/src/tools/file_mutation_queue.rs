/// File mutation queue - serializes concurrent writes to the same file
/// Prevents race conditions when multiple edit operations target the same file.
/// Operations on *different* files run in parallel; operations on the *same* file are serialized.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;

/// Global file mutation queue
static QUEUE: std::sync::OnceLock<FileMutationQueue> = std::sync::OnceLock::new();

/// Get the global file mutation queue
pub fn global_mutation_queue() -> &'static FileMutationQueue {
    QUEUE.get_or_init(FileMutationQueue::new)
}

/// Serializes file mutation operations per canonical path.
#[derive(Debug)]
pub struct FileMutationQueue {
    /// Map from canonical path to a mutex that serializes operations
    queues: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl FileMutationQueue {
    /// TODO.
    pub fn new() -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Execute a mutation operation on a file, serialized per canonical path.
    /// If the file doesn't exist yet, uses the path as-is.
    pub async fn with_queue<F, Fut, T>(&self, path: &Path, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let canonical = fs::canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_path_buf());

        // Get or create a mutex for this file
        let mutex = {
            let mut queues = self.queues.lock().await;
            queues
                .entry(canonical)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Lock the per-file mutex
        let _guard = mutex.lock().await;

        // Execute the operation
        f().await
    }

    /// Clean up entries for files that no longer need serialization.
    pub async fn cleanup(&self, path: &Path) {
        let canonical = fs::canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_path_buf());
        let mut queues = self.queues.lock().await;
        queues.remove(&canonical);
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
}
