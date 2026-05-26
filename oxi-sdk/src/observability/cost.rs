//! Token cost tracking — per-agent token usage and cost breakdown.

use oxi_ai::{Model, ModelRegistry};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Token usage broken down by component.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input (prompt) tokens.
    pub input: u64,
    /// Output (completion) tokens.
    pub output: u64,
    /// Tokens read from cache.
    pub cache_read: u64,
    /// Tokens written to cache.
    pub cache_write: u64,
}

impl TokenUsage {
    /// Total tokens across all categories.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// Compute cost breakdown using a model's pricing data.
    pub fn cost(&self, model: &Model) -> CostBreakdown {
        let input_cost = self.input as f64 * model.cost.input / 1_000_000.0;
        let output_cost = self.output as f64 * model.cost.output / 1_000_000.0;
        let cache_read_cost = self.cache_read as f64 * model.cost.cache_read / 1_000_000.0;
        let cache_write_cost = self.cache_write as f64 * model.cost.cache_write / 1_000_000.0;
        CostBreakdown {
            input_cost,
            output_cost,
            cache_read_cost,
            cache_write_cost,
        }
    }
}

/// Cost breakdown by category (USD).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Input cost in USD.
    pub input_cost: f64,
    /// Output cost in USD.
    pub output_cost: f64,
    /// Cache read cost in USD.
    pub cache_read_cost: f64,
    /// Cache write cost in USD.
    pub cache_write_cost: f64,
}

impl CostBreakdown {
    /// Total cost across all categories.
    pub fn total(&self) -> f64 {
        self.input_cost + self.output_cost + self.cache_read_cost + self.cache_write_cost
    }
}

/// Per-agent cost snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSnapshot {
    pub agent_id: String,
    pub usage: TokenUsage,
    pub cost: CostBreakdown,
    pub budget_remaining: Option<f64>,
}

/// Global cost snapshot across all agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCostSnapshot {
    pub total_agents: usize,
    pub total_usage: TokenUsage,
    pub total_cost: CostBreakdown,
    pub global_budget_remaining: Option<f64>,
    pub per_agent: Vec<CostSnapshot>,
}

/// Configuration for the cost tracker.
#[derive(Debug, Clone, Default)]
pub struct CostTrackerConfig {
    /// Budget per agent in USD. `None` = unlimited.
    pub per_agent_budget: Option<f64>,
    /// Global budget in USD. `None` = unlimited.
    pub global_budget: Option<f64>,
}

/// Cost tracker — accumulates token usage and computed costs per agent.
///
/// Usage: call `record(agent_id, model, usage)` after each LLM call,
/// then query `snapshot(agent_id)` or `global_snapshot()`.
pub struct CostTracker {
    usage: Arc<RwLock<HashMap<String, TokenUsage>>>,
    costs: Arc<RwLock<HashMap<String, CostBreakdown>>>,
    _model_registry: Arc<ModelRegistry>,
    config: CostTrackerConfig,
}

impl CostTracker {
    /// Create a new cost tracker.
    pub fn new(model_registry: Arc<ModelRegistry>, config: CostTrackerConfig) -> Self {
        Self {
            usage: Arc::new(RwLock::new(HashMap::new())),
            costs: Arc::new(RwLock::new(HashMap::new())),
            _model_registry: model_registry,
            config,
        }
    }

    /// Record token usage for an agent using a model's pricing.
    pub fn record(&self, agent_id: &str, model: &Model, usage: TokenUsage) {
        let cost = usage.cost(model);

        self.usage
            .write()
            .entry(agent_id.into())
            .and_modify(|u| {
                u.input += usage.input;
                u.output += usage.output;
                u.cache_read += usage.cache_read;
                u.cache_write += usage.cache_write;
            })
            .or_insert(usage);

        self.costs
            .write()
            .entry(agent_id.into())
            .and_modify(|c| {
                c.input_cost += cost.input_cost;
                c.output_cost += cost.output_cost;
                c.cache_read_cost += cost.cache_read_cost;
                c.cache_write_cost += cost.cache_write_cost;
            })
            .or_insert(cost);
    }

    /// Snapshot for a specific agent.
    pub fn snapshot(&self, agent_id: &str) -> Option<CostSnapshot> {
        let usage = self.usage.read().get(agent_id)?.clone();
        let cost = self.costs.read().get(agent_id)?.clone();
        let budget_remaining = self.config.per_agent_budget.map(|b| b - cost.total());
        Some(CostSnapshot {
            agent_id: agent_id.into(),
            usage,
            cost,
            budget_remaining,
        })
    }

    /// Global snapshot across all agents.
    pub fn global_snapshot(&self) -> GlobalCostSnapshot {
        let usage_guard = self.usage.read();
        let cost_guard = self.costs.read();

        let total_usage = usage_guard
            .values()
            .fold(TokenUsage::default(), |mut acc, u| {
                acc.input += u.input;
                acc.output += u.output;
                acc.cache_read += u.cache_read;
                acc.cache_write += u.cache_write;
                acc
            });

        let total_cost = cost_guard
            .values()
            .fold(CostBreakdown::default(), |mut acc, c| {
                acc.input_cost += c.input_cost;
                acc.output_cost += c.output_cost;
                acc.cache_read_cost += c.cache_read_cost;
                acc.cache_write_cost += c.cache_write_cost;
                acc
            });

        let global_budget_remaining = self.config.global_budget.map(|b| b - total_cost.total());
        let per_agent = usage_guard
            .keys()
            .map(|id| CostSnapshot {
                agent_id: id.clone(),
                usage: usage_guard.get(id).cloned().unwrap_or_default(),
                cost: cost_guard.get(id).cloned().unwrap_or_default(),
                budget_remaining: self
                    .config
                    .per_agent_budget
                    .map(|b| b - cost_guard.get(id).map(|c| c.total()).unwrap_or(0.0)),
            })
            .collect();

        GlobalCostSnapshot {
            total_agents: usage_guard.len(),
            total_usage,
            total_cost,
            global_budget_remaining,
            per_agent,
        }
    }

