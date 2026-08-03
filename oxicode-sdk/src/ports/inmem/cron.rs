//! Simple `CronScheduler` — in-memory job registry with placeholder scheduling.
//!
//! The implementation intentionally focuses on **registration and
//! introspection**, not wall-clock firing. A production scheduler would
//! drive firing via a separate background task. This impl is sufficient
//! for tests and for products that drive cron from their own loop.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::SdkError;
use crate::ports::{CronJob, CronScheduler};

/// In-process `CronScheduler` that stores jobs in a `HashMap` behind a mutex.
///
/// See the module docs: this implementation registers and lists jobs but
/// does not fire them on a wall-clock schedule — callers drive firing
/// themselves.
pub struct InMemoryCronScheduler {
    jobs: Mutex<HashMap<String, CronJob>>,
}

impl std::fmt::Debug for InMemoryCronScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryCronScheduler").finish()
    }
}

impl Default for InMemoryCronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryCronScheduler {
    /// Create an empty scheduler.
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

impl CronScheduler for InMemoryCronScheduler {
    fn register(
        &self,
        job: CronJob,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        self.jobs.lock().insert(job.id.clone(), job);
        Box::pin(async { Ok(()) })
    }

    fn unregister(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + '_>> {
        self.jobs.lock().remove(id);
        Box::pin(async { Ok(()) })
    }

    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<CronJob>, SdkError>> + Send + '_>> {
        let jobs = self.jobs.lock().values().cloned().collect();
        Box::pin(async { Ok(jobs) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_list_unregister() {
        let s = InMemoryCronScheduler::new();
        s.register(CronJob {
            id: "j1".into(),
            schedule: "*/5 * * * *".into(),
            action: "tick".into(),
            payload: None,
        })
        .await
        .unwrap();
        let list = s.list().await.unwrap();
        assert_eq!(list.len(), 1);
        s.unregister("j1").await.unwrap();
        assert!(s.list().await.unwrap().is_empty());
    }
}
