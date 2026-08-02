//! Circuit-breaker behavior trait + reference implementation.
//!
//! SDK owns the `CircuitBreaker` trait + `DefaultCircuitBreaker` (this
//! module). Consumers implement `CircuitBreaker` for domain-specific traffic
//! classes (A2A, HTTP, LLM calls) where the SDK's reference thresholds do not
//! match the consumer's profile.
//!
//! See `docs/oxi-sdk-ownership.md` §3 for the ownership contract and the
//! reference pattern.
//!
//! # State machine
//!
//! ```text
//!   Closed --failures >= threshold--> Open
//!      ^                                 |
//!      |                             reset_timeout
//!      |                                 v
//!      +---success---- HalfOpen <-- first check()
//! ```
//!
//! - **Closed**: every `CircuitBreaker::check` returns `Ok`.
//!   `CircuitBreaker::record_failure` increments the failure count; when it
//!   reaches the threshold the breaker trips to `Open`.
//! - **Open**: `CircuitBreaker::check` returns `Err(BreakerError::Open)` until
//!   `reset_timeout` has elapsed since the last failure.
//! - **HalfOpen**: the first `CircuitBreaker::check` after the reset timeout
//!   returns `Ok` (a trial call). A success closes the breaker; a failure
//!   re-opens it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Behavior contract for circuit-breaking resilience.
///
/// SDK owns this trait + [`DefaultCircuitBreaker`]; consumers implement it for
/// domain-specific traffic classes (A2A, HTTP, etc.). See
/// `docs/oxi-sdk-ownership.md`.
///
/// This trait is `#[unstable]` initially — the surface may evolve as we
/// integrate with the agent loop and learn which signals consumers actually
/// need. It graduates to `#[stable]` after it proves useful in production.
pub trait CircuitBreaker: Send + Sync {
    /// Returns `Err(BreakerError::Open)` if the circuit is open (calls should
    /// fail-fast). The check is cheap (atomic load) so callers may invoke it
    /// before every retry.
    fn check(&self) -> Result<(), BreakerError>;

    /// Record a successful call. Resets the failure count and, if the breaker
    /// was in `HalfOpen`, returns it to `Closed`.
    fn record_success(&self);

    /// Record a failed call. Increments the failure count; if the count
    /// reaches the configured threshold the breaker trips to `Open`.
    fn record_failure(&self);
}

/// Error returned when the circuit is open.
///
/// `#[non_exhaustive]` — consumers MUST add a catch-all arm. New variants
/// will be added in future minor releases (e.g. `HalfOpen` or `Forced`) to
/// surface state transitions that consumers may want to react to.
///
/// **A circuit-open error is NOT retryable.** A breaker's whole purpose is
/// to STOP hammering a failing upstream. Callers that receive [`BreakerError`]
/// MUST short-circuit the retry loop and return the error to the user. The
/// [`BreakerError::is_retryable`] method exists to make this contract
/// machine-checkable: integrations that map breaker errors into a provider
/// retry loop should branch on it.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum BreakerError {
    /// The circuit is open: too many consecutive failures, or the reset
    /// timeout has not yet elapsed since the last failure.
    #[error("circuit open: too many consecutive failures")]
    Open,
}

impl BreakerError {
    /// Returns `false`. A circuit-open error is a deliberate stop signal;
    /// retrying would burn the retry budget against a known-open circuit and
    /// is the exact behavior the breaker exists to prevent.
    pub fn is_retryable(&self) -> bool {
        false
    }
}