    /// Returns `true` if the agent has exceeded its per-agent budget.
    pub fn is_over_budget(&self, agent_id: &str) -> bool {
        if let Some(budget) = self.config.per_agent_budget {
            let cost = self
                .costs
                .read()
                .get(agent_id)
                .map(|c| c.total())
                .unwrap_or(0.0);
            cost > budget
        } else {
            false
        }
    }

    /// Returns `true` if the total across all agents exceeds the global budget.
    pub fn is_over_global_budget(&self) -> bool {
        if let Some(budget) = self.config.global_budget {
            let total = self.costs.read().values().map(|c| c.total()).sum::<f64>();
            total > budget
        } else {
            false
        }
    }

    /// Get current total cost for an agent.
    pub fn agent_cost(&self, agent_id: &str) -> f64 {
        self.costs
            .read()
            .get(agent_id)
            .map(|c| c.total())
            .unwrap_or(0.0)
    }

    /// Reset counters for a specific agent.
    pub fn reset(&self, agent_id: &str) {
        self.usage.write().remove(agent_id);
        self.costs.write().remove(agent_id);
    }

    /// Reset all counters.
    pub fn reset_all(&self) {
        self.usage.write().clear();
        self.costs.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_ai::Api;

    fn test_model() -> Model {
        let mut m = Model::new(
            "gpt-4o",
            "GPT-4o",
            Api::AnthropicMessages,
            "openai",
            "https://api.openai.com",
        );
        m.cost.input = 2.5; // $2.50 / M
        m.cost.output = 10.0; // $10 / M
        m.cost.cache_read = 1.25;
        m.cost.cache_write = 0.0;
        m
    }

    #[test]
    fn token_usage_total() {
        let usage = TokenUsage {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
        };
        assert_eq!(usage.total(), 1800);
    }

    #[test]
    fn token_usage_cost() {
        let usage = TokenUsage {
            input: 1_000_000,
            output: 500_000,
            cache_read: 0,
            cache_write: 0,
        };
        let cost = usage.cost(&test_model());
        assert!((cost.input_cost - 2.5).abs() < 1e-9);
        assert!((cost.output_cost - 5.0).abs() < 1e-9);
    }

    #[test]
    fn cost_breakdown_total() {
        let cost = CostBreakdown {
            input_cost: 1.0,
            output_cost: 2.0,
            cache_read_cost: 0.5,
            cache_write_cost: 0.0,
        };
        assert!((cost.total() - 3.5).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_tracker_record() {
        let registry = Arc::new(ModelRegistry::new());
        let tracker = CostTracker::new(registry, CostTrackerConfig::default());

        let model = test_model();
        let usage = TokenUsage {
            input: 1_000_000,
            output: 500_000,
            cache_read: 0,
            cache_write: 0,
        };
        tracker.record("agent-1", &model, usage.clone());

        let snap = tracker.snapshot("agent-1").unwrap();
        assert_eq!(snap.agent_id, "agent-1");
        assert_eq!(snap.usage.input, 1_000_000);
        assert!((snap.cost.total() - 7.5).abs() < 1e-6);
    }

    #[test]
    fn cost_tracker_accumulation() {
        let registry = Arc::new(ModelRegistry::new());
        let tracker = CostTracker::new(registry, CostTrackerConfig::default());

        let model = test_model();
        tracker.record(
            "a1",
            &model,
            TokenUsage {
                input: 100,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );
        tracker.record(
            "a1",
            &model,
            TokenUsage {
                input: 100,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );

        let snap = tracker.snapshot("a1").unwrap();
        assert_eq!(snap.usage.input, 200);
    }

    #[test]
    fn cost_tracker_budget_check() {
        let registry = Arc::new(ModelRegistry::new());
        let tracker = CostTracker::new(
            registry,
            CostTrackerConfig {
                per_agent_budget: Some(1.0),
                global_budget: None,
            },
        );

        let model = test_model();
        // $2.50/M input × 1M = $2.50 > $1.00 budget
        tracker.record(
            "a1",
            &model,
            TokenUsage {
                input: 1_000_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );

        assert!(tracker.is_over_budget("a1"));
    }

    #[test]
    fn cost_tracker_reset() {
        let registry = Arc::new(ModelRegistry::new());
        let tracker = CostTracker::new(registry, CostTrackerConfig::default());

        let model = test_model();
        tracker.record(
            "a1",
            &model,
            TokenUsage {
                input: 100,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );

        tracker.reset("a1");
        assert!(tracker.snapshot("a1").is_none());
    }

    #[test]
    fn cost_tracker_global_snapshot() {
        let registry = Arc::new(ModelRegistry::new());
        let tracker = CostTracker::new(registry, CostTrackerConfig::default());

        let model = test_model();
        tracker.record(
            "a1",
            &model,
            TokenUsage {
                input: 1_000_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );
        tracker.record(
            "a2",
            &model,
            TokenUsage {
                input: 500_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );

        let global = tracker.global_snapshot();
        assert_eq!(global.total_agents, 2);
        assert_eq!(global.total_usage.input, 1_500_000);
        assert!((global.total_cost.input_cost - 3.75).abs() < 1e-6);
    }
}
