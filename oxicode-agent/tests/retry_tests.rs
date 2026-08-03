//! Tests for oxicode-agent retry logic: exponential backoff,
//! retryable error classification, and partial response accumulation.

use oxicode_agent::agent_loop::config::{BACKOFF_BASE_SECS, MAX_RETRIES};
use oxicode_agent::agent_loop::retry::is_retryable_error;
use oxicode_agent::recovery::PartialResponse;

use oxicode_ai::{
    Api, AssistantMessage, AssistantRole, ContentBlock, StopReason, TextContent, TextContentType,
    Usage,
};

// ---------------------------------------------------------------------------
// Helper: build an AssistantMessage with a specific stop_reason + error
// ---------------------------------------------------------------------------

fn make_error_message(error: &str) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::Text(TextContent {
            content_type: TextContentType::Text,
            text: String::new(),
            text_signature: None,
        })],
        api: Api::OpenAiCompletions,
        provider: "test".into(),
        model: "test-model".into(),
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some(error.to_string()),
        response_id: None,
        timestamp: 0,
    }
}

fn make_normal_message() -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::Text(TextContent {
            content_type: TextContentType::Text,
            text: "Hello".into(),
            text_signature: None,
        })],
        api: Api::OpenAiCompletions,
        provider: "test".into(),
        model: "test-model".into(),
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        response_id: None,
        timestamp: 0,
    }
}

// ---------------------------------------------------------------------------
// Exponential backoff calculation
// ---------------------------------------------------------------------------

#[test]
fn backoff_constants_are_sane() {
    assert_eq!(BACKOFF_BASE_SECS, 2, "base should be 2 seconds");
    assert_eq!(MAX_RETRIES, 3, "max retries should be 3");
}

#[test]
fn backoff_doubles_each_attempt() {
    // Attempt 0 → 2^1 = 2s, Attempt 1 → 2^2 = 4s, Attempt 2 → 2^3 = 8s
    let attempt_0 = BACKOFF_BASE_SECS.pow(1);
    let attempt_1 = BACKOFF_BASE_SECS.pow(1u32 + 1);
    let attempt_2 = BACKOFF_BASE_SECS.pow(2u32 + 1);

    assert_eq!(attempt_0, 2);
    assert_eq!(attempt_1, 4);
    assert_eq!(attempt_2, 8);
}

#[test]
fn backoff_grows_exponentially() {
    let delays: Vec<u64> = (0..=MAX_RETRIES)
        .map(|attempt| BACKOFF_BASE_SECS.pow(attempt as u32 + 1))
        .collect();
    for window in delays.windows(2) {
        assert!(window[1] > window[0], "Delay should increase: {:?}", delays);
    }
}

// ---------------------------------------------------------------------------
// is_retryable_error: correct classification
// ---------------------------------------------------------------------------

#[test]
fn retryable_overloaded() {
    assert!(is_retryable_error(&make_error_message(
        "server is overloaded"
    )));
}

#[test]
fn retryable_rate_limit() {
    assert!(is_retryable_error(&make_error_message(
        "rate limit exceeded"
    )));
}

#[test]
fn retryable_429() {
    assert!(is_retryable_error(&make_error_message(
        "HTTP 429 Too Many Requests"
    )));
}

#[test]
fn retryable_500() {
    assert!(is_retryable_error(&make_error_message(
        "500 Internal Server Error"
    )));
}

#[test]
fn retryable_502() {
    assert!(is_retryable_error(&make_error_message("502 Bad Gateway")));
}

#[test]
fn retryable_503() {
    assert!(is_retryable_error(&make_error_message(
        "503 Service Unavailable"
    )));
}

#[test]
fn retryable_504() {
    assert!(is_retryable_error(&make_error_message(
        "504 Gateway Timeout"
    )));
}

#[test]
fn retryable_timeout() {
    assert!(is_retryable_error(&make_error_message("request timed out")));
}

#[test]
fn retryable_connection_refused() {
    assert!(is_retryable_error(&make_error_message(
        "connection refused"
    )));
}

#[test]
fn retryable_network_error() {
    assert!(is_retryable_error(&make_error_message("network error")));
}

#[test]
fn retryable_provider_returned_error() {
    assert!(is_retryable_error(&make_error_message(
        "provider returned error: upstream failure"
    )));
}

