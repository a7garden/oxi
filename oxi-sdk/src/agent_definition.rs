//! Agent definition file parsing and validation.
//!
//! Re-exports from `oxi_agent::agent_definition` — the canonical implementation
//! lives in the `oxi-agent` crate (lower layer in the dependency graph).

pub use oxi_agent::agent_definition::{
    AgentDefinition, AgentDiscovery, AgentScope, DefaultContext, current_subagent_depth,
    max_subagent_depth, validate_agent_name,
};
