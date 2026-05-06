/// Retry logic for agent loop

use crate::{AgentError, AgentEvent};
use anyhow::Result;
use oxi_ai::{Context, Model, ProviderEvent, StreamOptions, StopReason, Message};
use regex::Regex;
use std::pin::Pin;
use std::sync::atomic::Ordering;

pub use super::config::{BACKOFF_BASE_SECS, MAX_RETRIES};

/// Stream with automatic retry on transient provider errors.
pub(crate) async fn stream_with_retry(
    loop_ref: &super::AgentLoop,
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
    emit: &super::EmitFn,
) -> Result<Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>> {
    let mut last_err: Option<String> = None;

    for attempt in 0..=MAX_RETRIES {
        if let Err(open_err) = loop_ref.circuit_breaker.allow_request() {
            tracing::error!(session_id = ?loop_ref.session_id, "Circuit breaker open: {}", open_err);
            emit(AgentEvent::Error {
                message: format!("Circuit breaker open: {}", open_err),
                session_id: loop_ref.session_id.clone(),
            });
            return Err(AgentError::Stream(format!("Circuit breaker open: {}", open_err)).into());
        }

        match loop_ref.provider.stream(model, context, options.clone()).await {
            Ok(stream) => {
                loop_ref.circuit_breaker.record_success();
                return Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
            }
            Err(e) => {
                loop_ref.circuit_breaker.record_failure();
                let msg = e.to_string();
                let is_rate_limit = matches!(e, oxi_ai::ProviderError::HttpError(429, _));

                if !is_rate_limit && attempt == 0 {
                    return Err(AgentError::Stream(msg).into());
                }

                last_err = Some(msg.clone());

                if attempt < MAX_RETRIES {
                    let delay = BACKOFF_BASE_SECS.pow(attempt as u32 + 1);

                    let final_delay = if let Some(max_delay) = loop_ref.config.max_retry_delay_ms {
                        delay.min(max_delay)
                    } else {
                        delay
                    };

                    tracing::warn!(session_id = ?loop_ref.session_id, attempt, max_retries = MAX_RETRIES, "Retrying stream request: {}", msg);
                    emit(AgentEvent::Retry {
                        attempt: attempt + 1,
                        max_retries: MAX_RETRIES,
                        retry_after_secs: final_delay,
                        reason: msg.clone(),
                        session_id: loop_ref.session_id.clone(),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_secs(final_delay)).await;
                }
            }
        }
    }

    Err(AgentError::RetriesExhausted {
        attempts: MAX_RETRIES,
        last_error: last_err.unwrap_or_default(),
    }.into())
}

/// Detect whether an assistant message contains a retryable error.
pub fn is_retryable_error(message: &oxi_ai::AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let err = match message.error_message.as_deref() {
        Some(e) if !e.is_empty() => e,
        _ => return false,
    };

    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
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
pub(crate) async fn handle_retryable_error(
    loop_ref: &super::AgentLoop,
    message: &oxi_ai::AssistantMessage,
    messages: &mut Vec<Message>,
    emit: &super::EmitFn,
) -> bool {
    if !loop_ref.config.auto_retry_enabled {
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
        error_message: message.error_message.clone().unwrap_or_else(|| "Unknown error".into()),
    });

    if messages.last().map_or(false, |m| matches!(m, Message::Assistant(_))) {
        messages.pop();
    }

    *loop_ref.auto_retry_cancel.write() = false;

    tokio::select! {
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {}
        _ = tokio::task::yield_now() => {
            if *loop_ref.auto_retry_cancel.read() {
                emit(AgentEvent::AutoRetryEnd {
                    success: false,
                    attempt,
                    final_error: Some("Retry cancelled".into()),
                });
                loop_ref.auto_retry_attempt.store(0, Ordering::Relaxed);
                return false;
            }
        }
    }

    if *loop_ref.auto_retry_cancel.read() {
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
pub fn cancel_auto_retry(loop_ref: &super::AgentLoop) {
    *loop_ref.auto_retry_cancel.write() = true;
}

/// Returns the current auto-retry attempt number (0 = no retry in progress).
pub fn auto_retry_attempt_method(loop_ref: &super::AgentLoop) -> usize {
    loop_ref.auto_retry_attempt.load(Ordering::Relaxed)
}