//! Error types for oxicode-ai

use thiserror::Error;

/// Structured HTTP error detail.
///
/// omp aligns per-provider error classes — `AnthropicApiError` carries a
/// `request-id`, `OpenAIHttpError` parses the body envelope, etc. This struct
/// captures the common structured fields so callers inspect provider/error
/// identity directly instead of parsing a flat `(u16, String)` tuple.
#[derive(Debug, Clone)]
pub struct HttpErrorDetail {
    /// HTTP status code.
    pub status: u16,
    /// Raw response body.
    pub body: String,
    /// Provider id (e.g. `"anthropic"`, `"openai"`, `"deepseek"`), if known.
    pub provider: Option<String>,
    /// Provider request id (Anthropic `request-id`, OpenAI `x-request-id`, …).
    pub request_id: Option<String>,
}

impl HttpErrorDetail {
    /// Minimal detail from a status code and response body.
    pub fn new(status: u16, body: String) -> Self {
        Self {
            status,
            body,
            provider: None,
            request_id: None,
        }
    }

    /// Attach the provider id.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Attach a provider request id (e.g. parsed from a response header).
    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }
}

impl std::fmt::Display for HttpErrorDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP error {}: {}", self.status, self.body)?;
        if let Some(provider) = &self.provider {
            write!(f, " [{provider}]")?;
        }
        if let Some(id) = &self.request_id {
            write!(f, " (request-id: {id})")?;
        }
        Ok(())
    }
}

/// Provider-specific errors. `#[non_exhaustive]` — consumers MUST add a
/// catch-all `_ =>` arm in their `match` expressions. Existing named variants
/// are frozen; their meaning does not change between releases (see
/// `docs/release-process.md`).
#[derive(Error, Debug)]
#[non_exhaustive]
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

    /// HTTP error with structured detail (status, body, provider, request-id).
    #[error("{0}")]
    HttpError(HttpErrorDetail),

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
            Self::HttpError(detail) => detail.status == 429 || detail.status >= 500,
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
            Self::HttpError(detail) if detail.status == 429 => {
                Some(std::time::Duration::from_secs(5))
            }
            _ => None,
        }
    }

    /// Returns the HTTP status code if this is an HTTP error, else `None`.
    ///
    /// Convenience for call sites that previously destructured the old
    /// `HttpError(u16, String)` tuple (e.g. inside `matches!`, which cannot
    /// carry a guard).
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::HttpError(detail) => Some(detail.status),
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

/// Unified error type for oxicode-ai
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
            ProviderError::HttpError(HttpErrorDetail::new(429, "rate limited".to_string()))
                .to_string(),
            "HTTP error 429: rate limited"
        );
        // Structured detail surfaces provider + request-id (omp AnthropicApiError align).
        assert_eq!(
            ProviderError::HttpError(
                HttpErrorDetail::new(500, "boom".to_string())
                    .with_provider("anthropic")
                    .with_request_id(Some("req_123".to_string()))
            )
            .to_string(),
            "HTTP error 500: boom [anthropic] (request-id: req_123)"
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
