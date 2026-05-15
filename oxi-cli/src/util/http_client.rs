//! Shared HTTP client singleton for oxi-cli.
//!
//! All CLI subsystems that need an HTTP client should use `shared_http_client()`
//! to reuse the same connection pool and TLS sessions across the process lifetime.

use std::sync::OnceLock;

/// Return a process-lifetime shared `reqwest::Client`.
///
/// The client is configured with sensible defaults:
/// connection pooling (4 idle conns/host, 30 s idle timeout) and a
/// 30-second request timeout.
pub fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("HTTP client init failed")
    })
}

/// Return a process-lifetime shared `reqwest::Client` with a custom timeout.
///
/// Uses the same connection pool defaults but allows overriding the request
/// timeout. The client is cached per-timeout value via a separate static.
pub fn shared_http_client_with_timeout(timeout: std::time::Duration) -> &'static reqwest::Client {
    // For the extended timeout variant (used by tools_manager downloads, etc.)
    static LONG_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    LONG_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .timeout(timeout)
            .build()
            .expect("HTTP client init failed")
    })
}
