/// Shared streaming retry logic used by both [`Agent`](crate::Agent) and
/// [`AgentLoop`](crate::AgentLoop).
///
/// The core retry loop (exponential back-off, rate-limit detection) is
/// identical between the two agent implementations. This module factors
/// that logic into a single place so it can be tested once and reused.
use crate::error::AgentError;
use oxi_ai::circuit_breaker::CircuitBreaker;
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
/// This is the non-breaking entry point — it delegates to
/// [`stream_with_retry_core_with_breaker`] with `breaker = None` (no circuit
/// breaking). Consumers that want circuit breaking call the `_with_breaker`
/// variant directly.
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
    stream_with_retry_core_with_breaker(
        provider, model, context, options, retry_cb, max_delay, None,
    )
    .await
}

/// Attempt to open a streaming connection to the provider with retry,
/// exponential back-off, and an optional circuit breaker.
///
/// Same contract as [`stream_with_retry_core`] plus:
///
/// * `breaker` – optional circuit breaker. Consulted before each provider
///   attempt (`check()`); an open circuit short-circuits the retry loop and
///   returns [`AgentError::Stream`] with a `breaker open:` prefix (NOT
///   retryable — that is the breaker's whole purpose: stop hammering a
///   failing upstream). On every successful call the breaker records
///   success; on every error it records failure. `None` = no circuit
///   breaking (identical to [`stream_with_retry_core`]).
///
/// This function is additive (`stream_with_retry_core` keeps its signature
/// and delegates here with `None`) so consumers on the old surface are
/// unaffected.
pub async fn stream_with_retry_core_with_breaker(
    provider: &dyn oxi_ai::Provider,
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
    retry_cb: &dyn RetryCallback,
    max_delay: Option<u64>,
    breaker: Option<&dyn CircuitBreaker>,
) -> Result<futures::stream::BoxStream<'static, ProviderEvent>, AgentError> {
    let mut last_err: Option<String> = None;

    for attempt in 0..=MAX_RETRIES {
        // Consumer-supplied circuit breaker. Runs BEFORE the provider call
        // so an open circuit short-circuits the retry loop (we don't burn
        // retries against a known-open upstream). Do NOT record a failure
        // here — the upstream didn't fail, we declined to call it.
        if let Some(b) = breaker
            && let Err(e) = b.check()
        {
            return Err(AgentError::Stream(format!(
                "breaker open: {e} (provider call refused by circuit breaker)"
            )));
        }

        match provider.stream(model, context, options.clone()).await {
            Ok(stream) => {
                if let Some(b) = breaker {
                    b.record_success();
                }
                return Ok(stream as futures::stream::BoxStream<'static, ProviderEvent>);
            }
            Err(e) => {
                if let Some(b) = breaker {
                    b.record_failure();
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_ai::circuit_breaker::DefaultCircuitBreaker;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal provider that either succeeds immediately or fails with a
    /// rate-limit error (retryable) so the retry loop is exercised.
    struct StubProvider {
        fail_with_429: bool,
        calls: AtomicUsize,
    }

    impl oxi_ai::Provider for StubProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a oxi_ai::Model,
            _context: &'a oxi_ai::Context,
            _options: Option<oxi_ai::StreamOptions>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = oxi_ai::StreamResult> + Send + 'a>>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_with_429 {
                Box::pin(async { Err(oxi_ai::ProviderError::RateLimited { retry_after: None }) })
            } else {
                Box::pin(async {
                    Ok(Box::pin(futures::stream::empty())
                        as futures::stream::BoxStream<
                            'static,
                            oxi_ai::ProviderEvent,
                        >)
                })
            }
        }
    }

    struct NoopCallback;
    impl RetryCallback for NoopCallback {
        fn on_retry(&self, _: usize, _: usize, _: u64, _: String) {}
    }

    fn model() -> oxi_ai::Model {
        oxi_ai::Model::new(
            "test-model",
            "test-model",
            oxi_ai::Api::AnthropicMessages,
            "test",
            "http://localhost:1",
        )
    }

    #[tokio::test]
    async fn open_breaker_short_circuits_without_calling_provider() {
        // A breaker that is already open must prevent ANY provider call —
        // the whole point of circuit breaking.
        let breaker = Arc::new(DefaultCircuitBreaker::new(1, Duration::from_secs(60)));
        breaker.record_failure(); // trip it open
        let provider = StubProvider {
            fail_with_429: false,
            calls: AtomicUsize::new(0),
        };
        let cb = NoopCallback;
        let ctx = oxi_ai::Context::new();

        let err = match stream_with_retry_core_with_breaker(
            &provider,
            &model(),
            &ctx,
            None,
            &cb,
            None,
            Some(breaker.as_ref()),
        )
        .await
        {
            Ok(_) => panic!("open breaker must refuse the call"),
            Err(e) => e,
        };

        assert!(
            err.to_string().contains("breaker open"),
            "expected breaker-open message, got: {err}"
        );
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            0,
            "provider must never be called when the circuit is open"
        );
    }

    #[tokio::test]
    async fn success_records_success_on_breaker() {
        let breaker = Arc::new(DefaultCircuitBreaker::new(2, Duration::from_secs(60)));
        let provider = StubProvider {
            fail_with_429: false,
            calls: AtomicUsize::new(0),
        };
        let cb = NoopCallback;
        let ctx = oxi_ai::Context::new();

        let _ = stream_with_retry_core_with_breaker(
            &provider,
            &model(),
            &ctx,
            None,
            &cb,
            None,
            Some(breaker.as_ref()),
        )
        .await
        .expect("successful stream");

        assert_eq!(
            breaker.failure_count(),
            0,
            "success must reset the breaker's failure count"
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failure_records_failure_on_breaker() {
        let breaker = Arc::new(DefaultCircuitBreaker::new(5, Duration::from_secs(60)));
        let provider = StubProvider {
            fail_with_429: true,
            calls: AtomicUsize::new(0),
        };
        let cb = NoopCallback;
        let ctx = oxi_ai::Context::new();

        // First call fails with 429 (retryable) -> breaker records a failure.
        let _ = stream_with_retry_core_with_breaker(
            &provider,
            &model(),
            &ctx,
            None,
            &cb,
            None,
            Some(breaker.as_ref()),
        )
        .await;

        assert_eq!(
            breaker.failure_count(),
            1,
            "failed provider call must be recorded on the breaker"
        );
    }

    #[tokio::test]
    async fn no_breaker_preserves_legacy_behavior() {
        // stream_with_retry_core (the old entry) must behave exactly as
        // before: no breaker consulted, provider called.
        let provider = StubProvider {
            fail_with_429: false,
            calls: AtomicUsize::new(0),
        };
        let cb = NoopCallback;
        let ctx = oxi_ai::Context::new();

        let _ = stream_with_retry_core(&provider, &model(), &ctx, None, &cb, None)
            .await
            .expect("legacy entry point still works");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
