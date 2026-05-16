//! Error types for oxi-ai

use thiserror::Error;

/// Provider-specific errors
#[derive(Error, Debug)]
pub enum ProviderError {
    /// API 키가 누락됨.
    #[error("Missing API key")]
    MissingApiKey,

    /// 알 수 없는 프로바이더.
    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    /// 프로바이더가 아직 구현되지 않음.
    #[error("Provider not implemented: {0}")]
    NotImplemented(String),

    /// HTTP 오류 (상태 코드 + 메시지).
    #[error("HTTP error {0}: {1}")]
    HttpError(u16, String),

    /// HTTP 요청 실패.
    #[error("Request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    /// I/O 오류.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// 프로바이더로부터 올바르지 않은 응답.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// API 키 형식이 올바르지 않음.
    #[error("Invalid API key format")]
    InvalidApiKey,

    /// JSON 파싱 오류.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// 스트리밍 오류.
    #[error("Stream error: {0}")]
    StreamError(String),

    /// 네트워크 오류.
    #[error("Network error: {0}")]
    NetworkError(String),

    /// 요청 시간 초과.
    #[error("Request timed out")]
    Timeout,

    /// 요청 속도 제한.
    #[error("Rate limited")]
    RateLimited {
        /// 서버가 제시한 대기 시간.
        retry_after: Option<std::time::Duration>,
    },
}

impl ProviderError {
    /// 오류가 재시도 가능한지 반환.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::HttpError(status, _) => *status == 429 || *status >= 500,
            Self::NetworkError(_) => true,
            Self::Timeout => true,
            Self::RateLimited { .. } => true,
            _ => false,
        }
    }

    /// 서버가 제시한 재시도 대기 시간을 반환.
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
    /// 프로바이더 오래의 래핑.
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    /// 유효성 검사 오래의 래핑.
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    /// I/O 오류의 래핑.
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
