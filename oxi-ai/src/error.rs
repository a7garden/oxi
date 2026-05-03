//! Error types for oxi-ai

use thiserror::Error;

/// Provider-specific errors
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("Missing API key")]
    MissingApiKey,

    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    #[error("Provider not implemented: {0}")]
    NotImplemented(String),

    #[error("HTTP error {0}: {1}")]
    HttpError(u16, String),

    #[error("Request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Invalid API key format")]
    InvalidApiKey,

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Stream error: {0}")]
    StreamError(String),
}

/// Validation errors
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),

    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),

    #[error("Missing required field: {0}")]
    MissingRequiredField(String),
}

/// Unified error type for oxi-ai
#[derive(Error, Debug)]
pub enum Error {
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type alias
pub type Result<T> = std::result::Result<T, Error>;
