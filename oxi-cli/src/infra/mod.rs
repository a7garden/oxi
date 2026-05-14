//! Infrastructure — process execution, error handling, events, monitoring
pub(crate) mod bash_executor;
pub(crate) mod child_process;
pub(crate) mod error_recovery;
pub(crate) mod event_bus;
pub(crate) mod output_guard;
pub(crate) mod tools_manager;
pub(crate) mod version_check;
pub(crate) mod diagnostics;
pub(crate) mod fs_watch;
pub(crate) mod shutdown;

// Re-exports for extension hooks (used by lib.rs)
pub use error_recovery::{RetryConfig, RetryableError};
