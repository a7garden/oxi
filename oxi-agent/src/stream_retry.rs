/// Shared streaming retry logic used by both [`Agent`](crate::Agent) and
/// [`AgentLoop`](crate::AgentLoop).
///
/// The core retry loop (exponential back-off, rate-limit detection) is
/// identical between the two agent implementations. This module factors
/// that logic into a single place so it can be tested once and reused.
use crate::error::AgentError;
use oxi_ai::{Context, Model, ProviderEvent, StreamOptions};
use std::time::Duration;

/// Maximum retry attempts for provider stream requests.
pub const MAX_RETRIES: usize = 3;

/// Base delay in seconds for exponential backoff.
pub const BACKOFF_BASE_SECS: u64 = 2;

/// Callback invoked each time a retry is about to happen.
///
/// The implementer can use this to emit events or log the retry.
pub trait RetryCallback: Send + Sync {
    /// Called before sleeping for `delay_secs`.
    fn on_retry(&self, attempt: usize, max_retries: usize, delay_secs: u64, reason: String);
}

/// Attempt to open a streaming connection to the provider with retry and
/// exponential back-off.
///
/// * `provider`   – the LLM provider to call.
/// * `model`      – resolved model descriptor.
/// * `context`    – conversation context (system prompt + messages + tools).
/// * `options`    – stream options (temperature, max_tokens …).
/// * `retry_cb`   – callback fired on each retry attempt.
/// * `max_delay`  – optional cap on the back-off delay (seconds).
pub async fn stream_with_retry_core(
    provider: &dyn oxi_ai::Provider,
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
    retry_cb: &dyn RetryCallback,
    max_delay: Option<u64>,
) -> Result<futures::stream::BoxStream<'static, ProviderEvent>, AgentError> {
    let mut last_err: Option<String> = None;

    for attempt in 0..=MAX_RETRIES {
        match provider.stream(model, context, options.clone()).await {
            Ok(stream) => {
                return Ok(stream as futures::stream::BoxStream<'static, ProviderEvent>);
            }
            Err(e) => {
                let msg = e.to_string();
                let is_rate_limit = e.http_status() == Some(429);
                let is_server_error = e.http_status().is_some_and(|code| code >= 500);
                let is_retryable = is_rate_limit
                    || is_server_error
                    || matches!(e, oxi_ai::ProviderError::RequestFailed(_));

                // A `MissingApiKey` is a *configuration* error, not a
                // transient upstream failure — fast-fail before any retry.
                if matches!(e, oxi_ai::ProviderError::MissingApiKey) {
                    return Err(AgentError::Stream(format!(
                        "{msg} — set the corresponding *_API_KEY env var or run `oxi setup`"
                    )));
                }

                if !is_retryable && attempt == 0 {
                    return Err(AgentError::Stream(msg));
                }

                last_err = Some(msg.clone());

                if attempt < MAX_RETRIES {
                    let mut delay = BACKOFF_BASE_SECS.pow(attempt as u32 + 1);
                    if let Some(cap) = max_delay {
                        delay = delay.min(cap);
                    }
                    retry_cb.on_retry(attempt + 1, MAX_RETRIES, delay, msg);
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
            }
        }
    }

    Err(AgentError::RetriesExhausted {
        attempts: MAX_RETRIES,
        last_error: last_err.unwrap_or_default(),
    })
}
