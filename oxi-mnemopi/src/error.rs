//! Error types for the oxi-mnemopi memory engine.

use thiserror::Error;

/// Error type for all oxi-mnemopi operations.
#[derive(Debug, Error)]
pub enum MnemopiError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("memory not found: {0}")]
    NotFound(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("llm error: {0}")]
    Llm(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("{0}")]
    Other(String),
}

/// Result alias with defaulted error parameter.
pub type Result<T, E = MnemopiError> = std::result::Result<T, E>;
