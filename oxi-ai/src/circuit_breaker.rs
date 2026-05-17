//! Per-provider circuit breaker implementation.
//!
//! This module provides a thread-safe circuit breaker pattern for managing
//! provider failures in the oxi-ai library. Each provider can have its own
//! circuit breaker instance that prevents cascading failures by temporarily
//! blocking requests to unhealthy providers.
//!
//! # Circuit States
//!
//! - **Closed**: Normal operation. All requests are allowed through.
//!   Failures are counted, and the circuit opens after reaching the threshold.
//!
//! - **Open**: The provider is considered unhealthy. Requests are blocked
//!   for a configurable duration, then the circuit transitions to half-open
//!   to test recovery.
//!
//! - **Half-Open**: Recovery testing mode. A limited number of requests
//!   are allowed to test if the provider has recovered. If enough succeed,
//!   the circuit closes; if any fail, the circuit reopens.
//!
//! # Example
//!
//! ```rust
//! use std::time::Duration;
//! use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
//!
//! let config = CircuitBreakerConfig::default();
//! let breaker = ProviderCircuitBreaker::new("openai".to_string(), config);
//!
//! // Check if request is allowed
//! match breaker.allow_request() {
//!     Ok(()) => { /* proceed with request */ }
//!     Err(e) => { /* circuit is open, retry after e.remaining */ }
//! }
//! ```
//!
//! # Thread Safety
//!
//! All state is managed using atomic operations and parking_lot mutex,
//! making this implementation safe for concurrent access from multiple
//! async tasks or threads.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;

// ============================================================================
// Circuit State
// ============================================================================

/// Circuit breaker states.
///
/// The state is stored as a `u8` in an atomic, so these values correspond
/// to the numeric representation (0, 1, 2) for efficient atomic operations.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation. All requests are allowed through.
    /// Failures increment the consecutive failure counter.
    Closed = 0,

    /// Provider is unhealthy. Requests are blocked for a configured duration.
    /// After the duration elapses, transitions to `HalfOpen`.
    Open = 1,

    /// Recovery testing mode. Limited requests are allowed.
    /// Successes are counted; circuit closes after `half_open_successes` succeed.
    /// Any failure reopens the circuit.
    HalfOpen = 2,
}

impl CircuitState {
    /// Convert a raw u8 value to a `CircuitState`.
    ///
    /// Returns `CircuitState::HalfOpen` for any value >= 2 to handle
    /// potential future state additions gracefully.
    #[inline]
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Closed,
            1 => Self::Open,
            _ => Self::HalfOpen,
        }
    }

    /// Convert `CircuitState` to its numeric representation.
    #[inline]
    fn as_u8(&self) -> u8 {
        *self as u8
    }
}

// ============================================================================
// Circuit Breaker Configuration
// ============================================================================

/// Configuration parameters for a provider circuit breaker.
///
/// All parameters can be tuned based on the provider's reliability and
/// the acceptable impact of failures on your application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures required to open the circuit.
    ///
    /// Default: 5
    ///
    /// A lower value makes the circuit more sensitive to failures,
    /// while a higher value requires more failures before opening.
    pub failure_threshold: u32,

    /// Duration to keep the circuit open before transitioning to half-open.
    ///
    /// Default: 30 seconds
    ///
    /// This should be long enough for the provider to recover from
    /// whatever caused the failures (e.g., rate limits, temporary outages).
    pub open_duration: Duration,

    /// Number of successful requests required in half-open state to close the circuit.
    ///
    /// Default: 1
    ///
    /// Setting this higher makes recovery testing more conservative,
    /// requiring multiple successful requests before fully trusting the provider.
    pub half_open_successes: u32,
}

impl Default for CircuitBreakerConfig {
    /// Creates a default circuit breaker configuration.
    ///
    /// The defaults are tuned for general-purpose use:
    /// - 5 failures before opening (reasonable for most APIs)
    /// - 30 second cooldown (enough for temporary issues to resolve)
    /// - 1 success to close (fast recovery testing)
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            open_duration: Duration::from_secs(30),
            half_open_successes: 1,
        }
    }
}

