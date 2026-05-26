//! CoordinatedGroup — AgentHandle-based coordination strategies.

use std::sync::Arc;

use crate::coordination::consensus::Consensus;
use crate::coordination::shared_memory::{MemoryKey, SharedMemory};
use crate::coordination::work_queue::{WorkQueue, WorkResult};
use crate::lifecycle::AgentHandle;

/// Builder for `CoordinatedGroup`.
pub struct CoordinatedGroupBuilder {
    handles: Vec<AgentHandle>,
    work_queue: Option<Arc<WorkQueue>>,
    shared_memory: Option<Arc<SharedMemory>>,
    consensus: Option<Arc<Consensus>>,
}

impl CoordinatedGroupBuilder {
    fn new() -> Self {
        Self {
            handles: Vec::new(),
            work_queue: None,
            shared_memory: None,
            consensus: None,
        }
    }

    /// Add an agent handle to the group.
    pub fn handle(mut self, handle: AgentHandle) -> Self {
        self.handles.push(handle);
        self
    }

    /// Set the work queue.
    pub fn work_queue(mut self, queue: Arc<WorkQueue>) -> Self {
        self.work_queue = Some(queue);
        self
    }

    /// Set the shared memory.
    pub fn shared_memory(mut self, memory: Arc<SharedMemory>) -> Self {
        self.shared_memory = Some(memory);
        self
    }

    /// Set the consensus manager.
    pub fn consensus(mut self, consensus: Arc<Consensus>) -> Self {
        self.consensus = Some(consensus);
        self
    }

    /// Build the coordinated group.
    pub fn build(self) -> CoordinatedGroup {
        CoordinatedGroup {
            handles: self.handles,
            work_queue: self.work_queue,
            shared_memory: self.shared_memory,
            consensus: self.consensus,
        }
    }
}

/// A group of agent handles with coordination primitives.
///
/// Supports fan-out (parallel work), voting, and map-reduce patterns.
pub struct CoordinatedGroup {
    handles: Vec<AgentHandle>,
    work_queue: Option<Arc<WorkQueue>>,
    shared_memory: Option<Arc<SharedMemory>>,
    consensus: Option<Arc<Consensus>>,
}

impl CoordinatedGroup {
    /// Create a new builder.
    pub fn builder() -> CoordinatedGroupBuilder {
        CoordinatedGroupBuilder::new()
    }

    /// Number of agents in the group.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Whether the group is empty.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Fan-out: distribute payloads across agents via the work queue.
    ///
    /// Each agent claims and executes one item in parallel.
    pub async fn fan_out(
        &self,
        work_type: &str,
        payloads: Vec<serde_json::Value>,
    ) -> Vec<WorkResult> {
        let queue = match &self.work_queue {
            Some(q) => q,
            None => return Vec::new(),
        };

        // Enqueue all work items
        for payload in &payloads {
            queue.enqueue(work_type, payload.clone(), 0);
        }

        // Each agent claims and runs
        let mut join_handles = Vec::new();
        for handle in &self.handles {
            let queue = Arc::clone(queue);
            let handle = handle.clone();

            join_handles.push(tokio::spawn(async move {
                let item = queue.claim(handle.agent_id(), None);
                if let Some(item) = item {
                    queue.start(&item.id).ok();
                    let prompt = format!(
                        "Complete this task:\n{}",
                        serde_json::to_string_pretty(&item.payload).unwrap_or_default()
                    );
                    let start = std::time::Instant::now();
                    match handle.run(prompt).await {
                        Ok((response, _)) => {
                            let result = WorkResult {
                                success: true,
                                content: response.content,
                                error: None,
                                duration_ms: start.elapsed().as_millis() as u64,
                                tokens_used: None,
                            };
                            queue.complete(&item.id, result.clone()).ok();
                            Some(result)
                        }
                        Err(e) => {
                            let result = WorkResult {
                                success: false,
                                content: String::new(),
                                error: Some(e.to_string()),
                                duration_ms: start.elapsed().as_millis() as u64,
                                tokens_used: None,
                            };
                            queue.complete(&item.id, result.clone()).ok();
                            Some(result)
                        }
                    }
                } else {
                    None
                }
            }));
        }

        // Collect results
        let mut results = Vec::new();
        for jh in join_handles {
            if let Ok(Some(result)) = jh.await {
                results.push(result);
            }
        }
        results
    }

    /// Vote: each agent casts a vote on a question.
    ///
    /// Uses the consensus module. Returns `None` if no consensus is configured.
    pub async fn vote(
        &self,
        question: &str,
        options: &[&str],
    ) -> Option<crate::coordination::consensus::VoteResult> {
        let consensus = self.consensus.as_ref()?;

        let vote_id = format!("vote-{}", uuid::Uuid::new_v4());
        let voter_ids: Vec<String> = self
            .handles
            .iter()
            .map(|h| h.agent_id().to_string())
            .collect();
        consensus.start(&vote_id, voter_ids, 0.5);

        // Each agent "decides" by running a prompt
        for handle in &self.handles {
            let options_str = options.join(", ");
            let prompt = format!(
                "Choose one option for this question. Reply with ONLY the option name, nothing else.\n\nQuestion: {}\nOptions: {}",
                question, options_str
            );

            // For vote, we try to run but handle failures gracefully
            if let Ok((response, _)) = handle.run(prompt).await {
                let choice = response.content.trim().to_string();
                // Find the closest matching option
                let matched = options
                    .iter()
                    .find(|o| choice.contains(**o))
                    .map(|o| o.to_string())
                    .unwrap_or(choice);
                consensus.vote(&vote_id, handle.agent_id(), matched).ok();
            }
        }

        consensus.status(&vote_id)
    }

    /// Map-reduce: each agent processes a payload, results stored in shared memory.
    pub async fn map_reduce(
        &self,
        work_type: &str,
        payloads: Vec<serde_json::Value>,
        reduce_key: &MemoryKey,
    ) -> anyhow::Result<Vec<WorkResult>> {
        let memory = self.shared_memory.as_ref();
        let results = self.fan_out(work_type, payloads).await;

        // Store results in shared memory if available
        if let Some(memory) = memory {
            let results_json = serde_json::to_string(&results)?;
            memory.write(
                reduce_key,
                serde_json::json!(results_json),
                "coordinator",
                None,
            )?;
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_creates_group() {
        let group = CoordinatedGroup::builder().build();
        assert!(group.is_empty());
        assert_eq!(group.len(), 0);
    }
}
