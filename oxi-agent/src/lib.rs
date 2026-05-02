//! oxi-agent: Agent runtime for oxi
//!
//! This crate provides an agent runtime that integrates with oxi-ai providers.

pub mod types;
pub mod events;
pub mod tools;
pub mod state;
pub mod config;
pub mod agent;

pub use agent::Agent;
pub use config::AgentConfig;
pub use events::AgentEvent;
pub use state::AgentState;
pub use tools::ToolRegistry;

#[cfg(test)]
mod tests;
