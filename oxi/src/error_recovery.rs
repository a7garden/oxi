//! Error recovery with retry logic
//!
//! Provides retry functionality with exponential backoff, rate limiting support,
//! and UI integration for countdown display.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Base delay in milliseconds
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds
    pub max_delay_ms: u64,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Jitter factor (0.0 to 1.0) to add randomness to delays
    pub jitter: f64,
    /// Total timeout for all retries combined (None = no timeout)
    pub total_timeout_ms: Option<u64>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: 0.1,
            total_timeout_ms: None,
        }
    }
}

impl RetryConfig {
    /// Create a new retry config with sensible defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum retries
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Set base delay in milliseconds
    pub fn with_base_delay(mut self, ms: u64) -> Self {
        self.base_delay_ms = ms;
        self
    }

    /// Set maximum delay in milliseconds
    pub fn with_max_delay(mut self, ms: u64) -> Self {
        self.max_delay_ms = ms;
        self
    }

    /// Set backoff multiplier
    pub fn with_backoff_multiplier(mut self, mult: f64) -> Self {
        self.backoff_multiplier = mult;
        self
    }

    /// Set jitter factor (0.0 to 1.0)
    pub fn with_jitter(mut self, jitter: f64) -> Self {
        self.jitter = jitter.clamp(0.0, 1.0);
        self
    }

    /// Set total timeout in milliseconds
    pub fn with_total_timeout(mut self, ms: u64) -> Self {
        self.total_timeout_ms = Some(ms);
        self
    }

    /// Check if should retry based on error and attempt count
    pub fn should_retry(&self, error: &RetryableError, attempt: u32) -> bool {
        if attempt >= self.max_retries {
            return false;
        }

        // Check total timeout if configured
        if let Some(timeout_ms) = self.total_timeout_ms {
            if attempt > 0 {
                // Rough estimate: if we've taken more than timeout_ms, don't retry
                let estimated_time = self.estimate_total_time(attempt);
                if estimated_time > Duration::from_millis(timeout_ms) {
                    return false;
                }
            }
        }

        // Some errors should not be retried
        matches!(
            error,
            RetryableError::NetworkError | RetryableError::Timeout
        ) || matches!(error, RetryableError::RateLimitError { .. })
            || matches!(error, RetryableError::ServerError { .. })
    }

    /// Calculate next delay based on attempt number
    pub fn next_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::from_millis(self.base_delay_ms);
        }

        // Exponential backoff: base_delay * (multiplier ^ attempt)
        let exponential = self.base_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32);

        // Cap at max delay
        let capped = exponential.min(self.max_delay_ms as f64);

        // Add jitter
        let jitter_range = capped * self.jitter;
        let jitter = (rand_simple(attempt) * 2.0 - 1.0) * jitter_range;
        let final_delay = (capped + jitter).max(0.0);

        Duration::from_millis(final_delay as u64)
    }

    /// Estimate total time spent on retries up to given attempt
    fn estimate_total_time(&self, attempts: u32) -> Duration {
        let mut total_ms = 0u64;
        for i in 0..attempts {
            total_ms += self.next_delay(i).as_millis() as u64;
        }
        Duration::from_millis(total_ms)
    }
}

/// Types of errors that can be retried
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryableError {
    /// Network connectivity issues
    NetworkError,
    /// Rate limit exceeded
    RateLimitError { retry_after: u64 },
    /// Server-side errors (5xx)
    ServerError { code: u16 },
    /// Request timeout
    Timeout,
    /// Service unavailable
    ServiceUnavailable { retry_after: u64 },
    /// Temporary failure
    TemporaryFailure,
}

impl RetryableError {
    /// Create from HTTP status code
    pub fn from_status_code(code: u16) -> Option<Self> {
        match code {
            408 | 429 => Some(Self::RateLimitError { retry_after: 0 }),
            500 | 502 | 503 | 504 => Some(Self::ServerError { code }),
            503 => Some(Self::ServiceUnavailable { retry_after: 0 }),
            _ if code >= 500 => Some(Self::ServerError { code }),
            _ => None,
        }
    }

