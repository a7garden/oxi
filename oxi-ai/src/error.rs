//! Error types for oxi-ai

use thiserror::Error;

/// Provider-specific errors
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("Missing API key")]
/// missing api key variant.
    MissingApiKey,

    #[error("Unknown provider: {0}")]
/// unknown provider variant.
    UnknownProvider(String),

    #[error("Provider not implemented: {0}")]
/// not implemented variant.
    NotImplemented(String),

    #[error("HTTP error {0}: {1}")]
/// http error variant.
    HttpError(u16, String),

    #[error("Request failed: {0}")]
/// request failed variant.
    RequestFailed(#[from] reqwest::Error),

    #[error("IO error: {0}")]
/// io error variant.
    IoError(#[from] std::io::Error),

    #[error("Invalid response: {0}")]
/// invalid response variant.
    InvalidResponse(String),

    #[error("Invalid API key format")]
/// invalid api key variant.
    InvalidApiKey,

    #[error("JSON parse error: {0}")]
/// json parse variant.
    JsonParse(#[from] serde_json::Error),

    #[error("Stream error: {0}")]
/// stream error variant.
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
/// provider variant.
    Provider(#[from] ProviderError),

    #[error("Validation error: {0}")]
/// validation variant.
    Validation(#[from] ValidationError),

    #[error("IO error: {0}")]
/// io variant.
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
        assert!(matches!(outer, Error::Provider(ProviderError::MissingApiKey)));
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
