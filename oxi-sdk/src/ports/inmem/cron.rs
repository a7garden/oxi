//! Simple `CronScheduler` — in-memory job registry with placeholder scheduling.
//!
//! The implementation intentionally focuses on **registration and
//! introspection**, not wall-clock firing. A production scheduler would
//! drive firing via a separate background task. This impl is sufficient
//! for tests and for products that drive cron from their own loop.

use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;

use crate::ports::{CronJob, CronScheduler};
use crate::SdkError;

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
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl CronScheduler for InMemoryCronScheduler {
    async fn register(&self, job: CronJob) -> Result<(), SdkError> {
        self.jobs.lock().insert(job.id.clone(), job);
        Ok(())
    }

    async fn unregister(&self, id: &str) -> Result<(), SdkError> {
        self.jobs.lock().remove(id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<CronJob>, SdkError> {
        Ok(self.jobs.lock().values().cloned().collect())
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