    /// Create from network error message
    pub fn from_network_error(msg: &str) -> Self {
        let msg_lower = msg.to_lowercase();
        if msg_lower.contains("timeout") {
            RetryableError::Timeout
        } else if msg_lower.contains("connection") || msg_lower.contains("network") {
            RetryableError::NetworkError
        } else {
            RetryableError::NetworkError
        }
    }

    /// Get suggested retry delay in seconds, if any
    pub fn suggested_delay(&self) -> Option<u64> {
        match self {
            RetryableError::RateLimitError { retry_after } => {
                if *retry_after > 0 {
                    Some(*retry_after)
                } else {
                    None
                }
            }
            RetryableError::ServiceUnavailable { retry_after } => {
                if *retry_after > 0 {
                    Some(*retry_after)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check if this error indicates we should wait before retry
    pub fn has_required_wait(&self) -> bool {
        matches!(
            self,
            RetryableError::RateLimitError { retry_after } | RetryableError::ServiceUnavailable { retry_after }
        ) && *retry_after > 0
    }
}

impl std::fmt::Display for RetryableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryableError::NetworkError => write!(f, "Network error"),
            RetryableError::RateLimitError { retry_after } => {
                if *retry_after > 0 {
                    write!(f, "Rate limit exceeded (retry after {}s)", retry_after)
                } else {
                    write!(f, "Rate limit exceeded")
                }
            }
            RetryableError::ServerError { code } => write!(f, "Server error ({})", code),
            RetryableError::Timeout => write!(f, "Request timeout"),
            RetryableError::ServiceUnavailable { retry_after } => {
                if *retry_after > 0 {
                    write!(f, "Service unavailable (retry after {}s)", retry_after)
                } else {
                    write!(f, "Service unavailable")
                }
            }
            RetryableError::TemporaryFailure => write!(f, "Temporary failure"),
        }
    }
}

/// Current state of a retry operation
#[derive(Debug, Clone)]
pub struct RetryState {
    /// Current attempt number (0-indexed)
    pub attempt: u32,
    /// When the next retry will happen
    pub next_retry: DateTime<Utc>,
    /// The error that caused the retry
    pub error: RetryableError,
    /// Total elapsed time so far
    pub elapsed_ms: u64,
    /// Whether the retry was aborted
    pub aborted: bool,
}

impl RetryState {
    /// Create a new retry state
    pub fn new(attempt: u32, error: RetryableError) -> Self {
        Self {
            attempt,
            next_retry: Utc::now(),
            error,
            elapsed_ms: 0,
            aborted: false,
        }
    }

    /// Update next retry time
    pub fn set_next_retry(&mut self, delay: Duration) {
        self.next_retry = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
    }

    /// Mark as aborted
    pub fn abort(&mut self) {
        self.aborted = true;
    }

    /// Get seconds until next retry (floored)
    pub fn seconds_until_retry(&self) -> i64 {
        let now = Utc::now();
        let diff = self.next_retry.signed_duration_since(now);
        diff.num_seconds().max(0)
    }

