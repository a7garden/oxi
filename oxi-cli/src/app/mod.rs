//! Application core — App, AgentSession, interactive loop
pub(crate) mod agent_session;
pub(crate) mod agent_session_runtime;

// Re-exports for convenience
pub use agent_session::{AgentSession, AgentSessionHandle, ScopedModel, SessionEvent};
pub use crate::context::auto_compaction::CompactionReason;
pub use agent_session_runtime::AgentSessionRuntime;
