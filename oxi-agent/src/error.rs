//! Error types for oxi-agent

/// Agent-specific errors
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Tool execution error
    #[error("Tool '{tool_name}' failed: {message}")]
    Tool { tool_name: String, message: String },
    /// Stream / provider communication error
    #[error("Stream error: {0}")]
    Stream(String),
    /// State management error
    #[error("State error: {0}")]
    State(String),
    /// Configuration error
    #[error("Config error: {0}")]
    Config(String),
    /// Model not found or unavailable
    #[error("Model '{model_id}' error: {message}")]
    Model { model_id: String, message: String },
    /// Maximum iterations reached
    #[error("Maximum iterations reached ({iterations})")]
    MaxIterations { iterations: usize },
    /// Rate limited – retry after N seconds
    #[error("Rate limited – retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    /// A retriable error that failed after exhausting retries
    #[error("Failed after {attempts} retries: {last_error}")]
    RetriesExhausted { attempts: usize, last_error: String },
    /// Fallback failed – both primary and fallback model errored
    #[error("Both models failed – {primary_model} ({primary_error}) and {fallback_model} ({fallback_error})")]
    FallbackFailed {
        primary_model: String,
        primary_error: String,
        fallback_model: String,
        fallback_error: String,
    },
}

impl AgentError {
    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Stream(_) | Self::RetriesExhausted { .. }
        )
    }

    /// Produce a short, user-friendly message suitable for TUI display.
    pub fn user_friendly(&self) -> String {
        match self {
            Self::Tool { tool_name, message } => {
                format!("Tool '{}' failed: {}", tool_name, message)
            }
            Self::Stream(msg) => format!("Connection error: {}", msg),
            Self::State(msg) => format!("Internal error: {}", msg),
            Self::Config(msg) => format!("Configuration error: {}", msg),
            Self::Model { model_id, message } => {
                format!("Model '{}' error: {}", model_id, message)
            }
            Self::MaxIterations { iterations } => {
                format!(
                    "Reached the iteration limit ({}). Try simplifying your request.",
                    iterations
                )
            }
            Self::RateLimited { retry_after_secs } => {
                format!("Rate limited – will retry in {}s", retry_after_secs)
            }
            Self::RetriesExhausted {
                attempts,
                last_error,
            } => {
                format!("Failed after {} attempts: {}", attempts, last_error)
            }
            Self::FallbackFailed {
                primary_model,
                primary_error,
                fallback_model,
                fallback_error: _,
            } => {
                format!(
                    "Primary model ({}) failed: {}. Fallback ({}) also failed.",
                    primary_model, primary_error, fallback_model
                )
            }
        }
    }
}

impl From<anyhow::Error> for AgentError {
    fn from(err: anyhow::Error) -> Self {
        AgentError::Stream(err.to_string())
    }
}

/// Result type alias for agent operations.
pub type Result<T> = std::result::Result<T, AgentError>;