impl CircuitBreakerConfig {
    /// Creates a new configuration with all values explicitly set.
    ///
    /// # Arguments
    ///
    /// * `failure_threshold` - Consecutive failures to trigger circuit opening
    /// * `open_duration` - Time to wait before testing recovery
    /// * `half_open_successes` - Successes needed in half-open to close circuit
    ///
    /// # Panics
    ///
    /// Panics if `failure_threshold` or `half_open_successes` are zero,
    /// as this would create an immediately opening circuit.
    #[inline]
    pub fn new(failure_threshold: u32, open_duration: Duration, half_open_successes: u32) -> Self {
        if failure_threshold == 0 {
            panic!("failure_threshold cannot be zero");
        }
        if half_open_successes == 0 {
            panic!("half_open_successes cannot be zero");
        }
        Self {
            failure_threshold,
            open_duration,
            half_open_successes,
        }
    }

    /// Sets the failure threshold.
    ///
    /// Returns a new configuration with the updated value.
    #[inline]
    #[must_use]
    pub fn with_failure_threshold(mut self, threshold: u32) -> Self {
        self.failure_threshold = threshold;
        self
    }

    /// Sets the open duration.
    ///
    /// Returns a new configuration with the updated value.
    #[inline]
    #[must_use]
    pub fn with_open_duration(mut self, duration: Duration) -> Self {
        self.open_duration = duration;
        self
    }

    /// Sets the half-open successes required.
    ///
    /// Returns a new configuration with the updated value.
    #[inline]
    #[must_use]
    pub fn with_half_open_successes(mut self, successes: u32) -> Self {
        self.half_open_successes = successes;
        self
    }
}

// ============================================================================
// Circuit Open Error
// ============================================================================

/// Error returned when attempting to make a request while the circuit is open.
///
/// This error indicates that the circuit breaker has blocked the request
/// because the provider is considered unhealthy. The `remaining` field
/// indicates how long you should wait before attempting another request.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("Circuit breaker open for provider '{provider}': retry after {remaining:?}")]
pub struct CircuitOpenError {
    /// The name of the provider whose circuit is open.
    pub provider: String,
    /// Time remaining before the circuit transitions to half-open.
    pub remaining: Duration,
}

impl CircuitOpenError {
    /// Creates a new circuit open error for the given provider and duration.
    #[inline]
    pub fn new(provider: impl Into<String>, remaining: Duration) -> Self {
        Self {
            provider: provider.into(),
            remaining,
        }
    }
}

// ============================================================================
// Provider Circuit Breaker
// ============================================================================

/// A per-provider circuit breaker for preventing cascading failures.
///
/// This struct manages the state machine for a single provider's circuit breaker.
/// It tracks consecutive failures and successes, manages state transitions,
/// and determines whether requests should be allowed.
///
/// # Thread Safety
///
/// All operations are thread-safe and can be called concurrently from
/// multiple async tasks or threads. The implementation uses atomic
/// operations for the fast path (checking state) and a mutex only for
/// the timestamp update when opening the circuit.
///
/// # State Machine
///
/// ```text
/// ┌─────────┐  failure_threshold reached   ┌────────┐
/// │ Closed  │ ───────────────────────────►  │  Open  │
/// └────┬────┘                               └───┬────┘
///      │                                         │
///      │ record_success()                         │ open_duration elapsed
///      │ (reset failures to 0)                    ▼
///      │                                   ┌───────────┐
///      │                                   │ Half-Open │
///      │                                   └─────┬─────┘
///      │                                         │
///      │                              half_open_successes reached
///      └─────────────────────────────────────────┘
///      │                                         ▲
///      │         any failure                     │
///      └─────────────────────────────────────────┘
/// ```
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
/// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
///
/// let config = CircuitBreakerConfig::default();
/// let breaker = ProviderCircuitBreaker::new("anthropic".to_string(), config);
///
/// // Check if a request is allowed
/// match breaker.allow_request() {
///     Ok(()) => {
///         // Proceed with the request
///     }
///     Err(e) => {
///         println!("Circuit open: {}", e);
///     }
/// }
/// ```
#[derive(Debug)]
pub struct ProviderCircuitBreaker {
    /// Identifier for the provider this breaker protects.
    provider_name: String,

    /// Configuration parameters for this circuit breaker.
    config: CircuitBreakerConfig,

