//! Error types for oxi-agent

use std::fmt;

/// Agent-specific errors
#[derive(Debug)]
pub enum AgentError {
    /// Tool execution error
    Tool { tool_name: String, message: String },
    /// Stream / provider communication error
    Stream(String),
    /// State management error
    State(String),
    /// Configuration error
    Config(String),
    /// Model not found or unavailable
    Model { model_id: String, message: String },
    /// Maximum iterations reached
    MaxIterations { iterations: usize },
    /// Rate limited – retry after N seconds
    RateLimited { retry_after_secs: u64 },
    /// A retriable error that failed after exhausting retries
    RetriesExhausted { attempts: usize, last_error: String },
    /// Fallback failed – both primary and fallback model errored
    FallbackFailed {
        primary_model: String,
        primary_error: String,
        fallback_model: String,
        fallback_error: String,
    },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tool { tool_name, message } => {
                write!(f, "Tool '{}' failed: {}", tool_name, message)
            }
            Self::Stream(msg) => write!(f, "Stream error: {}", msg),
            Self::State(msg) => write!(f, "State error: {}", msg),
            Self::Config(msg) => write!(f, "Config error: {}", msg),
            Self::Model { model_id, message } => {
                write!(f, "Model '{}' error: {}", model_id, message)
            }
            Self::MaxIterations { iterations } => {
                write!(f, "Maximum iterations reached ({})", iterations)
            }
            Self::RateLimited { retry_after_secs } => {
                write!(f, "Rate limited – retry after {}s", retry_after_secs)
            }
            Self::RetriesExhausted {
                attempts,
                last_error,
            } => {
                write!(f, "Failed after {} retries: {}", attempts, last_error)
            }
            Self::FallbackFailed {
                primary_model,
                primary_error,
                fallback_model,
                fallback_error,
            } => {
                write!(
                    f,
                    "Both models failed – {} ({}) and {} ({})",
                    primary_model, primary_error, fallback_model, fallback_error
                )
            }
        }
    }
}

impl std::error::Error for AgentError {}

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
