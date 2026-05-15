//! Infrastructure — error handling
pub(crate) mod error_recovery;

// Re-exports for extension hooks (used by lib.rs)
pub use error_recovery::{RetryConfig, RetryableError};
