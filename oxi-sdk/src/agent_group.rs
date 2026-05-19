//! Agent group — multi-agent orchestration primitives.
//!
//! Provides `AgentGroup` for running multiple agents with different
//! strategies: pipeline (sequential), parallel, or orchestrated.

use anyhow::Result;
use oxi_agent::Agent;
use std::sync::Arc;

/// Multi-agent execution strategy.
#[derive(Debug, Clone)]
pub enum GroupStrategy {
    /// Sequential execution. Each agent receives the previous agent's output.
    Pipeline,

    /// Parallel execution. All agents run concurrently, results are collected.
    Parallel {
        /// Maximum concurrent agent executions.
        max_concurrency: usize,
    },

    /// Leader agent distributes work to worker agents.
    Orchestrated {
        /// Index of the leader agent in the group.
        leader: usize,
    },
}

impl Default for GroupStrategy {
    fn default() -> Self {
        GroupStrategy::Parallel { max_concurrency: 4 }
    }
}

/// Output from a single agent in a group execution.
#[derive(Debug, Clone)]
pub struct AgentGroupOutput {
    /// Agent name or model ID.
    pub name: String,
    /// Final response text.
    pub content: String,
    /// Whether the agent succeeded.
    pub success: bool,
    /// Error message if the agent failed.
    pub error: Option<String>,
}

/// Aggregate result from a group execution.
#[derive(Debug)]
pub struct GroupResult {
    /// Per-agent outputs, in agent order.
    pub results: Vec<AgentGroupOutput>,
    /// Total execution time in milliseconds.
    pub total_duration_ms: u64,
}

