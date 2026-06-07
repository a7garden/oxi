//! Provider pool — rate limiting and concurrency control for provider calls.
//!
//! Wraps any `Provider` with a semaphore (max concurrent requests) and
//! a simple sliding-window rate limiter (RPM). Used when multiple agents
//! share a single API key and need coordinated access.

use crate::{Context, Model, Provider, ProviderError, StreamOptions, StreamResult};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Rate-limiting and concurrency policy for a provider pool.
#[derive(Debug, Clone)]
pub struct RateLimitPolicy {
    /// Maximum requests per minute.
    pub rpm: u32,
    /// Maximum number of concurrent in-flight requests.
    pub max_concurrent: usize,
}

impl RateLimitPolicy {
    /// Create a policy with the given RPM.
    ///
    /// Sets `max_concurrent` to `rpm / 6` (distributed over 10-second windows).
    pub fn rpm(rpm: u32) -> Self {
        Self {
            rpm,
            max_concurrent: (rpm as usize / 6).max(1),
        }
    }

    /// Create a policy with the given requests-per-second.
    pub fn per_second(rps: u32) -> Self {
        Self {
            rpm: rps * 60,
            max_concurrent: rps as usize,
        }
    }

    /// Create an unrestricted policy (no rate limiting).
    pub fn unlimited() -> Self {
        Self {
            rpm: u32::MAX,
            max_concurrent: usize::MAX,
        }
    }

    /// Set a custom max-concurrency override.
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }
}

/// Internal rate-limiter state (sliding window).
struct RateLimiterState {
    rpm: u32,
    timestamps: Vec<Instant>,
}

impl RateLimiterState {
    fn new(rpm: u32) -> Self {
        Self {
            rpm,
            timestamps: Vec::with_capacity(64),
        }
    }

    /// Remove timestamps older than 60 seconds.
    fn prune(&mut self) {
        let cutoff = Instant::now() - Duration::from_secs(60);
        self.timestamps.retain(|&t| t > cutoff);
    }

    /// Check if a new request can be made within the RPM limit.
    fn can_proceed(&mut self) -> bool {
        self.prune();
        (self.timestamps.len() as u32) < self.rpm
    }

    /// Record a request timestamp.
    fn record(&mut self) {
        self.timestamps.push(Instant::now());
    }
}

/// A `Provider` wrapper that enforces rate limits and concurrency.
///
/// Multiple agents sharing an API key should go through a single
/// `ProviderPool` instance to avoid exceeding rate limits.
pub struct ProviderPool {
    inner: Arc<dyn Provider>,
    semaphore: Arc<Semaphore>,
    limiter: Arc<tokio::sync::Mutex<RateLimiterState>>,
    pool_name: String,
}

impl ProviderPool {
    /// Create a new pool wrapping the given provider with the specified policy.
    pub fn new(
        provider: Arc<dyn Provider>,
        policy: RateLimitPolicy,
        name: impl Into<String>,
    ) -> Self {
        Self {
            inner: provider,
            semaphore: Arc::new(Semaphore::new(policy.max_concurrent)),
            limiter: Arc::new(tokio::sync::Mutex::new(RateLimiterState::new(policy.rpm))),
            pool_name: name.into(),
        }
    }
}

impl Provider for ProviderPool {
    fn name(&self) -> &str {
        &self.pool_name
    }

    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            // 1. Acquire concurrency permit
            let _permit =
                self.semaphore
                    .acquire()
                    .await
                    .map_err(|_| ProviderError::RateLimited {
                        retry_after: Some(Duration::from_secs(5)),
                    })?;

            // 2. Rate-limit check with simple backoff
            {
                let mut limiter = self.limiter.lock().await;
                if !limiter.can_proceed() {
                    // Simple: wait 1 second and retry once
                    drop(limiter);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let mut limiter = self.limiter.lock().await;
                    if !limiter.can_proceed() {
                        return Err(ProviderError::RateLimited {
                            retry_after: Some(Duration::from_secs(5)),
                        });
                    }
                    limiter.record();
                } else {
                    limiter.record();
                }
            }

            // 3. Delegate to inner provider
            self.inner.stream(model, context, options).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_policy_rpm() {
        let policy = RateLimitPolicy::rpm(60);
        assert_eq!(policy.rpm, 60);
        assert_eq!(policy.max_concurrent, 10); // 60/6
    }

    #[test]
    fn test_rate_limit_policy_unlimited() {
        let policy = RateLimitPolicy::unlimited();
        assert_eq!(policy.rpm, u32::MAX);
        assert_eq!(policy.max_concurrent, usize::MAX);
    }

    #[test]
    fn test_rate_limit_policy_custom_concurrency() {
        let policy = RateLimitPolicy::rpm(60).with_max_concurrent(3);
        assert_eq!(policy.max_concurrent, 3);
    }

    #[tokio::test]
    async fn test_rate_limiter_state_allows_within_limit() {
        let mut state = RateLimiterState::new(5);
        assert!(state.can_proceed());
        state.record();
        assert!(state.can_proceed());
    }

    #[tokio::test]
    async fn test_rate_limiter_state_blocks_at_limit() {
        let mut state = RateLimiterState::new(2);
        assert!(state.can_proceed());
        state.record();
        assert!(state.can_proceed());
        state.record();
        assert!(!state.can_proceed()); // at limit
    }
}