    /// Current circuit state (0=Closed, 1=Open, 2=HalfOpen).
    /// Stored as atomic for lock-free reads.
    state: AtomicU8,

    /// Count of consecutive failures since last success in closed state.
    consecutive_failures: AtomicU64,

    /// Count of consecutive successes in half-open state.
    consecutive_successes: AtomicU64,

    /// Timestamp when the circuit was opened.
    /// Protected by mutex because it's rarely accessed (only in Open state).
    opened_at: Mutex<Option<Instant>>,
}

impl ProviderCircuitBreaker {
    /// Creates a new circuit breaker for the specified provider.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Identifier for the provider (e.g., "openai", "anthropic")
    /// * `config` - Circuit breaker configuration parameters
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
    ///
    /// let breaker = ProviderCircuitBreaker::new(
    ///     "openai".to_string(),
    ///     CircuitBreakerConfig::default(),
    /// );
    /// ```
    #[inline]
    pub fn new(provider_name: String, config: CircuitBreakerConfig) -> Self {
        Self {
            provider_name,
            config,
            state: AtomicU8::new(CircuitState::Closed.as_u8()),
            consecutive_failures: AtomicU64::new(0),
            consecutive_successes: AtomicU64::new(0),
            opened_at: Mutex::new(None),
        }
    }

    /// Creates a new circuit breaker with default configuration.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Identifier for the provider
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::ProviderCircuitBreaker;
    ///
    /// let breaker = ProviderCircuitBreaker::with_defaults("openai".to_string());
    /// assert!(breaker.allow_request().is_ok());
    /// ```
    #[inline]
    pub fn with_defaults(provider_name: String) -> Self {
        Self::new(provider_name, CircuitBreakerConfig::default())
    }

    /// Checks whether a request should be allowed to proceed.
    ///
    /// Returns `Ok(())` if the request is allowed, or `Err(CircuitOpenError)`
    /// if the circuit is open and requests are blocked.
    ///
    /// # State Transitions
    ///
    /// - **Closed**: Always allows, but first call in Open state with elapsed
    ///   duration transitions to HalfOpen.
    ///
    /// - **Open**: Blocks requests. If `open_duration` has elapsed since
    ///   opening, transitions to HalfOpen and allows this request.
    ///
    /// - **HalfOpen**: Always allows (limited probe requests).
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
    ///
    /// let breaker = ProviderCircuitBreaker::new(
    ///     "openai".to_string(),
    ///     CircuitBreakerConfig::default(),
    /// );
    ///
    /// match breaker.allow_request() {
    ///     Ok(()) => {
    ///         // Proceed with the request
    ///     }
    ///     Err(e) => {
    ///         eprintln!("Circuit open: {}", e);
    ///     }
    /// }
    /// ```
    pub fn allow_request(&self) -> Result<(), CircuitOpenError> {
        let state = self.load_state();

        match state {
            CircuitState::Closed => {
                // Closed: always allow requests
                Ok(())
            }

            CircuitState::Open => {
                // Open: check if duration has elapsed
                let opened_at = self.opened_at.lock();

                if let Some(timestamp) = *opened_at {
                    let elapsed = timestamp.elapsed();

                    if elapsed >= self.config.open_duration {
                        // Duration elapsed: transition to half-open
                        drop(opened_at);
                        self.state
                            .store(CircuitState::HalfOpen.as_u8(), Ordering::SeqCst);
                        self.consecutive_successes.store(0, Ordering::SeqCst);
                        return Ok(());
                    }

                    // Still in cooldown period
                    let remaining = self.config.open_duration.saturating_sub(elapsed);
                    return Err(CircuitOpenError::new(&self.provider_name, remaining));
                }

                // No timestamp recorded somehow; treat as half-open
                drop(opened_at);
                self.state
                    .store(CircuitState::HalfOpen.as_u8(), Ordering::SeqCst);
                Ok(())
            }

            CircuitState::HalfOpen => {
                // HalfOpen: allow probe requests
                Ok(())
            }
        }
    }

