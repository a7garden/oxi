//! Error recovery and circuit breaker for the agent runtime.
//!
//! Provides:
//! - **Circuit breaker**: Tracks consecutive failures and opens the circuit
//!   after a threshold, preventing further requests during a cooldown period.
//! - **Partial response recovery**: When a stream fails mid-response, the
//!   accumulated text is preserved and delivered as a partial result.
//! - **Graceful degradation**: When the primary model fails, falls back
//!   through a configured chain of models.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// Circuit breaker states.
#[repr(u8)]
enum CircuitState {
    /// Normal operation — requests pass through.
    Closed = 0,
    /// Too many failures — requests are rejected.
    Open = 1,
    /// Testing whether the downstream service has recovered.
    HalfOpen = 2,
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// How long to stay open before transitioning to half-open.
    pub open_duration: Duration,
    /// How many successes in half-open state before closing.
    pub half_open_successes: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            open_duration: Duration::from_secs(30),
            half_open_successes: 1,
        }
    }
}

/// A lock-free circuit breaker.
///
/// Thread-safe via atomic operations — no mutex needed.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: AtomicU8,
    consecutive_failures: AtomicU64,
    consecutive_successes: AtomicU64,
    opened_at: parking_lot::Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: AtomicU8::new(CircuitState::Closed as u8),
            consecutive_failures: AtomicU64::new(0),
            consecutive_successes: AtomicU64::new(0),
            opened_at: parking_lot::Mutex::new(None),
        }
    }

    /// Check if a request is allowed to proceed.
    pub fn allow_request(&self) -> Result<(), CircuitOpenError> {
        let state = self.load_state();
        match state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                let opened_at = self.opened_at.lock();
                if let Some(t) = *opened_at {
                    if t.elapsed() >= self.config.open_duration {
                        drop(opened_at);
                        self.state.store(CircuitState::HalfOpen as u8, Ordering::SeqCst);
                        self.consecutive_successes.store(0, Ordering::SeqCst);
                        return Ok(());
                    }
                }
                Err(CircuitOpenError {
                    remaining: self.config.open_duration.saturating_sub(
                        opened_at.map(|t| t.elapsed()).unwrap_or_default(),
                    ),
                })
            }
            CircuitState::HalfOpen => Ok(()),
        }
    }

    /// Record a successful request.
    pub fn record_success(&self) {
        let state = self.load_state();
        match state {
            CircuitState::Closed => {
                self.consecutive_failures.store(0, Ordering::SeqCst);
            }
            CircuitState::HalfOpen => {
                let prev = self.consecutive_successes.fetch_add(1, Ordering::SeqCst);
                if prev + 1 >= self.config.half_open_successes as u64 {
                    self.state.store(CircuitState::Closed as u8, Ordering::SeqCst);
                    self.consecutive_failures.store(0, Ordering::SeqCst);
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failed request.
    pub fn record_failure(&self) {
        let state = self.load_state();
        match state {
            CircuitState::Closed => {
                let prev = self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
                if prev + 1 >= self.config.failure_threshold as u64 {
                    self.state.store(CircuitState::Open as u8, Ordering::SeqCst);
                    *self.opened_at.lock() = Some(Instant::now());
                }
            }
            CircuitState::HalfOpen => {
                self.state.store(CircuitState::Open as u8, Ordering::SeqCst);
                *self.opened_at.lock() = Some(Instant::now());
            }
            CircuitState::Open => {}
        }
    }

    /// Get the current number of consecutive failures.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst) as u32
    }

    /// Reset the circuit breaker to closed state.
    pub fn reset(&self) {
        self.state.store(CircuitState::Closed as u8, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
        *self.opened_at.lock() = None;
    }

    fn load_state(&self) -> CircuitState {
        match self.state.load(Ordering::SeqCst) {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            _ => CircuitState::HalfOpen,
        }
    }
}

/// Error returned when the circuit breaker is open.
#[derive(Debug, thiserror::Error)]
#[error("Circuit is open — retry after {remaining:?}")]
pub struct CircuitOpenError {
    pub remaining: Duration,
}

// ---------------------------------------------------------------------------

/// Partial response accumulator.
///
/// Collects streamed text deltas so that if the stream fails partway through,
/// whatever was received so far can still be delivered.
#[derive(Debug, Default)]
pub struct PartialResponse {
    text: String,
    thinking: String,
    has_thinking: bool,
}

impl PartialResponse {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a text delta.
    pub fn push_text(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    /// Append a thinking delta.
    pub fn push_thinking(&mut self, delta: &str) {
        self.has_thinking = true;
        self.thinking.push_str(delta);
    }

    /// Take the accumulated text, replacing it with empty.
    pub fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    /// Get the accumulated text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Get the accumulated thinking.
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    /// Whether any thinking content was received.
    pub fn has_thinking(&self) -> bool {
        self.has_thinking
    }

    /// Whether any content was received.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.thinking.is_empty()
    }

    /// Clear the accumulator.
    pub fn clear(&mut self) {
        self.text.clear();
        self.thinking.clear();
        self.has_thinking = false;
    }
}

// ---------------------------------------------------------------------------

/// Fallback model chain.
///
/// When the primary model fails, each fallback model in the chain is tried
/// in order. The first successful response wins.
#[derive(Debug, Clone)]
pub struct FallbackChain {
    /// Ordered list of fallback model IDs (e.g. `["openai/gpt-4o-mini", "deepseek/deepseek-chat"]`).
    pub models: Vec<String>,
}

impl Default for FallbackChain {
    fn default() -> Self {
        Self {
            models: vec!["openai/gpt-4o-mini".to_string()],
        }
    }
}

impl FallbackChain {
    pub fn new(models: Vec<String>) -> Self {
        Self { models }
    }

    /// Get the fallback model at the given index.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.models.get(index).map(|s| s.as_str())
    }

    /// Number of fallback models in the chain.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_breaker_allows_when_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request().is_ok());

        cb.record_failure();
        assert!(cb.allow_request().is_err());
    }

    #[test]
    fn circuit_breaker_resets() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);
        cb.record_failure();
        assert!(cb.allow_request().is_err());
        cb.reset();
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn partial_response_accumulates() {
        let mut pr = PartialResponse::new();
        pr.push_text("Hello ");
        pr.push_text("world");
        assert_eq!(pr.text(), "Hello world");
        assert!(!pr.take_text().is_empty());
        assert!(pr.text().is_empty());
    }

    #[test]
    fn partial_response_thinking() {
        let mut pr = PartialResponse::new();
        pr.push_thinking("Let me think...");
        pr.push_text("Here's the answer");
        assert!(pr.has_thinking());
        assert_eq!(pr.thinking(), "Let me think...");
    }

    #[test]
    fn fallback_chain() {
        let chain = FallbackChain::default();
        assert_eq!(chain.get(0), Some("openai/gpt-4o-mini"));
        assert_eq!(chain.get(1), None);
        assert!(!chain.is_empty());
    }
}
