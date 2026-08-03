//! Tests for oxicode-ai error types: creation, display, From impls, and error chains.

use oxicode_ai::{Error, ProviderError, ToolValidationError as ToolsValidationError};

// ---------------------------------------------------------------------------
// ProviderError variant creation and display
// ---------------------------------------------------------------------------

#[test]
fn missing_api_key_display() {
    let err = ProviderError::MissingApiKey;
    assert_eq!(err.to_string(), "Missing API key");
}

#[test]
fn unknown_provider_display() {
    let err = ProviderError::UnknownProvider("groq".into());
    assert_eq!(err.to_string(), "Unknown provider: groq");
}

#[test]
fn not_implemented_display() {
    let err = ProviderError::NotImplemented("ollama".into());
    assert_eq!(err.to_string(), "Provider not implemented: ollama");
}

#[test]
fn http_error_display() {
    let err =
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(429, "rate limited".into()));
    assert_eq!(err.to_string(), "HTTP error 429: rate limited");
}

#[test]
fn http_error_common_status_codes() {
    assert!(
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(400, "bad request".into()))
            .to_string()
            .contains("400")
    );
    assert!(
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(401, "unauthorized".into()))
            .to_string()
            .contains("401")
    );
    assert!(
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(403, "forbidden".into()))
            .to_string()
            .contains("403")
    );
    assert!(
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(500, "internal".into()))
            .to_string()
            .contains("500")
    );
    assert!(
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(502, "bad gateway".into()))
            .to_string()
            .contains("502")
    );
    assert!(
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(503, "unavailable".into()))
            .to_string()
            .contains("503")
    );
}

#[test]
fn invalid_response_display() {
    let err = ProviderError::InvalidResponse("unexpected EOF".into());
    assert_eq!(err.to_string(), "Invalid response: unexpected EOF");
}

#[test]
fn invalid_api_key_display() {
    let err = ProviderError::InvalidApiKey;
    assert_eq!(err.to_string(), "Invalid API key format");
}

#[test]
fn stream_error_display() {
    let err = ProviderError::StreamError("connection reset".into());
    assert_eq!(err.to_string(), "Stream error: connection reset");
}

#[test]
fn json_parse_error_display() {
    let bad_json = "{invalid";
    let serde_err: serde_json::Error =
        serde_json::from_str::<serde_json::Value>(bad_json).unwrap_err();
    let err = ProviderError::JsonParse(serde_err);
    assert!(err.to_string().starts_with("JSON parse error:"));
}

// ---------------------------------------------------------------------------
// From implementations
// ---------------------------------------------------------------------------

#[test]
fn from_io_error_to_provider_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "config file missing");
    let provider_err: ProviderError = io_err.into();
    assert!(matches!(provider_err, ProviderError::IoError(_)));
    assert!(provider_err.to_string().contains("config file missing"));
}

#[test]
fn from_io_error_to_top_level_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let top: Error = io_err.into();
    assert!(matches!(top, Error::Io(_)));
    assert!(top.to_string().contains("access denied"));
}

#[test]
fn from_provider_error_to_top_level_error() {
    let provider_err = ProviderError::MissingApiKey;
    let top: Error = provider_err.into();
    assert!(matches!(top, Error::Provider(ProviderError::MissingApiKey)));
}

#[test]
fn from_json_error_to_provider_error() {
    let serde_err = serde_json::from_str::<serde_json::Value>("}").unwrap_err();
    let provider_err: ProviderError = ProviderError::JsonParse(serde_err);
    assert!(matches!(provider_err, ProviderError::JsonParse(_)));
}

// ---------------------------------------------------------------------------
// Error chain preserves context
// ---------------------------------------------------------------------------

#[test]
fn provider_error_chain_preserves_original_message() {
    let inner = ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(
        503,
        "service unavailable".into(),
    ));
    let top: Error = inner.into();
    let msg = top.to_string();
    assert!(msg.contains("503"), "should preserve status code: {msg}");
    assert!(
        msg.contains("service unavailable"),
        "should preserve reason: {msg}"
    );
}

#[test]
fn io_error_chain_through_provider() {
    let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
    let provider_err = ProviderError::IoError(io_err);
    let top: Error = provider_err.into();
    let msg = top.to_string();
    assert!(
        msg.contains("broken pipe"),
        "should preserve root cause: {msg}"
    );
}

#[test]
fn double_wrap_preserves_message() {
    let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "read timed out");
    let top: Error = ProviderError::IoError(io_err).into();
    let msg = top.to_string();
    assert!(
        msg.contains("read timed out"),
        "chain should preserve original message: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Config errors provide helpful messages
// ---------------------------------------------------------------------------

#[test]
fn missing_api_key_message_is_helpful() {
    let err = ProviderError::MissingApiKey;
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("api key"),
        "MissingApiKey message should mention 'api key': {msg}"
    );
}

#[test]
fn unknown_provider_message_contains_name() {
    let provider_name = "my-custom-provider";
    let err = ProviderError::UnknownProvider(provider_name.into());
    assert!(
        err.to_string().contains(provider_name),
        "should include the unknown provider name"
    );
}

#[test]
fn not_implemented_message_contains_name() {
    let provider_name = "bedrock";
    let err = ProviderError::NotImplemented(provider_name.into());
    assert!(
        err.to_string().contains(provider_name),
        "should include the unimplemented provider name"
    );
}

#[test]
fn invalid_api_key_format_message() {
    let err = ProviderError::InvalidApiKey;
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("api key"),
        "should mention api key: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("format"),
        "should mention format: {msg}"
    );
}

// Tools::ValidationError is the publicly exposed one
#[test]
fn tools_validation_missing_field_helpful() {
    let err = ToolsValidationError::SchemaValidation("missing required property 'model'".into());
    let msg = err.to_string();
    assert!(msg.contains("model"), "should mention field: {msg}");
}

#[test]
fn tools_validation_schema_error_helpful() {
    let detail = "additionalProperties: additional property 'foo' not allowed";
    let err = ToolsValidationError::SchemaValidation(detail.into());
    let msg = err.to_string();
    assert!(
        msg.contains("foo"),
        "should include schema error detail: {msg}"
    );
}

#[test]
fn tools_validation_invalid_json() {
    let serde_err = serde_json::from_str::<serde_json::Value>("}").unwrap_err();
    let err = ToolsValidationError::InvalidJson(serde_err);
    assert!(err.to_string().contains("Invalid JSON"));
}

// ---------------------------------------------------------------------------
// Debug formatting works (important for logging)
// ---------------------------------------------------------------------------

#[test]
fn provider_errors_have_debug_impl() {
    let variants: Vec<ProviderError> = vec![
        ProviderError::MissingApiKey,
        ProviderError::UnknownProvider("x".into()),
        ProviderError::NotImplemented("y".into()),
        ProviderError::HttpError(oxicode_ai::HttpErrorDetail::new(500, "err".into())),
        ProviderError::InvalidResponse("bad".into()),
        ProviderError::InvalidApiKey,
        ProviderError::StreamError("disconnected".into()),
    ];
    for v in &variants {
        let debug = format!("{:?}", v);
        assert!(!debug.is_empty(), "Debug should not be empty for {:?}", v);
    }
}

#[test]
fn top_level_error_debug_works() {
    let err = Error::Provider(ProviderError::MissingApiKey);
    let debug = format!("{:?}", err);
    assert!(!debug.is_empty());
}