#[test]
fn retryable_socket_hang_up() {
    assert!(is_retryable_error(&make_error_message("socket hang up")));
}

#[test]
fn retryable_fetch_failed() {
    assert!(is_retryable_error(&make_error_message("fetch failed")));
}

#[test]
fn retryable_server_error() {
    assert!(is_retryable_error(&make_error_message("server error")));
}

#[test]
fn retryable_case_insensitive() {
    assert!(is_retryable_error(&make_error_message("RATE LIMIT")));
    assert!(is_retryable_error(&make_error_message("Overloaded")));
    assert!(is_retryable_error(&make_error_message("TIMEOUT")));
}

#[test]
fn retryable_service_unavailable() {
    assert!(is_retryable_error(&make_error_message(
        "service unavailable"
    )));
}

#[test]
fn retryable_other_side_closed() {
    assert!(is_retryable_error(&make_error_message("other side closed")));
}

#[test]
fn retryable_connection_error() {
    assert!(is_retryable_error(&make_error_message("connection error")));
}

#[test]
fn retryable_timedout() {
    assert!(is_retryable_error(&make_error_message("request timed out")));
}

#[test]
fn retryable_terminated() {
    assert!(is_retryable_error(&make_error_message(
        "connection terminated"
    )));
}

// ---------------------------------------------------------------------------
// is_retryable_error: non-retryable cases
// ---------------------------------------------------------------------------

#[test]
fn non_retryable_normal_stop() {
    assert!(!is_retryable_error(&make_normal_message()));
}

#[test]
fn non_retryable_no_error_message() {
    let msg = AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::Text(TextContent {
            content_type: TextContentType::Text,
            text: String::new(),
            text_signature: None,
        })],
        api: Api::OpenAiCompletions,
        provider: "test".into(),
        model: "test-model".into(),
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        error_message: None,
        response_id: None,
        timestamp: 0,
    };
    assert!(!is_retryable_error(&msg));
}

#[test]
fn non_retryable_empty_error_message() {
    let msg = AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::Text(TextContent {
            content_type: TextContentType::Text,
            text: String::new(),
            text_signature: None,
        })],
        api: Api::OpenAiCompletions,
        provider: "test".into(),
        model: "test-model".into(),
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some(String::new()),
        response_id: None,
        timestamp: 0,
    };
    assert!(!is_retryable_error(&msg));
}

#[test]
fn non_retryable_wrong_stop_reason() {
    let msg = AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::Text(TextContent {
            content_type: TextContentType::Text,
            text: String::new(),
            text_signature: None,
        })],
        api: Api::OpenAiCompletions,
        provider: "test".into(),
        model: "test-model".into(),
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: Some("rate limit exceeded".into()),
        response_id: None,
        timestamp: 0,
    };
    // Even though message says "rate limit", stop_reason is Stop, not Error
    assert!(!is_retryable_error(&msg));
}

#[test]
fn non_retryable_unknown_error_text() {
    assert!(!is_retryable_error(&make_error_message(
        "user asked to cancel"
    )));
    assert!(!is_retryable_error(&make_error_message(
        "invalid JSON payload"
    )));
    assert!(!is_retryable_error(&make_error_message(
        "authentication failed"
    )));
}

// ---------------------------------------------------------------------------
// PartialResponse
// ---------------------------------------------------------------------------

#[test]
fn partial_response_accumulates_text() {
    let mut pr = PartialResponse::new();
    assert!(pr.is_empty());
    pr.push_text("Hello ");
    pr.push_text("World");
    assert_eq!(pr.text(), "Hello World");
    assert!(!pr.is_empty());
}

#[test]
fn partial_response_take_text() {
    let mut pr = PartialResponse::new();
    pr.push_text("content");
    let taken = pr.take_text();
    assert_eq!(taken, "content");
    assert!(pr.text().is_empty());
}

#[test]
fn partial_response_thinking() {
    let mut pr = PartialResponse::new();
    assert!(!pr.has_thinking());
    pr.push_thinking("hmm");
    assert!(pr.has_thinking());
    assert_eq!(pr.thinking(), "hmm");
}

#[test]
fn partial_response_clear() {
    let mut pr = PartialResponse::new();
    pr.push_text("text");
    pr.push_thinking("think");
    pr.clear();
    assert!(pr.is_empty());
    assert!(!pr.has_thinking());
}