    /// Records a successful request.
    ///
    /// Updates internal counters based on current state:
    ///
    /// - **Closed**: Resets failure counter to zero.
    ///
    /// - **HalfOpen**: Increments success counter. If threshold reached,
    ///   closes the circuit (transitions to Closed).
    ///
    /// - **Open**: No effect (successes don't matter while waiting for cooldown).
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
    ///
    /// let breaker = ProviderCircuitBreaker::new(
    ///     "openai".to_string(),
    ///     CircuitBreakerConfig::default(),
    /// );
    ///
    /// // Simulate a successful request
    /// breaker.record_success();
    /// ```
    pub fn record_success(&self) {
        let state = self.load_state();

        match state {
            CircuitState::Closed => {
                // Reset failure counter on success in closed state
                self.consecutive_failures.store(0, Ordering::SeqCst);
            }

            CircuitState::HalfOpen => {
                // Count successes in half-open state
                let prev = self.consecutive_successes.fetch_add(1, Ordering::SeqCst);
                let new_count = prev + 1;

                if new_count >= self.config.half_open_successes as u64 {
                    // Enough successes: close the circuit
                    self.state
                        .store(CircuitState::Closed.as_u8(), Ordering::SeqCst);
                    self.consecutive_failures.store(0, Ordering::SeqCst);
                    self.consecutive_successes.store(0, Ordering::SeqCst);
                    // Clear the opened_at timestamp
                    *self.opened_at.lock() = None;
                }
            }

            CircuitState::Open => {
                // No action needed while circuit is open
            }
        }
    }

    /// Records a failed request.
    ///
    /// Updates internal counters and may trigger state transitions:
    ///
    /// - **Closed**: Increments failure counter. If threshold reached,
    ///   opens the circuit (records `Instant::now()` as opening time).
    ///
    /// - **HalfOpen**: Any failure reopens the circuit immediately.
    ///
    /// - **Open**: No additional effect (already tracking the failure).
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
    ///
    /// let breaker = ProviderCircuitBreaker::new(
    ///     "openai".to_string(),
    ///     CircuitBreakerConfig::default(),
    /// );
    ///
    /// // Simulate a failed request
    /// breaker.record_failure();
    /// ```
    pub fn record_failure(&self) {
        let state = self.load_state();

        match state {
            CircuitState::Closed => {
                // Increment failure counter
                let prev = self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
                let new_count = prev + 1;

                if new_count >= self.config.failure_threshold as u64 {
                    // Threshold reached: open the circuit
                    self.state
                        .store(CircuitState::Open.as_u8(), Ordering::SeqCst);
                    *self.opened_at.lock() = Some(Instant::now());
                }
            }

            CircuitState::HalfOpen => {
                // Any failure in half-open reopens the circuit
                self.state
                    .store(CircuitState::Open.as_u8(), Ordering::SeqCst);
                *self.opened_at.lock() = Some(Instant::now());
            }

            CircuitState::Open => {
                // Already open; no additional action needed
            }
        }
    }

    /// Manually resets the circuit breaker to the closed state.
    ///
    /// This is useful for:
    /// - Administrative intervention after fixing provider issues
    /// - Testing and development
    /// - Implementing custom reset logic
    ///
    /// After reset:
    /// - State becomes Closed
    /// - Consecutive failures reset to 0
    /// - Consecutive successes reset to 0
    /// - Opening timestamp is cleared
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
    ///
    /// let breaker = ProviderCircuitBreaker::new(
    ///     "openai".to_string(),
    ///     CircuitBreakerConfig::default(),
    /// );
    ///
    /// // Manually reset the circuit
    /// breaker.reset();
    /// ```
    pub fn reset(&self) {
        self.state
            .store(CircuitState::Closed.as_u8(), Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
        *self.opened_at.lock() = None;
    }

    /// Returns the current circuit state.
    ///
    /// This is a snapshot and may change immediately after being read.
    /// For decision-making, prefer `allow_request()` which handles state
    /// transitions atomically.
    #[inline]
    pub fn state(&self) -> CircuitState {
        self.load_state()
    }

    /// Returns the provider name this circuit breaker protects.
    #[inline]
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Returns a reference to the configuration.
    #[inline]
    pub fn config(&self) -> &CircuitBreakerConfig {
        &self.config
    }

    /// Returns the number of consecutive failures.
    ///
    /// This is useful for monitoring and debugging.
    #[inline]
    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    /// Returns the number of consecutive successes (in half-open state).
    ///
    /// This is useful for monitoring and debugging.
    #[inline]
    pub fn consecutive_successes(&self) -> u64 {
        self.consecutive_successes.load(Ordering::SeqCst)
    }

    /// Returns the time remaining before the circuit transitions to half-open.
    ///
    /// Returns `None` if the circuit is not in the open state.
    #[inline]
    pub fn remaining_open_time(&self) -> Option<Duration> {
        if self.load_state() == CircuitState::Open {
            let opened_at = self.opened_at.lock();
            opened_at.map(|t| {
                let elapsed = t.elapsed();
                self.config.open_duration.saturating_sub(elapsed)
            })
        } else {
            None
        }
    }

    /// Loads the current state from the atomic value.
    #[inline]
    fn load_state(&self) -> CircuitState {
        CircuitState::from_u8(self.state.load(Ordering::SeqCst))
    }
}

// ============================================================================
// Diagnostic Info
// ============================================================================

/// Provides diagnostic information about a circuit breaker's current state.
///
/// This struct is useful for monitoring dashboards, logging, and debugging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitBreakerDiagnostics {
    /// The provider this breaker protects.
    pub provider: String,
    /// Current state.
    pub state: CircuitState,
    /// Number of consecutive failures.
    pub consecutive_failures: u64,
    /// Number of consecutive successes (in half-open).
    pub consecutive_successes: u64,
    /// Whether the circuit is currently open.
    pub is_open: bool,
    /// Time remaining in open state, if applicable.
    pub remaining_open_time: Option<Duration>,
}

