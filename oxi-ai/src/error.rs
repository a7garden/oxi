//! Error types for oxi-ai

use thiserror::Error;

/// Provider-specific errors
#[derive(Error, Debug)]
pub enum ProviderError {
    /// API key is missing.
    #[error("Missing API key")]
    MissingApiKey,

    /// Unknown provider.
    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    /// Provider not yet implemented.
    #[error("Provider not implemented: {0}")]
    NotImplemented(String),

    /// HTTP error (status code + message).
    #[error("HTTP error {0}: {1}")]
    HttpError(u16, String),

    /// HTTP request failed.
    #[error("Request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    /// I/O error.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Invalid response from provider.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// Invalid API key format.
    #[error("Invalid API key format")]
    InvalidApiKey,

    /// JSON parsing error.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// Streaming error.
    #[error("Stream error: {0}")]
    StreamError(String),

    /// Network error.
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Context window overflow.
    #[error("Context overflow")]
    ContextOverflow,

    /// Request timed out.
    #[error("Request timed out")]
    Timeout,

    /// Rate limit exceeded.
    #[error("Rate limited")]
    RateLimited {
        /// Wait time suggested by the server.
        retry_after: Option<std::time::Duration>,
    },
}

impl ProviderError {
    /// Returns whether this error is retryable.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::HttpError(status, _) => *status == 429 || *status >= 500,
            Self::NetworkError(_) => true,
            Self::Timeout => true,
            Self::RateLimited { .. } => true,
            _ => false,
        }
    }

    /// Returns the retry wait time suggested by the server.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            Self::HttpError(429, _) => Some(std::time::Duration::from_secs(5)),
            _ => None,
        }
    }
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
    /// Wraps a provider error.
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    /// Wraps a validation error.
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    /// Wraps an I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type alias
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_display() {
        assert_eq!(ProviderError::MissingApiKey.to_string(), "Missing API key");
        assert_eq!(
            ProviderError::UnknownProvider("foo".to_string()).to_string(),
            "Unknown provider: foo"
        );
        assert_eq!(
            ProviderError::HttpError(429, "rate limited".to_string()).to_string(),
            "HTTP error 429: rate limited"
        );
        assert_eq!(
            ProviderError::InvalidResponse("bad json".to_string()).to_string(),
            "Invalid response: bad json"
        );
        assert_eq!(
            ProviderError::StreamError("disconnected".to_string()).to_string(),
            "Stream error: disconnected"
        );
        assert_eq!(
            ProviderError::NotImplemented("x".to_string()).to_string(),
            "Provider not implemented: x"
        );
    }

    #[test]
    fn error_chain_from_provider_error() {
        let inner = ProviderError::MissingApiKey;
        let outer: Error = inner.into();
        assert!(matches!(
            outer,
            Error::Provider(ProviderError::MissingApiKey)
        ));
        assert!(outer.to_string().contains("Missing API key"));
    }

    #[test]
    fn validation_error_display() {
        let err = ValidationError::MissingRequiredField("model".to_string());
        assert_eq!(err.to_string(), "Missing required field: model");
    }

    #[test]
    fn error_chain_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let outer: Error = io_err.into();
        assert!(matches!(outer, Error::Io(_)));
    }
}
