//! Shared HTTP client singleton for oxi-agent tools.
//!
//! All agent tools that need an HTTP client should use `shared_http_client()`
//! to reuse the same connection pool and TLS sessions across the process lifetime.

use std::sync::OnceLock;

/// Return a process-lifetime shared `reqwest::Client`.
///
/// The client is configured with sensible defaults for tool use:
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