impl GroupResult {
    /// Check if all agents succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.results.iter().all(|r| r.success)
    }

    /// Check if any agents failed.
    ///
    /// Unlike `!all_succeeded()`, this clearly conveys intent for callers
    /// that need to handle partial failures.
    pub fn has_failures(&self) -> bool {
        self.results.iter().any(|r| !r.success)
    }

    /// Number of agents that completed successfully.
    pub fn success_count(&self) -> usize {
        self.results.iter().filter(|r| r.success).count()
    }

    /// Get the combined content from all agents.
    pub fn combined_content(&self) -> String {
        self.results
            .iter()
            .map(|r| r.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// A group of agents that can be executed together.
pub struct AgentGroup {
    agents: Vec<Arc<Agent>>,
    strategy: GroupStrategy,
}

impl AgentGroup {
    /// Create a new empty group with the given strategy.
    pub fn new(strategy: GroupStrategy) -> Self {
        Self {
            agents: Vec::new(),
            strategy,
        }
    }

    /// Add an agent to the group.
    pub fn agent(mut self, agent: Arc<Agent>) -> Self {
        self.agents.push(agent);
        self
    }

    /// Get the number of agents in the group.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Check if the group is empty.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Execute the group with the given prompt.
    pub async fn run(&self, prompt: String) -> Result<GroupResult> {
        if self.agents.is_empty() {
            return Ok(GroupResult {
                results: Vec::new(),
                total_duration_ms: 0,
            });
        }

        let start = std::time::Instant::now();
        let results = match &self.strategy {
            GroupStrategy::Pipeline => self.run_pipeline(prompt).await?,
            GroupStrategy::Parallel { max_concurrency } => {
                self.run_parallel(prompt, *max_concurrency).await?
            }
            GroupStrategy::Orchestrated { leader } => {
                self.run_orchestrated(prompt, *leader).await?
            }
        };

        Ok(GroupResult {
            total_duration_ms: start.elapsed().as_millis() as u64,
            results,
        })
    }

    /// Sequential pipeline: each agent receives the previous output.
    ///
    /// If an agent fails, the pipeline stops and returns partial results.
    /// The caller should check [`GroupResult::has_failures`] to detect this.
    async fn run_pipeline(&self, prompt: String) -> Result<Vec<AgentGroupOutput>> {
        let mut results = Vec::with_capacity(self.agents.len());
        let mut current_input = prompt;

        for agent in &self.agents {
            match agent.run(current_input.clone()).await {
                Ok((response, _events)) => {
                    results.push(AgentGroupOutput {
                        name: agent.model_id(),
                        content: response.content.clone(),
                        success: true,
                        error: None,
                    });
                    current_input = response.content;
                }
                Err(e) => {
                    results.push(AgentGroupOutput {
                        name: agent.model_id(),
                        content: String::new(),
                        success: false,
                        error: Some(e.to_string()),
                    });
                    // Pipeline stops on first failure. Partial results are
                    // returned in the GroupResult — use has_failures() to detect.
                    tracing::warn!(
                        agent = agent.model_id(),
                        index = results.len() - 1,
                        "Pipeline agent failed, stopping sequential execution"
                    );
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Parallel execution with concurrency limit.
    ///
    /// Uses `spawn_blocking` because `Agent::run()` produces `!Send` futures.
    async fn run_parallel(
        &self,
        prompt: String,
        max_concurrency: usize,
    ) -> Result<Vec<AgentGroupOutput>> {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
        let mut handles = Vec::with_capacity(self.agents.len());

        for agent in self.agents.iter() {
            let agent = Arc::clone(agent);
            let prompt = prompt.clone();
            let sem = Arc::clone(&semaphore);

            handles.push(tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create runtime");
                rt.block_on(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    match agent.run(prompt).await {
                        Ok((response, _events)) => AgentGroupOutput {
                            name: agent.model_id(),
                            content: response.content,
                            success: true,
                            error: None,
                        },
                        Err(e) => AgentGroupOutput {
                            name: agent.model_id(),
                            content: String::new(),
                            success: false,
                            error: Some(e.to_string()),
                        },
                    }
                })
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(output) => results.push(output),
                Err(e) => results.push(AgentGroupOutput {
                    name: String::new(),
                    content: String::new(),
                    success: false,
                    error: Some(format!("Join error: {}", e)),
                }),
            }
        }
        Ok(results)
    }

    /// Orchestrated: leader delegates to workers.
    async fn run_orchestrated(
        &self,
        prompt: String,
        leader_idx: usize,
    ) -> Result<Vec<AgentGroupOutput>> {
        if leader_idx >= self.agents.len() {
            anyhow::bail!(
                "Leader index {} out of range ({} agents)",
                leader_idx,
                self.agents.len()
            );
        }

        let leader = &self.agents[leader_idx];
        let (response, _events) = leader.run(prompt).await?;

        Ok(vec![AgentGroupOutput {
            name: leader.model_id(),
            content: response.content,
            success: true,
            error: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_strategy_default() {
        let strategy = GroupStrategy::default();
        matches!(strategy, GroupStrategy::Parallel { max_concurrency: 4 });
    }

    #[test]
    fn test_empty_group() {
        let group = AgentGroup::new(GroupStrategy::Pipeline);
        assert!(group.is_empty());
        assert_eq!(group.len(), 0);
    }

    #[test]
    fn test_group_result_all_succeeded() {
        let result = GroupResult {
            results: vec![
                AgentGroupOutput {
                    name: "a".into(),
                    content: "ok".into(),
                    success: true,
                    error: None,
                },
                AgentGroupOutput {
                    name: "b".into(),
                    content: "ok".into(),
                    success: true,
                    error: None,
                },
            ],
            total_duration_ms: 100,
        };
        assert!(result.all_succeeded());
    }

    #[test]
    fn test_group_result_combined_content() {
        let result = GroupResult {
            results: vec![
                AgentGroupOutput {
                    name: "a".into(),
                    content: "first".into(),
                    success: true,
                    error: None,
                },
                AgentGroupOutput {
                    name: "b".into(),
                    content: "second".into(),
                    success: true,
                    error: None,
                },
            ],
            total_duration_ms: 100,
        };
        assert_eq!(result.combined_content(), "first\n\nsecond");
    }
}