    /// Check if retry is due
    pub fn is_ready(&self) -> bool {
        self.seconds_until_retry() <= 0 && !self.aborted
    }
}

/// Result of a retry operation
pub enum RetryResult<T> {
    /// Success with the result
    Success(T),
    /// All retries exhausted
    Exhausted { attempts: u32, last_error: RetryableError },
    /// User aborted the retry
    Aborted { attempts: u32 },
    /// Timeout exceeded
    TimedOut { attempts: u32, elapsed_ms: u64 },
}

/// Execute a function with retry logic
pub async fn with_retry<R, F, Fut>(config: &RetryConfig, f: F) -> Result<RetryResult<R>, RetryableError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<R, RetryableError>>,
{
    let start = Instant::now();
    let mut attempt = 0u32;

    loop {
        debug!("Attempt {} of {}", attempt + 1, config.max_retries + 1);

        match f().await {
            Ok(result) => {
                return Ok(RetryResult::Success(result));
            }
            Err(error) => {
                if !config.should_retry(&error, attempt) {
                    info!("Retry not possible for error: {}", error);
                    return Ok(RetryResult::Exhausted {
                        attempts: attempt + 1,
                        last_error: error,
                    });
                }

                // Check if we have a required wait from the error
                let delay = if error.has_required_wait() {
                    let wait = error.suggested_delay().unwrap_or(0);
                    Duration::from_secs(wait)
                } else {
                    config.next_delay(attempt)
                };

                info!("Retrying in {:?} due to: {}", delay, error);

                // Wait before next attempt
                tokio::time::sleep(delay).await;

                // Check total timeout
                if let Some(timeout_ms) = config.total_timeout_ms {
                    let elapsed = start.elapsed().as_millis() as u64;
                    if elapsed >= timeout_ms {
                        return Ok(RetryResult::TimedOut {
                            attempts: attempt + 1,
                            elapsed_ms: elapsed,
                        });
                    }
                }

                attempt += 1;
            }
        }
    }
}

/// Execute a function with retry logic, returning the result directly (unwrapping Success)
pub async fn retry<R, F, Fut>(config: &RetryConfig, f: F) -> Result<R, RetryableError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<R, RetryableError>>,
{
    match with_retry(config, f).await? {
        RetryResult::Success(r) => Ok(r),
        RetryResult::Exhausted { attempts, last_error } => {
            warn!("Retry exhausted after {} attempts: {}", attempts, last_error);
            Err(last_error)
        }
        RetryResult::Aborted { attempts } => {
            warn!("Retry aborted after {} attempts", attempts);
            Err(RetryableError::TemporaryFailure) // Use generic error for abort
        }
        RetryResult::TimedOut { attempts, elapsed_ms } => {
            warn!("Retry timed out after {} attempts ({}ms)", attempts, elapsed_ms);
            Err(RetryableError::Timeout)
        }
    }
}

/// Countdown timer for UI display
pub struct CountdownTimer {
    remaining_ms: RwLock<u64>,
    start_time: Instant,
    total_ms: u64,
    on_tick: Option<Box<dyn Fn(u64) + Send + Sync>>,
    on_complete: Option<Box<dyn Fn() + Send + Sync>>,
}

use parking_lot::RwLock;

impl CountdownTimer {
    /// Create a new countdown timer
    pub fn new(
        duration_ms: u64,
        on_tick: Option<Box<dyn Fn(u64) + Send + Sync>>,
        on_complete: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let now = Instant::now();
        Self {
            remaining_ms: RwLock::new(duration_ms),
            start_time: now,
            total_ms: duration_ms,
            on_tick,
            on_complete,
        }
    }

    /// Update the countdown and return remaining seconds (0 if complete)
    pub fn update(&self) -> u64 {
        let elapsed = self.start_time.elapsed().as_millis() as u64;
        let remaining = self.total_ms.saturating_sub(elapsed);
        *self.remaining_ms.write() = remaining;

        let seconds = (remaining / 1000) as u64;

        // Trigger tick callback every second
        if remaining % 1000 < 50 {
            if let Some(ref cb) = self.on_tick {
                cb(seconds);
            }
        }

        // Trigger complete callback when done
        if remaining == 0 {
            if let Some(ref cb) = self.on_complete {
                cb();
            }
        }

        seconds
    }

    /// Get remaining time in milliseconds
    pub fn remaining_ms(&self) -> u64 {
        *self.remaining_ms.read()
    }

    /// Check if countdown is complete
    pub fn is_complete(&self) -> bool {
        *self.remaining_ms.read() == 0
    }

    /// Get progress as a fraction (0.0 to 1.0)
    pub fn progress(&self) -> f64 {
        let remaining = *self.remaining_ms.read();
        if self.total_ms == 0 {
            1.0
        } else {
            1.0 - (remaining as f64 / self.total_ms as f64)
        }
    }

    /// Get formatted time string (e.g., "3s" or "0s")
    pub fn formatted(&self) -> String {
        let remaining = *self.remaining_ms.read();
        let seconds = remaining / 1000;
        if seconds > 0 {
            format!("{}s", seconds)
        } else {
            "0s".to_string()
        }
    }

