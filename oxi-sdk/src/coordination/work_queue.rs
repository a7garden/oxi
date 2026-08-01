//! Work queue — distributed task queue for inter-agent coordination.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;

/// Unique work item ID type.
type WorkId = String;

/// A unit of work in the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    /// Unique identifier.
    pub id: WorkId,
    /// Work type (for filtering).
    pub work_type: String,
    /// Payload for the worker.
    pub payload: serde_json::Value,
    /// Priority (higher = more urgent).
    pub priority: i32,
    /// Current status.
    pub status: WorkStatus,
    /// Agent that claimed this item.
    pub claimed_by: Option<String>,
    /// Execution result.
    pub result: Option<WorkResult>,
    /// Creation timestamp.
    pub created_at_ms: u64,
    /// Claim timestamp.
    pub claimed_at_ms: Option<u64>,
    /// Completion timestamp.
    pub completed_at_ms: Option<u64>,
    /// Maximum retries.
    pub max_retries: usize,
    /// Current retry count.
    pub retry_count: usize,
}

/// Status of a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkStatus {
    /// Waiting to be claimed by a worker.
    Pending,
    /// Atomically claimed by an agent but not yet started.
    Claimed,
    /// Currently being executed by an agent.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Finished unsuccessfully after exhausting retries.
    Failed,
    /// Cancelled before or during execution.
    Cancelled,
}

/// Result of work item execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkResult {
    /// Whether the work completed without error.
    pub success: bool,
    /// Output produced by the worker.
    pub content: String,
    /// Error message if the work failed.
    pub error: Option<String>,
    /// Wall-clock execution time in milliseconds.
    pub duration_ms: u64,
    /// Model tokens consumed, if tracked.
    pub tokens_used: Option<u64>,
}

/// Events emitted by the work queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkEvent {
    /// A work item was added to the queue.
    Enqueued {
        /// Identifier of the work item.
        id: WorkId,
        /// Type category of the work item.
        work_type: String,
    },
    /// A work item was claimed by an agent.
    Claimed {
        /// Identifier of the work item.
        id: WorkId,
        /// Identifier of the claiming agent.
        agent_id: String,
    },
    /// Claimed work began executing.
    Started {
        /// Identifier of the work item.
        id: WorkId,
    },
    /// A work item finished.
    Completed {
        /// Identifier of the work item.
        id: WorkId,
        /// Whether the work succeeded.
        success: bool,
    },
    /// A work item was cancelled.
    Cancelled {
        /// Identifier of the work item.
        id: WorkId,
    },
}

/// Queue statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkQueueStats {
    /// Number of items waiting to be claimed.
    pub pending: usize,
    /// Number of items claimed but not started.
    pub claimed: usize,
    /// Number of items currently executing.
    pub in_progress: usize,
    /// Number of items finished successfully.
    pub completed: usize,
    /// Number of items that failed.
    pub failed: usize,
    /// Number of items cancelled.
    pub cancelled: usize,
}

/// Configuration for the work queue.
#[derive(Debug, Clone)]
pub struct WorkQueueConfig {
    /// Maximum items in the queue before eviction.
    pub max_items: usize,
}

impl Default for WorkQueueConfig {
    fn default() -> Self {
        Self { max_items: 10_000 }
    }
}

/// In-memory work queue with priority-based atomic claim.
pub struct WorkQueue {
    items: Arc<RwLock<HashMap<WorkId, WorkItem>>>,
    next_id: AtomicU64,
    #[allow(dead_code)]
    config: WorkQueueConfig,
    tx: broadcast::Sender<WorkEvent>,
}

