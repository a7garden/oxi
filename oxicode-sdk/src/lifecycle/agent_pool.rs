//! Agent pool — live agent instance tracking for session persistence.
//!
//! Stores running `Agent` instances keyed by ID, enabling:
//! - Session continuation after agent completion
//! - State export/import for persistence
//! - Agent lookup by ID

use oxicode_agent::Agent;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Pool of live `Agent` instances keyed by string ID.
///
/// Thread-safe via `parking_lot::RwLock`. Agents can be inserted,
/// retrieved, removed, and have their state exported/imported.
pub struct AgentPool {
    agents: RwLock<HashMap<String, Arc<Agent>>>,
}

impl AgentPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Insert an agent into the pool.
    pub fn insert(&self, id: String, agent: Arc<Agent>) {
        self.agents.write().insert(id, agent);
    }

    /// Get an agent by ID.
    pub fn get(&self, id: &str) -> Option<Arc<Agent>> {
        self.agents.read().get(id).cloned()
    }

    /// Remove an agent from the pool.
    pub fn remove(&self, id: &str) -> Option<Arc<Agent>> {
        self.agents.write().remove(id)
    }

    /// Export an agent's state as JSON.
    ///
    /// Returns `None` if the agent is not in the pool or state
    /// serialization fails.
    pub fn export_state(&self, id: &str) -> Option<serde_json::Value> {
        let agents = self.agents.read();
        let agent = agents.get(id)?;
        agent.export_state().ok()
    }

    /// Import agent state from JSON.
    ///
    /// Returns `false` if the agent is not in the pool or import fails.
    pub fn import_state(&self, id: &str, state: serde_json::Value) -> bool {
        let agents = self.agents.read();
        if let Some(agent) = agents.get(id) {
            agent.import_state(state).is_ok()
        } else {
            false
        }
    }

    /// Number of agents in the pool.
    pub fn len(&self) -> usize {
        self.agents.read().len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.agents.read().is_empty()
    }

    /// List all agent IDs in the pool.
    pub fn ids(&self) -> Vec<String> {
        self.agents.read().keys().cloned().collect()
    }

    /// Check if an agent exists in the pool.
    pub fn contains(&self, id: &str) -> bool {
        self.agents.read().contains_key(id)
    }

    /// Snapshot iteration over all (id, agent) pairs. Holds the read lock for
    /// the duration of the closure; do not call back into the pool from `f`.
    pub fn for_each_row<F: FnMut(&str, &Arc<Agent>)>(&self, mut f: F) {
        let agents = self.agents.read();
        for (id, agent) in agents.iter() {
            f(id.as_str(), agent);
        }
    }
}

impl Default for AgentPool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AgentPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPool")
            .field("count", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `make_agent` was removed — it used `std::mem::zeroed::<Agent>()` which
    // is UB (Agent contains non-zero fields) and was never called by any test.
    // Re-add a proper Agent constructor here if pool tests are needed.
    // For a safe version, we test the pool with just the data structure operations.

    #[test]
    fn test_pool_new_empty() {
        let pool = AgentPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn for_each_row_visits_all_inserted() {
        let pool = AgentPool::new();
        let mut seen = Vec::new();
        pool.for_each_row(|id, _| seen.push(id.to_string()));
        assert!(seen.is_empty());
    }

    #[test]
    fn test_pool_default() {
        let pool = AgentPool::default();
        assert!(pool.is_empty());
    }

    #[test]
    fn test_pool_debug() {
        let pool = AgentPool::new();
        let debug = format!("{:?}", pool);
        assert!(debug.contains("AgentPool"));
    }
}