    /// Cancel the countdown
    pub fn cancel(&self) {
        *self.remaining_ms.write() = 0;
    }
}

/// Format a retry message for UI display
pub fn format_retry_message(
    attempt: u32,
    max_attempts: u32,
    delay_seconds: u64,
    error: &RetryableError,
    can_cancel: bool,
) -> String {
    let error_str = match error {
        RetryableError::NetworkError => "Network error",
        RetryableError::RateLimitError { .. } => "Rate limit exceeded",
        RetryableError::ServerError { code } => format!("Server error ({})", code),
        RetryableError::Timeout => "Request timeout",
        RetryableError::ServiceUnavailable { .. } => "Service unavailable",
        RetryableError::TemporaryFailure => "Temporary failure",
    };

    let cancel_hint = if can_cancel {
        " (Esc to cancel)"
    } else {
        ""
    };

    format!(
        "Retrying ({}/{}) in {}s: {}{}",
        attempt, max_attempts, delay_seconds, error_str, cancel_hint
    )
}

/// Get a user-friendly error message
pub fn format_error_message(error: &RetryableError) -> String {
    match error {
        RetryableError::NetworkError => {
            "Connection failed. Check your network and try again.".to_string()
        }
        RetryableError::RateLimitError { retry_after } => {
            if *retry_after > 0 {
                format!("Rate limit hit. Wait {}s or reduce request frequency.", retry_after)
            } else {
                "Rate limit exceeded. Try again in a few moments.".to_string()
            }
        }
        RetryableError::ServerError { code } => {
            format!("Server error ({}). Try again later.", code)
        }
        RetryableError::Timeout => {
            "Request timed out. Try again or use a shorter request.".to_string()
        }
        RetryableError::ServiceUnavailable { retry_after } => {
            if *retry_after > 0 {
                format!("Service temporarily unavailable. Retry in {}s.", retry_after)
            } else {
                "Service temporarily unavailable. Try again later.".to_string()
            }
        }
        RetryableError::TemporaryFailure => {
            "Temporary failure. Try again.".to_string()
        }
    }
}

/// Simple pseudo-random function for jitter (not cryptographically secure)
fn rand_simple(seed: u32) -> f64 {
    // Simple linear congruential generator
    let mut x = seed.wrapping_mul(1103515245).wrapping_add(12345);
    x = x.wrapping_mul(1103515245).wrapping_add(12345);
    ((x as u64) % 1000000) as f64 / 1000000.0
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
        assert!((config.backoff_multiplier - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_retry_config_builder() {
        let config = RetryConfig::new()
            .with_max_retries(5)
            .with_base_delay(500)
            .with_max_delay(60000)
            .with_backoff_multiplier(1.5);

        assert_eq!(config.max_retries, 5);
        assert_eq!(config.base_delay_ms, 500);
        assert_eq!(config.max_delay_ms, 60000);
        assert!((config.backoff_multiplier - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_should_retry_within_limit() {
        let config = RetryConfig::default();

        assert!(config.should_retry(&RetryableError::NetworkError, 0));
        assert!(config.should_retry(&RetryableError::NetworkError, 1));
        assert!(config.should_retry(&RetryableError::NetworkError, 2));
    }

    #[test]
    fn test_should_retry_at_limit() {
        let config = RetryConfig::default();

        // At max_retries (3), should not retry
        assert!(!config.should_retry(&RetryableError::NetworkError, 3));
    }

    #[test]
    fn test_should_retry_rate_limit() {
        let config = RetryConfig::default();

        assert!(config.should_retry(
            &RetryableError::RateLimitError { retry_after: 0 },
            0
        ));
        assert!(config.should_retry(
            &RetryableError::RateLimitError { retry_after: 30 },
            0
        ));
    }

    #[test]
    fn test_should_retry_server_error() {
        let config = RetryConfig::default();

        assert!(config.should_retry(&RetryableError::ServerError { code: 500 }, 0));
        assert!(config.should_retry(&RetryableError::ServerError { code: 502 }, 0));
        assert!(config.should_retry(&RetryableError::ServerError { code: 503 }, 0));
    }

    #[test]
    fn test_should_retry_timeout() {
        let config = RetryConfig::default();
        assert!(config.should_retry(&RetryableError::Timeout, 0));
    }

    #[test]
    fn test_next_delay_exponential() {
        let config = RetryConfig::default();

        // First delay should be base_delay
        let delay0 = config.next_delay(0);
        assert!((delay0.as_millis() as u64 - config.base_delay_ms) <= 100); // Allow jitter tolerance

        // Second delay should be larger
        let delay1 = config.next_delay(1);
        assert!(delay1 > delay0);

        // Third delay should be larger still
        let delay2 = config.next_delay(2);
        assert!(delay2 > delay1);
    }

    #[test]
    fn test_next_delay_capped_at_max() {
        let config = RetryConfig::new()
            .with_base_delay(1000)
            .with_max_delay(5000)
            .with_backoff_multiplier(10.0);

        // With high multiplier, should cap at max_delay
        let delay = config.next_delay(10);
        assert!(delay.as_millis() as u64 <= config.max_delay_ms);
    }

    #[test]
    fn test_retryable_error_from_status_code() {
        assert_eq!(
            RetryableError::from_status_code(429),
            Some(RetryableError::RateLimitError { retry_after: 0 })
        );
        assert_eq!(
            RetryableError::from_status_code(500),
            Some(RetryableError::ServerError { code: 500 })
        );
        assert_eq!(
            RetryableError::from_status_code(502),
            Some(RetryableError::ServerError { code: 502 })
        );
        assert_eq!(
            RetryableError::from_status_code(503),
            Some(RetryableError::ServiceUnavailable { retry_after: 0 })
        );
        // Non-retryable status codes
        assert_eq!(RetryableError::from_status_code(400), None);
        assert_eq!(RetryableError::from_status_code(404), None);
    }

    #[test]
    fn test_retryable_error_suggested_delay() {
        let rate_limit = RetryableError::RateLimitError { retry_after: 30 };
        assert_eq!(rate_limit.suggested_delay(), Some(30));

        let service = RetryableError::ServiceUnavailable { retry_after: 60 };
        assert_eq!(service.suggested_delay(), Some(60));

        let network = RetryableError::NetworkError;
        assert_eq!(network.suggested_delay(), None);

        let timeout = RetryableError::Timeout;
        assert_eq!(timeout.suggested_delay(), None);
    }

    #[test]
    fn test_retryable_error_display() {
        assert_eq!(RetryableError::NetworkError.to_string(), "Network error");
        assert_eq!(
            RetryableError::RateLimitError { retry_after: 30 }.to_string(),
            "Rate limit exceeded (retry after 30s)"
        );
        assert_eq!(
            RetryableError::ServerError { code: 500 }.to_string(),
            "Server error (500)"
        );
        assert_eq!(RetryableError::Timeout.to_string(), "Request timeout");
    }

    #[test]
    fn test_retry_state_creation() {
        let state = RetryState::new(0, RetryableError::NetworkError);
        assert_eq!(state.attempt, 0);
        assert!(!state.aborted);
    }

    #[test]
    fn test_retry_state_update_next_retry() {
        let mut state = RetryState::new(0, RetryableError::NetworkError);
        state.set_next_retry(Duration::from_secs(5));
        assert!(state.next_retry > Utc::now());
    }

    #[test]
    fn test_retry_state_abort() {
        let mut state = RetryState::new(0, RetryableError::NetworkError);
        state.abort();
        assert!(state.aborted);
    }

    #[test]
    fn test_countdown_timer_basic() {
        let timer = CountdownTimer::new(5000, None, None);
        
        assert!(!timer.is_complete());
        assert_eq!(timer.remaining_ms(), 5000);
    }

    #[test]
    fn test_countdown_timer_progress() {
        let timer = CountdownTimer::new(10000, None, None);
        
        // At start, progress should be 0
        assert!((timer.progress() - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_countdown_timer_formatted() {
        let timer = CountdownTimer::new(3500, None, None);
        
        // Should round to seconds
        assert_eq!(timer.formatted(), "3s");
    }

    #[test]
    fn test_countdown_timer_cancel() {
        let timer = CountdownTimer::new(5000, None, None);
        timer.cancel();
        
        assert!(timer.is_complete());
        assert_eq!(timer.remaining_ms(), 0);
    }

    #[test]
    fn test_format_retry_message() {
        let error = RetryableError::NetworkError;
        let msg = format_retry_message(1, 3, 5, &error, true);
        assert!(msg.contains("1/3"));
        assert!(msg.contains("5s"));
        assert!(msg.contains("Network error"));
        assert!(msg.contains("Esc to cancel"));
    }

    #[test]
    fn test_format_retry_message_no_cancel() {
        let error = RetryableError::Timeout;
        let msg = format_retry_message(2, 3, 10, &error, false);
        assert!(msg.contains("2/3"));
        assert!(msg.contains("10s"));
        assert!(msg.contains("Request timeout"));
        assert!(!msg.contains("Esc"));
    }

    #[test]
    fn test_format_error_message() {
        assert_eq!(
            format_error_message(&RetryableError::NetworkError),
            "Connection failed. Check your network and try again."
        );
        assert_eq!(
            format_error_message(&RetryableError::RateLimitError { retry_after: 30 }),
            "Rate limit hit. Wait 30s or reduce request frequency."
        );
        assert_eq!(
            format_error_message(&RetryableError::ServerError { code: 500 }),
            "Server error (500). Try again later."
        );
    }

    #[test]
    fn test_retry_result_success() {
        let result = RetryResult::Success(42);
        match result {
            RetryResult::Success(val) => assert_eq!(val, 42),
            _ => panic!("Expected Success"),
        }
    }

    #[test]
    fn test_retry_result_exhausted() {
        let result = RetryResult::Exhausted {
            attempts: 3,
            last_error: RetryableError::NetworkError,
        };
        match result {
            RetryResult::Exhausted { attempts, last_error } => {
                assert_eq!(attempts, 3);
                assert!(matches!(last_error, RetryableError::NetworkError));
            }
            _ => panic!("Expected Exhausted"),
        }
    }

    #[test]
    fn test_retry_result_aborted() {
        let result = RetryResult::Aborted { attempts: 2 };
        match result {
            RetryResult::Aborted { attempts } => assert_eq!(attempts, 2),
            _ => panic!("Expected Aborted"),
        }
    }

    #[test]
    fn test_retry_result_timed_out() {
        let result = RetryResult::TimedOut {
            attempts: 3,
            elapsed_ms: 30000,
        };
        match result {
            RetryResult::TimedOut { attempts, elapsed_ms } => {
                assert_eq!(attempts, 3);
                assert_eq!(elapsed_ms, 30000);
            }
            _ => panic!("Expected TimedOut"),
        }
    }

    #[test]
    fn test_jitter_variance() {
        let config = RetryConfig::default();
        
        // Delays should vary due to jitter
        let delay1 = config.next_delay(1);
        let delay2 = config.next_delay(1);
        
        // Note: with jitter there may be some variance, but with low jitter (0.1)
        // the variance should be small relative to the base delay
        let diff = (delay1.as_millis() as i64 - delay2.as_millis() as i64).abs();
        let base_expected = (config.base_delay_ms as f64 * config.backoff_multiplier) as i64;
        assert!(diff < base_expected / 2); // Variance should be less than 50% of expected
    }

    #[test]
    fn test_total_timeout_check() {
        let config = RetryConfig::new()
            .with_max_retries(10)
            .with_total_timeout(5000); // 5 second total timeout

        // After many attempts, should not retry due to timeout
        assert!(!config.should_retry(&RetryableError::NetworkError, 20));
    }

    #[test]
    fn test_zero_jitter() {
        let config = RetryConfig::new()
            .with_jitter(0.0);

        // With zero jitter, subsequent calls should give similar results
        let delay1 = config.next_delay(1);
        let delay2 = config.next_delay(1);
        
        // Without jitter, they should be very similar
        let diff = (delay1.as_millis() as i64 - delay2.as_millis() as i64).abs();
        assert!(diff <= 1); // Allow 1ms tolerance for rounding
    }

    #[test]
    fn test_has_required_wait() {
        let rate_limit = RetryableError::RateLimitError { retry_after: 30 };
        assert!(rate_limit.has_required_wait());

        let rate_limit_no_wait = RetryableError::RateLimitError { retry_after: 0 };
        assert!(!rate_limit_no_wait.has_required_wait());

        let service = RetryableError::ServiceUnavailable { retry_after: 60 };
        assert!(service.has_required_wait());

        let network = RetryableError::NetworkError;
        assert!(!network.has_required_wait());
    }

    #[test]
    fn test_retryable_error_from_network_error() {
        let timeout_err = RetryableError::from_network_error("connection timeout");
        assert_eq!(timeout_err, RetryableError::Timeout);

        let network_err = RetryableError::from_network_error("connection refused");
        assert_eq!(network_err, RetryableError::NetworkError);
    }
}