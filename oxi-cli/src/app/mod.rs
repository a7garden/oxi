//! Application core — App, AgentSession, interactive loop
pub(crate) mod agent_session;
pub(crate) mod agent_session_runtime;

// Re-exports for convenience
pub use agent_session::{AgentSession, AgentSessionHandle, ScopedModel, SessionEvent, CompactionReason, InteractiveSession};
pub use agent_session_runtime::{AgentSessionRuntime, RuntimeConfig};