impl WorkQueue {
    /// Create a new work queue.
    pub fn new(config: WorkQueueConfig) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            items: Arc::new(RwLock::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            config,
            tx,
        }
    }

    /// Enqueue a new work item. Returns its ID.
    pub fn enqueue(
        &self,
        work_type: impl Into<String>,
        payload: serde_json::Value,
        priority: i32,
    ) -> WorkId {
        let id = format!("wq-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let item = WorkItem {
            id: id.clone(),
            work_type: work_type.into(),
            payload,
            priority,
            status: WorkStatus::Pending,
            claimed_by: None,
            result: None,
            created_at_ms: now_ms(),
            claimed_at_ms: None,
            completed_at_ms: None,
            max_retries: 3,
            retry_count: 0,
        };
        self.items.write().insert(id.clone(), item);
        let _ = self.tx.send(WorkEvent::Enqueued {
            id: id.clone(),
            work_type: String::new(),
        });
        id
    }

    /// Atomically claim the highest-priority pending item.
    ///
    /// If `work_type_filter` is provided, only items of those types are considered.
    /// Returns `None` if no items are available.
    pub fn claim(&self, agent_id: &str, work_type_filter: Option<&[String]>) -> Option<WorkItem> {
        let mut items = self.items.write();
        let mut best: Option<(WorkId, i32)> = None;

        for (id, item) in items.iter() {
            if item.status != WorkStatus::Pending {
                continue;
            }
            if let Some(filter) = work_type_filter
                && !filter.contains(&item.work_type)
            {
                continue;
            }
            match &best {
                Some((_, best_pri)) if item.priority <= *best_pri => {}
                _ => best = Some((id.clone(), item.priority)),
            }
        }

        if let Some((id, _)) = best {
            // SAFETY: `id` was picked from `items.iter()` above, so the key is
            // present in the map. Infallible by construction.
            #[allow(clippy::unwrap_used)]
            let item = items.get_mut(&id).unwrap();
            item.status = WorkStatus::Claimed;
            item.claimed_by = Some(agent_id.to_string());
            item.claimed_at_ms = Some(now_ms());
            let claimed = item.clone();
            let _ = self.tx.send(WorkEvent::Claimed {
                id: id.clone(),
                agent_id: agent_id.to_string(),
            });
            Some(claimed)
        } else {
            None
        }
    }

    /// Transition claimed item to in-progress.
    pub fn start(&self, item_id: &str) -> anyhow::Result<()> {
        let mut items = self.items.write();
        let item =
            items
                .get_mut(item_id)
                .ok_or_else(|| crate::error::SdkError::WorkItemNotFound {
                    item_id: item_id.to_string(),
                })?;
        if item.status != WorkStatus::Claimed {
            return Err(anyhow::anyhow!("Item {} not in Claimed state", item_id));
        }
        item.status = WorkStatus::InProgress;
        let _ = self.tx.send(WorkEvent::Started {
            id: item_id.to_string(),
        });
        Ok(())
    }

    /// Mark item as completed with result.
    pub fn complete(&self, item_id: &str, result: WorkResult) -> anyhow::Result<()> {
        let mut items = self.items.write();
        let item =
            items
                .get_mut(item_id)
                .ok_or_else(|| crate::error::SdkError::WorkItemNotFound {
                    item_id: item_id.to_string(),
                })?;
        item.status = WorkStatus::Completed;
        item.result = Some(result);
        item.completed_at_ms = Some(now_ms());
        let success = item.result.as_ref().map(|r| r.success).unwrap_or(false);
        let _ = self.tx.send(WorkEvent::Completed {
            id: item_id.to_string(),
            success,
        });
        Ok(())
    }

    /// Retry a failed item.
    pub fn retry(&self, item_id: &str) -> anyhow::Result<bool> {
        let mut items = self.items.write();
        let item =
            items
                .get_mut(item_id)
                .ok_or_else(|| crate::error::SdkError::WorkItemNotFound {
                    item_id: item_id.to_string(),
                })?;
        if item.retry_count >= item.max_retries {
            return Ok(false);
        }
        item.retry_count += 1;
        item.status = WorkStatus::Pending;
        item.claimed_by = None;
        item.claimed_at_ms = None;
        Ok(true)
    }

    /// Cancel an item.
    pub fn cancel(&self, item_id: &str) -> anyhow::Result<()> {
        let mut items = self.items.write();
        let item =
            items
                .get_mut(item_id)
                .ok_or_else(|| crate::error::SdkError::WorkItemNotFound {
                    item_id: item_id.to_string(),
                })?;
        item.status = WorkStatus::Cancelled;
        let _ = self.tx.send(WorkEvent::Cancelled {
            id: item_id.to_string(),
        });
        Ok(())
    }

    /// Get a work item by ID.
    pub fn get(&self, item_id: &str) -> Option<WorkItem> {
        self.items.read().get(item_id).cloned()
    }

    /// List items, optionally filtered by status.
    pub fn list(&self, filter: Option<WorkStatus>) -> Vec<WorkItem> {
        self.items
            .read()
            .values()
            .filter(|item| match filter {
                Some(s) => item.status == s,
                None => true,
            })
            .cloned()
            .collect()
    }

    /// Get queue statistics.
    pub fn stats(&self) -> WorkQueueStats {
        let items = self.items.read();
        let mut stats = WorkQueueStats::default();
        for item in items.values() {
            match item.status {
                WorkStatus::Pending => stats.pending += 1,
                WorkStatus::Claimed => stats.claimed += 1,
                WorkStatus::InProgress => stats.in_progress += 1,
                WorkStatus::Completed => stats.completed += 1,
                WorkStatus::Failed => stats.failed += 1,
                WorkStatus::Cancelled => stats.cancelled += 1,
            }
        }
        stats
    }

    /// Subscribe to work events.
    pub fn subscribe(&self) -> broadcast::Receiver<WorkEvent> {
        self.tx.subscribe()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_and_claim() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        let id = q.enqueue("review", serde_json::json!({"file": "main.rs"}), 1);
        let item = q.claim("agent-1", None).unwrap();
        assert_eq!(item.id, id);
        assert_eq!(item.claimed_by.unwrap(), "agent-1");
        assert_eq!(item.status, WorkStatus::Claimed);
    }

    #[test]
    fn claim_is_atomic() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        q.enqueue("task", serde_json::json!({}), 0);
        let first = q.claim("a1", None);
        let second = q.claim("a2", None);
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[test]
    fn claim_respects_priority() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        q.enqueue("low", serde_json::json!({}), 1);
        q.enqueue("high", serde_json::json!({}), 10);
        let item = q.claim("a1", None).unwrap();
        assert_eq!(item.priority, 10);
    }

    #[test]
    fn claim_with_type_filter() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        q.enqueue("review", serde_json::json!({}), 0);
        q.enqueue("build", serde_json::json!({}), 0);
        let item = q.claim("a1", Some(&["build".into()]));
        assert!(item.is_some());
        assert_eq!(item.unwrap().work_type, "build");
    }

    #[test]
    fn complete_item() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        let id = q.enqueue("task", serde_json::json!({}), 0);
        let _item = q.claim("a1", None).unwrap();
        q.start(&id).unwrap();
        q.complete(
            &id,
            WorkResult {
                success: true,
                content: "done".into(),
                error: None,
                duration_ms: 100,
                tokens_used: None,
            },
        )
        .unwrap();
        let item = q.get(&id).unwrap();
        assert_eq!(item.status, WorkStatus::Completed);
        assert!(item.result.unwrap().success);
    }

    #[test]
    fn retry_item() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        let id = q.enqueue("task", serde_json::json!({}), 0);
        {
            let mut items = q.items.write();
            let item = items.get_mut(&id).unwrap();
            item.status = WorkStatus::Failed;
            item.max_retries = 2;
        }
        assert!(q.retry(&id).unwrap());
        let item = q.get(&id).unwrap();
        assert_eq!(item.status, WorkStatus::Pending);
        assert_eq!(item.retry_count, 1);
    }

    #[test]
    fn cancel_item() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        let id = q.enqueue("task", serde_json::json!({}), 0);
        q.cancel(&id).unwrap();
        assert_eq!(q.get(&id).unwrap().status, WorkStatus::Cancelled);
    }

    #[test]
    fn queue_stats() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        q.enqueue("t1", serde_json::json!({}), 0);
        q.enqueue("t2", serde_json::json!({}), 0);
        q.claim("a1", None);
        let stats = q.stats();
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.claimed, 1);
    }

    #[test]
    fn subscribe_events() {
        let q = WorkQueue::new(WorkQueueConfig::default());
        let mut rx = q.subscribe();
        q.enqueue("task", serde_json::json!({}), 0);
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, WorkEvent::Enqueued { .. }));
    }
}
