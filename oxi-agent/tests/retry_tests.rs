//! Tests for oxi-agent retry logic: circuit breaker, exponential backoff,
//! and retryable error classification.

use oxi_agent::agent_loop::config::{BACKOFF_BASE_SECS, MAX_RETRIES};
use oxi_agent::agent_loop::retry::is_retryable_error;
use oxi_agent::recovery::{
    CircuitBreaker, CircuitBreakerConfig, CircuitOpenError, FallbackChain, PartialResponse,
};
use std::time::Duration;

use oxi_ai::{
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
// CircuitBreaker: opens after threshold failures
// ---------------------------------------------------------------------------

#[test]
fn circuit_breaker_allows_requests_when_closed() {
    let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
    assert!(cb.allow_request().is_ok());
}

#[test]
fn circuit_breaker_opens_after_threshold() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        open_duration: Duration::from_secs(60),
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);

    // 2 failures — still closed
    cb.record_failure();
    cb.record_failure();
    assert!(cb.allow_request().is_ok());

    // 3rd failure — should open
    cb.record_failure();
    assert!(cb.allow_request().is_err());
}

#[test]
fn circuit_breaker_threshold_of_one() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(60),
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);
    cb.record_failure();
    assert!(cb.allow_request().is_err());
}

#[test]
fn circuit_breaker_default_threshold_is_five() {
    let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
    for _ in 0..4 {
        cb.record_failure();
    }
    // Not yet at threshold of 5
    assert!(cb.allow_request().is_ok());
    cb.record_failure();
    // Now at threshold of 5
    assert!(cb.allow_request().is_err());
}

// ---------------------------------------------------------------------------
// CircuitBreaker: resets after cooldown (transitions to half-open)
// ---------------------------------------------------------------------------

#[test]
fn circuit_breaker_manual_reset() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(60),
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);

    cb.record_failure();
    assert!(cb.allow_request().is_err());

    cb.reset();
    assert!(cb.allow_request().is_ok());
}

#[test]
fn circuit_breaker_half_open_allows_requests() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        // Use a non-tiny cooldown to avoid flaky timing on busy/slow CI.
        open_duration: Duration::from_millis(50), // short but stable
        half_open_successes: 2,
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);

    cb.record_failure(); // opens
    assert!(cb.allow_request().is_err());

    // Wait for cooldown
    std::thread::sleep(Duration::from_millis(60));

    // Should transition to half-open and allow
    assert!(cb.allow_request().is_ok());
}

#[test]
fn circuit_breaker_half_open_closes_after_enough_successes() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_millis(50),
        half_open_successes: 3,
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);

    cb.record_failure(); // opens
    std::thread::sleep(Duration::from_millis(60));
    let _ = cb.allow_request(); // transitions to half-open

    // Need half_open_successes (3) to close
    cb.record_success();
    cb.record_success();
    cb.record_success(); // 3rd success — should close

    // Now failure count is reset; needs threshold failures again
    // After closing, consecutive_failures is 0, so one failure won't open
    // but threshold is 1, so it actually opens immediately
    // Let's just verify the circuit is closed before any new failure
    assert!(
        cb.allow_request().is_ok(),
        "Should be closed after 3 successes"
    );
}

#[test]
fn circuit_breaker_half_open_reopens_on_failure() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_millis(50),
        half_open_successes: 3,
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);

    cb.record_failure(); // opens
    std::thread::sleep(Duration::from_millis(60));
    let _ = cb.allow_request(); // transitions to half-open

    cb.record_failure(); // reopens immediately from half-open
    assert!(cb.allow_request().is_err());
}

// ---------------------------------------------------------------------------
// CircuitBreaker: success resets failure count
// ---------------------------------------------------------------------------

#[test]
fn success_resets_consecutive_failures() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        open_duration: Duration::from_secs(60),
        ..Default::default()
    };
    let cb = CircuitBreaker::new(config);

    cb.record_failure();
    cb.record_failure();
    cb.record_success(); // resets failure count
    cb.record_failure();
    cb.record_failure();
    // Only 2 consecutive failures now, should still be closed
    assert!(cb.allow_request().is_ok());
}

// ---------------------------------------------------------------------------
// CircuitOpenError
// ---------------------------------------------------------------------------

#[test]
fn circuit_open_error_display() {
    let err = CircuitOpenError {
        remaining: Duration::from_secs(30),
    };
    let msg = err.to_string();
    assert!(msg.contains("retry after"), "should mention retry: {msg}");
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
    let attempt_0 = BACKOFF_BASE_SECS.pow(0u32 + 1);
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

// ---------------------------------------------------------------------------
// FallbackChain
// ---------------------------------------------------------------------------

#[test]
fn fallback_chain_default() {
    let chain = FallbackChain::default();
    assert!(!chain.is_empty());
    assert_eq!(chain.get(0), Some("openai/gpt-4o-mini"));
}

#[test]
fn fallback_chain_custom() {
    let chain = FallbackChain::new(vec!["anthropic/claude-3".into(), "openai/gpt-4".into()]);
    assert_eq!(chain.get(0), Some("anthropic/claude-3"));
    assert_eq!(chain.get(1), Some("openai/gpt-4"));
    assert_eq!(chain.get(2), None);
}

#[test]
fn fallback_chain_empty() {
    let chain = FallbackChain::new(vec![]);
    assert!(chain.is_empty());
}
