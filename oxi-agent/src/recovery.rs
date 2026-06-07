//! Fault-recovery primitives — re-exported from oxi-ai.
//!
//! This module re-exports from `oxi_ai` for backward compatibility.
//! Types originally defined here have been promoted to `oxi-ai` for reuse
//! across both oxi-ai and oxi-agent.

pub use oxi_ai::circuit_breaker::{CircuitBreakerConfig, CircuitOpenError, ProviderCircuitBreaker};
pub use oxi_ai::fallback_chain::FallbackChain;
pub use oxi_ai::partial_response::PartialResponse;

// Keep the local CircuitBreaker (no provider name) — it's slightly different
// from ProviderCircuitBreaker in that it doesn't track per-provider state.
// Re-export the config types but keep a local struct for backward compat.
use oxi_ai::circuit_breaker::CircuitBreakerConfig as CircuitBreakerConfigLocal;

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

/// Error returned when the circuit is open and requests are not allowed.
/// Local version without provider name for backward compatibility.
#[derive(Debug, thiserror::Error)]
#[error("Circuit is open — retry after {remaining:?}")]
pub struct CircuitOpenErrorLocal {
    /// Time remaining before the circuit transitions to half-open.
    pub remaining: std::time::Duration,
}

/// A lock-free circuit breaker. Thread-safe via atomic operations.
/// Kept here for backward compatibility with existing oxi-agent code.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfigLocal,
    state: AtomicU8,
    consecutive_failures: AtomicU64,
    consecutive_successes: AtomicU64,
    opened_at: Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfigLocal) -> Self {
        Self {
            config,
            state: AtomicU8::new(0), // Closed
            consecutive_failures: AtomicU64::new(0),
            consecutive_successes: AtomicU64::new(0),
            opened_at: Mutex::new(None),
        }
    }

    /// Check if a request is allowed to proceed.
    pub fn allow_request(&self) -> Result<(), CircuitOpenErrorLocal> {
        let state = self.state.load(Ordering::SeqCst);
        match state {
            0 => Ok(()), // Closed
            1 => {
                // Open
                let opened_at = self.opened_at.lock();
                if let Some(t) = *opened_at {
                    if t.elapsed() >= self.config.open_duration {
                        drop(opened_at);
                        self.state.store(2, Ordering::SeqCst); // HalfOpen
                        self.consecutive_successes.store(0, Ordering::SeqCst);
                        return Ok(());
                    }
                    let remaining = self.config.open_duration.saturating_sub(t.elapsed());
                    return Err(CircuitOpenErrorLocal { remaining });
                }
                drop(opened_at);
                self.state.store(2, Ordering::SeqCst);
                Ok(())
            }
            _ => Ok(()), // HalfOpen
        }
    }

    /// Record a successful request.
    pub fn record_success(&self) {
        let state = self.state.load(Ordering::SeqCst);
        if state == 0 {
            self.consecutive_failures.store(0, Ordering::SeqCst);
        } else if state == 2 {
            let prev = self.consecutive_successes.fetch_add(1, Ordering::SeqCst);
            if prev + 1 >= self.config.half_open_successes as u64 {
                self.state.store(0, Ordering::SeqCst);
                self.consecutive_failures.store(0, Ordering::SeqCst);
            }
        }
    }

    /// Record a failed request.
    pub fn record_failure(&self) {
        let state = self.state.load(Ordering::SeqCst);
        if state == 0 {
            let prev = self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
            if prev + 1 >= self.config.failure_threshold as u64 {
                self.state.store(1, Ordering::SeqCst);
                *self.opened_at.lock() = Some(Instant::now());
            }
        } else if state == 2 {
            self.state.store(1, Ordering::SeqCst);
            *self.opened_at.lock() = Some(Instant::now());
        }
    }

    /// Manually reset the circuit breaker to the closed state.
    pub fn reset(&self) {
        self.state.store(0, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
        *self.opened_at.lock() = None;
    }
}
