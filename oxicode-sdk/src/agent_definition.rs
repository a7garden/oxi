//! Agent definition file parsing and validation.
//!
//! Re-exports from `oxicode_agent::agent_definition` — the canonical implementation
//! lives in the `oxicode-agent` crate (lower layer in the dependency graph).

pub use oxicode_agent::agent_definition::{
    AgentDefinition, AgentDiscovery, AgentScope, DefaultContext, current_subagent_depth,
    max_subagent_depth, validate_agent_name,
};
