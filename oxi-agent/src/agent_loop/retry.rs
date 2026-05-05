//! Retry logic for agent loop

use crate::{AgentError, AgentToolResult};
use anyhow::{Error, Result};
use oxi_ai::{Context, Model, ProviderEvent, StreamOptions};
use parking_lot::RwLock;
use regex::Regex;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::config::{BACKOFF_BASE_SECS, MAX_RETRIES};

impl super::AgentLoop {
    /// Stream with automatic retry on transient provider errors.
    pub(super) async fn stream_with_retry(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
        emit: &super::EmitFn,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>> {
        let mut last_err: Option<String> = None;

        for attempt in 0..=MAX_RETRIES {
            // Check the circuit breaker before each attempt.
            if let Err(open_err) = self.circuit_breaker.allow_request() {
                tracing::error!(session_id = ?self.session_id, "Circuit breaker open: {}", open_err);
                emit(super::AgentEvent::Error {
                    message: format!("Circuit breaker open: {}", open_err),
                    session_id: self.session_id.clone(),
                });
                return Err(AgentError::Stream(format!("Circuit breaker open: {}", open_err)).into());
            }

            match self.provider.stream(model, context, options.clone()).await {
                Ok(stream) => {
                    self.circuit_breaker.record_success();
                    return Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
                }
                Err(e) => {
                    self.circuit_breaker.record_failure();
                    let msg = e.to_string();
                    let is_rate_limit = matches!(e, oxi_ai::ProviderError::HttpError(429, _));

                    if !is_rate_limit && attempt == 0 {
                        return Err(AgentError::Stream(msg).into());
                    }

                    last_err = Some(msg.clone());

                    if attempt < MAX_RETRIES {
                        let delay = BACKOFF_BASE_SECS.pow(attempt as u32 + 1);

                        let final_delay = if let Some(max_delay) = self.config.max_retry_delay_ms {
                            delay.min(max_delay)
                        } else {
                            delay
                        };

                        tracing::warn!(session_id = ?self.session_id, attempt, max_retries = MAX_RETRIES, "Retrying stream request: {}", msg);
                        emit(super::AgentEvent::Retry {
                            attempt: attempt + 1,
                            max_retries: MAX_RETRIES,
                            retry_after_secs: final_delay,
                            reason: msg.clone(),
                            session_id: self.session_id.clone(),
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
    ///
    /// Checks that the stop_reason is `Error`, that an `error_message` is present,
    /// and that the error text matches known retryable patterns
    /// (overloaded, rate-limit, 5xx, network errors, timeouts, etc.).
    pub(super) fn is_retryable_error(message: &oxi_ai::AssistantMessage) -> bool {
        if message.stop_reason != oxi_ai::StopReason::Error {
            return false;
        }
        let err = match message.error_message.as_deref() {
            Some(e) if !e.is_empty() => e,
            _ => return false,
        };

        // Lazy-init a static regex so it's compiled only once.
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
    ///
    /// Returns `true` if a retry was initiated (the caller should *not*
    /// proceed to compaction/finish), or `false` if retries are disabled,
    /// max attempts were exceeded, or the retry was cancelled.
    pub(super) async fn handle_retryable_error(
        &self,
        message: &oxi_ai::AssistantMessage,
        messages: &mut Vec<oxi_ai::Message>,
        emit: &super::EmitFn,
    ) -> bool {
        if !self.config.auto_retry_enabled {
            return false;
        }

        let attempt = self.auto_retry_attempt.fetch_add(1, Ordering::Relaxed) + 1;
        let max_attempts = self.config.auto_retry_max_attempts;

        if attempt > max_attempts {
            // Exhausted all retries - emit final failure and reset.
            emit(super::AgentEvent::AutoRetryEnd {
                success: false,
                attempt: attempt - 1,
                final_error: message.error_message.clone(),
            });
            self.auto_retry_attempt.store(0, Ordering::Relaxed);
            return false;
        }

        let delay_ms = self.config.auto_retry_base_delay_ms * 2u64.pow((attempt - 1) as u32);

        emit(super::AgentEvent::AutoRetryStart {
            attempt,
            max_attempts,
            delay_ms,
            error_message: message.error_message.clone().unwrap_or_else(|| "Unknown error".into()),
        });

        // Remove the error assistant message from the conversation so the
        // next LLM call doesn't see it (keep it in session history via
        // the emitted events).
        if messages
            .last()
            .map_or(false, |m| matches!(m, oxi_ai::Message::Assistant(_)))
        {
            messages.pop();
        }

        // Reset cancellation flag.
        *self.auto_retry_cancel.write() = false;

        // Wait with exponential backoff (cancellable).
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {
                // Sleep completed normally - proceed to retry.
            }
            _ = tokio::task::yield_now() => {
                // Check cancellation.
                if *self.auto_retry_cancel.read() {
                    emit(super::AgentEvent::AutoRetryEnd {
                        success: false,
                        attempt,
                        final_error: Some("Retry cancelled".into()),
                    });
                    self.auto_retry_attempt.store(0, Ordering::Relaxed);
                    return false;
                }
            }
        }

        // If cancelled during sleep, bail out.
        if *self.auto_retry_cancel.read() {
            emit(super::AgentEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: Some("Retry cancelled".into()),
            });
            self.auto_retry_attempt.store(0, Ordering::Relaxed);
            return false;
        }

        true // Caller should retry the LLM call.
    }

    /// Cancel any in-progress auto-retry wait.
    pub fn cancel_auto_retry(&self) {
        *self.auto_retry_cancel.write() = true;
    }

    /// Returns the current auto-retry attempt number (0 = no retry in progress).
    pub fn auto_retry_attempt(&self) -> usize {
        self.auto_retry_attempt.load(Ordering::Relaxed)
    }
}