//! Error types for oxi-agent

use thiserror::Error;

/// Agent runtime errors
#[derive(Error, Debug)]
pub enum AgentError {
    #[error("agent not started")]
    NotStarted,

    #[error("agent already running")]
    AlreadyRunning,

    #[error("aborted")]
    Aborted,

    #[error("tool execution failed: {0}")]
    ToolExecutionFailed(String),

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("tool blocked: {0}")]
    ToolBlocked(String),

    #[error("invalid model configuration: {0}")]
    InvalidModelConfig(String),

    #[error("streaming error: {0}")]
    StreamingError(String),

    #[error("message conversion failed: {0}")]
    ConversionError(String),

    #[error("context transform failed: {0}")]
    ContextTransformError(String),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Result type alias for agent operations
pub type Result<T> = std::result::Result<T, AgentError>;
