use crate::AgentEvent;
/// Retry logic for agent loop
use crate::stream_retry::{self, RetryCallback};
use anyhow::Result;
use oxicode_ai::{Context, Message, Model, ProviderEvent, StopReason, StreamOptions};
use regex::Regex;
use std::sync::atomic::Ordering;

pub use crate::stream_retry::{BACKOFF_BASE_SECS, MAX_RETRIES};

/// [`RetryCallback`] that emits [`AgentEvent::Retry`] through the AgentLoop emit function.
struct EmitRetryCallback<'a> {
    emit: &'a super::EmitFn,
    session_id: Option<String>,
}

impl RetryCallback for EmitRetryCallback<'_> {
    fn on_retry(&self, attempt: usize, max_retries: usize, delay_secs: u64, reason: String) {
        (self.emit)(AgentEvent::Retry {
            attempt,
            max_retries,
            retry_after_secs: delay_secs,
            reason,
            session_id: self.session_id.clone(),
        });
    }
}

/// Stream with automatic retry on transient provider errors.
///
/// Wraps [`stream_retry::stream_with_retry_core`] with per-session event emission.
pub(crate) async fn stream_with_retry(
    loop_ref: &super::AgentLoop,
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
    emit: &super::EmitFn,
) -> Result<futures::stream::BoxStream<'static, ProviderEvent>> {
    let cb = EmitRetryCallback {
        emit,
        session_id: loop_ref.session_id.clone(),
    };

    let provider = loop_ref.provider.as_ref();
    let max_delay = loop_ref.config.max_retry_delay_ms;
    let breaker = loop_ref.config.circuit_breaker.as_deref();

    let result = stream_retry::stream_with_retry_core_with_breaker(
        provider, model, context, options, &cb, max_delay, breaker,
    )
    .await;

    result.map_err(Into::into)
}

/// Detect whether an assistant message contains a retryable error.
pub fn is_retryable_error(message: &oxicode_ai::AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let err = match message.error_message.as_deref() {
        Some(e) if !e.is_empty() => e,
        _ => return false,
    };

    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        // SAFETY: the auto-retry regex is a compile-time literal that is
        // verified valid by `regex::Regex::new`; a panic here is a programming
        // error in the literal itself, not a runtime condition.
        #[allow(clippy::expect_used)]
        Regex::new(
            r"(?i)overloaded|provider.?returned.?error|rate.?limit|too many requests\
             |429|500|502|503|504|service.?unavailable|server.?error|internal.?error\
             |network.?error|connection.?error|connection.?refused|connection.?lost\
             |other side closed|fetch failed|upstream.?connect|reset before headers\
             |socket hang up|ended without|http2 request did not get a response\
             |timed? out|timeout|terminated|retry delay",
        )
        .expect("auto-retry regex should compile")
    });

    re.is_match(err)
}

/// Attempt an auto-retry for a retryable assistant error.
///
/// Uses [`tokio::sync::Notify`] to allow immediate cancellation of the retry
/// delay sleep, instead of waiting for the full delay to elapse.
pub(crate) async fn handle_retryable_error(
    loop_ref: &super::AgentLoop,
    message: &oxicode_ai::AssistantMessage,
    messages: &mut Vec<Message>,
    emit: &super::EmitFn,
) -> bool {
    if !loop_ref.auto_retry_enabled() {
        return false;
    }

    let attempt = loop_ref.auto_retry_attempt.fetch_add(1, Ordering::Relaxed) + 1;
    let max_attempts = loop_ref.config.auto_retry_max_attempts;

    if attempt > max_attempts {
        emit(AgentEvent::AutoRetryEnd {
            success: false,
            attempt: attempt - 1,
            final_error: message.error_message.clone(),
        });
        loop_ref.auto_retry_attempt.store(0, Ordering::Relaxed);
        return false;
    }

    let delay_ms = loop_ref.config.auto_retry_base_delay_ms * 2u64.pow((attempt - 1) as u32);

    emit(AgentEvent::AutoRetryStart {
        attempt,
        max_attempts,
        delay_ms,
        error_message: message
            .error_message
            .clone()
            .unwrap_or_else(|| "Unknown error".into()),
    });

    // Remove the error assistant message so we can retry with a clean context.
    if messages
        .last()
        .is_some_and(|m| matches!(m, Message::Assistant(_)))
    {
        messages.pop();
    }

    // Reset cancel flag before entering the wait.
    loop_ref.reset_auto_retry_cancel();

    // Wait with immediate wake-up via Notify.
    // If cancel_auto_retry() is called during the sleep, the Notify fires
    // and we wake up immediately instead of waiting for the full delay.
    tokio::select! {
        biased;
        _ = loop_ref.auto_retry_notify.notified() => {
            tracing::info!(attempt, "Auto-retry wait interrupted by cancellation");
        }
        _ = loop_ref.external_auto_retry_notified() => {
            tracing::info!(attempt, "Auto-retry wait interrupted by external cancellation");
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {
            // Normal delay elapsed — check cancel flag below.
        }
    }

    if loop_ref.auto_retry_cancelled() {
        emit(AgentEvent::AutoRetryEnd {
            success: false,
            attempt,
            final_error: Some("Retry cancelled".into()),
        });
        loop_ref.auto_retry_attempt.store(0, Ordering::Relaxed);
        return false;
    }

    true
}

/// Cancel any in-progress auto-retry wait.
///
/// Sets the cancel flag and fires the [`tokio::sync::Notify`] to immediately
/// wake up the retry delay sleep.
pub fn cancel_auto_retry(loop_ref: &super::AgentLoop) {
    loop_ref.fire_auto_retry_cancel();
}

/// Returns the current auto-retry attempt number (0 = no retry in progress).
pub fn auto_retry_attempt_method(loop_ref: &super::AgentLoop) -> usize {
    loop_ref.auto_retry_attempt.load(Ordering::Relaxed)
}