impl ProviderCircuitBreaker {
    /// Returns diagnostic information about this circuit breaker.
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_ai::circuit_breaker::{CircuitBreakerConfig, ProviderCircuitBreaker};
    ///
    /// let breaker = ProviderCircuitBreaker::new(
    ///     "openai".to_string(),
    ///     CircuitBreakerConfig::default(),
    /// );
    ///
    /// let diagnostics = breaker.diagnostics();
    /// println!("Provider: {}", diagnostics.provider);
    /// ```
    pub fn diagnostics(&self) -> CircuitBreakerDiagnostics {
        let state = self.load_state();
        CircuitBreakerDiagnostics {
            provider: self.provider_name.clone(),
            state,
            consecutive_failures: self.consecutive_failures(),
            consecutive_successes: self.consecutive_successes(),
            is_open: state == CircuitState::Open,
            remaining_open_time: self.remaining_open_time(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // CircuitState Tests
    // ========================================================================

    #[test]
    fn circuit_state_from_u8() {
        assert_eq!(CircuitState::from_u8(0), CircuitState::Closed);
        assert_eq!(CircuitState::from_u8(1), CircuitState::Open);
        assert_eq!(CircuitState::from_u8(2), CircuitState::HalfOpen);
        assert_eq!(CircuitState::from_u8(255), CircuitState::HalfOpen); // Unknown values map to HalfOpen
    }

    #[test]
    fn circuit_state_as_u8() {
        assert_eq!(CircuitState::Closed.as_u8(), 0);
        assert_eq!(CircuitState::Open.as_u8(), 1);
        assert_eq!(CircuitState::HalfOpen.as_u8(), 2);
    }

    // ========================================================================
    // CircuitBreakerConfig Tests
    // ========================================================================

    #[test]
    fn config_default() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.open_duration, Duration::from_secs(30));
        assert_eq!(config.half_open_successes, 1);
    }

    #[test]
    fn config_new_valid() {
        let config = CircuitBreakerConfig::new(3, Duration::from_secs(10), 2);
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.open_duration, Duration::from_secs(10));
        assert_eq!(config.half_open_successes, 2);
    }

    #[test]
    #[should_panic(expected = "failure_threshold cannot be zero")]
    fn config_new_zero_failure_threshold() {
        CircuitBreakerConfig::new(0, Duration::from_secs(10), 1);
    }

    #[test]
    #[should_panic(expected = "half_open_successes cannot be zero")]
    fn config_new_zero_half_open_successes() {
        CircuitBreakerConfig::new(3, Duration::from_secs(10), 0);
    }