// Internal state representation. Stored as the raw byte in the AtomicU8
// so the state itself can be `Arc<DefaultCircuitBreaker>` without a Mutex.
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Observable breaker state. Returned by [`DefaultCircuitBreaker::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Breaker is closed — every call passes through.
    Closed,
    /// Breaker is open — calls fail-fast with [`BreakerError::Open`].
    Open,
    /// Breaker is half-open — the reset timeout has elapsed, and the next
    /// call is a trial. Success closes the breaker, failure re-opens it.
    HalfOpen,
}

/// SDK reference implementation: threshold-based with half-open state machine.
///
/// The thresholds (`failure_threshold`, `reset_timeout`) are SDK-illustrative,
/// not consumer policy. A consumer whose traffic profile differs (e.g. A2A
/// with `12 failures/min` and a 30s recovery window) implements
/// [`CircuitBreaker`] for its own struct rather than reusing this.
pub struct DefaultCircuitBreaker {
    failure_threshold: u32,
    reset_timeout: Duration,
    created_at: Instant,
    state: AtomicU8,
    failure_count: AtomicU32,
    last_failure_ms: AtomicU32,
}

impl DefaultCircuitBreaker {
    /// Construct a new breaker. `failure_threshold` is the number of
    /// consecutive failures that trip the breaker; `reset_timeout` is how
    /// long the breaker stays open before allowing a trial call (half-open).
    pub fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        Self {
            failure_threshold,
            reset_timeout,
            created_at: Instant::now(),
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            last_failure_ms: AtomicU32::new(0),
        }
    }

    /// Current state (testing/observability).
    pub fn state(&self) -> BreakerState {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => BreakerState::Closed,
            STATE_OPEN => BreakerState::Open,
            STATE_HALF_OPEN => BreakerState::HalfOpen,
            _ => BreakerState::Closed,
        }
    }

    /// Failure count since the last successful call (observability).
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Acquire)
    }
}

impl CircuitBreaker for DefaultCircuitBreaker {
    fn check(&self) -> Result<(), BreakerError> {
        let state = self.state.load(Ordering::Acquire);
        match state {
            STATE_CLOSED | STATE_HALF_OPEN => Ok(()),
            STATE_OPEN => {
                let last_ms = self.last_failure_ms.load(Ordering::Acquire);
                let elapsed_ms = self.created_at.elapsed().as_millis() as u64;
                if elapsed_ms.saturating_sub(u64::from(last_ms))
                    >= self.reset_timeout.as_millis() as u64
                {
                    // Reset timeout elapsed: allow one trial call.
                    self.state.store(STATE_HALF_OPEN, Ordering::Release);
                    Ok(())
                } else {
                    Err(BreakerError::Open)
                }
            }
            _ => Ok(()),
        }
    }

    fn record_success(&self) {
        self.failure_count.store(0, Ordering::Release);
        self.state.store(STATE_CLOSED, Ordering::Release);
    }

    fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        self.last_failure_ms.store(
            self.created_at.elapsed().as_millis() as u32,
            Ordering::Release,
        );
        if count >= self.failure_threshold {
            self.state.store(STATE_OPEN, Ordering::Release);
        }
    }
}

/// Shared ownership helper for `AgentLoopConfig.circuit_breaker`.
pub type SharedBreaker = Arc<dyn CircuitBreaker>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_starts_closed_allows_calls() {
        let b = DefaultCircuitBreaker::new(3, Duration::from_secs(30));
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.check().is_ok());
        assert_eq!(b.failure_count(), 0);
    }

    #[test]
    fn breaker_opens_after_threshold_failures() {
        let b = DefaultCircuitBreaker::new(3, Duration::from_secs(30));
        b.record_failure();
        b.record_failure();
        assert!(b.check().is_ok(), "2 < 3, still closed");
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Open);
        assert!(b.check().is_err(), "3 >= 3, now open");
    }

    #[test]
    fn breaker_half_opens_after_timeout() {
        let b = DefaultCircuitBreaker::new(1, Duration::from_millis(20));
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Open);
        assert!(b.check().is_err(), "still open immediately after trip");
        std::thread::sleep(Duration::from_millis(30));
        assert!(b.check().is_ok(), "half-open allows trial after timeout");
        assert_eq!(b.state(), BreakerState::HalfOpen);
        b.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn success_resets_failure_count() {
        let b = DefaultCircuitBreaker::new(3, Duration::from_secs(30));
        b.record_failure();
        b.record_failure();
        b.record_success();
        b.record_failure();
        b.record_failure();
        assert!(b.check().is_ok(), "only 2 since reset, still closed");
        assert_eq!(b.failure_count(), 2);
    }

    #[test]
    fn trait_object_dispatch_works() {
        let b: SharedBreaker = Arc::new(DefaultCircuitBreaker::new(2, Duration::from_secs(1)));
        b.record_failure();
        b.record_failure();
        assert!(b.check().is_err());
    }

    #[test]
    fn open_error_is_not_retryable() {
        // The whole point: callers that receive this error MUST NOT retry.
        let err = BreakerError::Open;
        assert!(!err.is_retryable());
    }
}
