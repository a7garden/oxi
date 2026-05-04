//! Sleep utilities with abort signal support.
//!
//! Provides async sleep functions that respect tokio cancellation.

use std::time::Duration;
use tokio::time::sleep as tokio_sleep;

/// Sleep for the specified duration, respecting the abort signal.
///
/// Returns `Ok(())` if the sleep completed normally, or `Err` if aborted.
///
/// # Arguments
/// * `ms` - Duration to sleep in milliseconds
/// * `signal` - Optional abort signal (can be any Future that resolves to indicate abort)
pub async fn sleep(ms: u64, signal: impl std::future::Future<Output = ()>) -> Result<(), SleepError> {
    sleep_ms_with_abort(ms, signal).await
}

/// Sleep for the specified duration with abort signal support.
///
/// This is the underlying implementation that uses `tokio::select!`.
async fn sleep_ms_with_abort(
    ms: u64,
    abort: impl std::future::Future<Output = ()>,
) -> Result<(), SleepError> {
    tokio::select! {
        _ = tokio_sleep(Duration::from_millis(ms)) => {
            Ok(())
        }
        _ = abort => {
            Err(SleepError::Aborted)
        }
    }
}

/// Sleep for a duration with optional early return on abort signal.
///
/// This variant checks the abort signal at the start and returns immediately
/// if already aborted, then sleeps for the remaining time.
///
/// # Arguments
/// * `ms` - Duration to sleep in milliseconds
/// * `abort` - Abort signal (returns immediately if already triggered)
pub async fn sleep_until_aborted(
    ms: u64,
    abort: impl std::future::Future<Output = ()>,
) -> Result<(), SleepError> {
    tokio::select! {
        biased; // Check abort first

        _ = abort => {
            Err(SleepError::Aborted)
        }
        _ = tokio_sleep(Duration::from_millis(ms)) => {
            Ok(())
        }
    }
}

/// Sleep for a duration, returning `true` if completed or `false` if aborted.
pub async fn sleep_or_abort(ms: u64, abort: impl std::future::Future<Output = ()>) -> bool {
    tokio::select! {
        _ = tokio_sleep(Duration::from_millis(ms)) => true,
        _ = abort => false,
    }
}

/// Errors that can occur during sleep operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SleepError {
    /// The sleep was aborted by the signal
    Aborted,
    /// The timer had an error (usually shouldn't happen)
    TimerError,
}

impl std::fmt::Display for SleepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SleepError::Aborted => write!(f, "sleep aborted"),
            SleepError::TimerError => write!(f, "timer error"),
        }
    }
}

impl std::error::Error for SleepError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sleep_completes() {
        let result = sleep(10, std::future::pending::<()>()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sleep_aborted() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        // Spawn a task to abort after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let _ = tx.send(());
        });

        let result = sleep(1000, rx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SleepError::Aborted);
    }

    #[tokio::test]
    async fn test_sleep_or_abort_completes() {
        let result = sleep_or_abort(10, std::future::pending::<()>()).await;
        assert!(result);
    }

    #[tokio::test]
    async fn test_sleep_or_abort_aborts() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let _ = tx.send(());
        });

        let result = sleep_or_abort(1000, rx).await;
        assert!(!result);
    }
}