    #[test]
    fn config_builder_methods() {
        let config = CircuitBreakerConfig::default()
            .with_failure_threshold(10)
            .with_open_duration(Duration::from_secs(60))
            .with_half_open_successes(2);

        assert_eq!(config.failure_threshold, 10);
        assert_eq!(config.open_duration, Duration::from_secs(60));
        assert_eq!(config.half_open_successes, 2);
    }

    // ========================================================================
    // ProviderCircuitBreaker Tests
    // ========================================================================

    #[test]
    fn breaker_allows_when_closed() {
        let breaker = ProviderCircuitBreaker::with_defaults("test".to_string());
        assert!(breaker.allow_request().is_ok());
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn breaker_success_in_closed_state() {
        let breaker = ProviderCircuitBreaker::with_defaults("test".to_string());
        breaker.record_success();
        assert_eq!(breaker.consecutive_failures(), 0);
    }

    #[test]
    fn breaker_opens_after_threshold() {
        let config = CircuitBreakerConfig::new(3, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        // Record failures up to threshold
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Closed);
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Closed);
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Should be blocked now
        assert!(breaker.allow_request().is_err());
    }

    #[test]
    fn breaker_success_resets_failure_count() {
        let config = CircuitBreakerConfig::new(3, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.consecutive_failures(), 2);

        breaker.record_success();
        assert_eq!(breaker.consecutive_failures(), 0);
    }

    #[test]
    fn breaker_reset() {
        let config = CircuitBreakerConfig::new(1, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        // Open the circuit
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Reset
        breaker.reset();
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert!(breaker.allow_request().is_ok());
    }

    #[test]
    fn breaker_half_open_on_duration_elapsed() {
        let config = CircuitBreakerConfig::new(1, Duration::from_millis(50), 1);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        // Open the circuit
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Wait for duration to elapse
        std::thread::sleep(Duration::from_millis(60));

        // Should transition to half-open on allow_request
        assert!(breaker.allow_request().is_ok());
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn breaker_half_open_success_closes_circuit() {
        let config = CircuitBreakerConfig::new(1, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        // Force to half-open
        breaker.reset();
        breaker
            .state
            .store(CircuitState::HalfOpen.as_u8(), Ordering::SeqCst);

        // Record success
        breaker.record_success();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn breaker_half_open_failure_reopens() {
        let config = CircuitBreakerConfig::new(1, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        // Force to half-open
        breaker.reset();
        breaker
            .state
            .store(CircuitState::HalfOpen.as_u8(), Ordering::SeqCst);

        // Record failure
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);
    }

    #[test]
    fn breaker_multiple_half_open_successes() {
        let config = CircuitBreakerConfig::new(1, Duration::from_secs(30), 3);
        let breaker = ProviderCircuitBreaker::new("test".to_string(), config);

        // Force to half-open
        breaker.reset();
        breaker
            .state
            .store(CircuitState::HalfOpen.as_u8(), Ordering::SeqCst);

        // Partial successes should not close
        breaker.record_success();
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        breaker.record_success();
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // Third success closes
        breaker.record_success();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn breaker_diagnostics() {
        let config = CircuitBreakerConfig::new(2, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("openai".to_string(), config);

        breaker.record_failure();
        let diag = breaker.diagnostics();

        assert_eq!(diag.provider, "openai");
        assert_eq!(diag.state, CircuitState::Closed);
        assert_eq!(diag.consecutive_failures, 1);
        assert!(!diag.is_open);
    }

    #[test]
    fn breaker_diagnostics_when_open() {
        let config = CircuitBreakerConfig::new(1, Duration::from_secs(30), 1);
        let breaker = ProviderCircuitBreaker::new("anthropic".to_string(), config);

        breaker.record_failure();
        let diag = breaker.diagnostics();

        assert!(diag.is_open);
        assert!(diag.remaining_open_time.is_some());
    }

    // ========================================================================
    // CircuitOpenError Tests
    // ========================================================================

    #[test]
    fn circuit_open_error_display() {
        let error = CircuitOpenError::new("openai", Duration::from_secs(10));
        let msg = error.to_string();
        assert!(msg.contains("openai"));
        assert!(msg.contains("10"));
    }

    #[test]
    fn circuit_open_error_clone() {
        let error = CircuitOpenError::new("test", Duration::from_secs(5));
        let cloned = error.clone();
        assert_eq!(error, cloned);
    }
}
