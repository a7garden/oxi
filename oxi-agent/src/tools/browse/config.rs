//! Configuration for browser tools behavior.
//!
//! Centralizes all tunable parameters so no values are hardcoded.
//! Default values are suitable for most use cases; override via
//! `settings.toml` or programmatically.

use serde::{Deserialize, Serialize};

/// Configuration for browser tools behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseConfig {
    /// Default `wait_for` timeout in milliseconds.
    #[serde(default = "default_wait_timeout_ms")]
    pub default_wait_timeout_ms: u64,

    /// Default page load timeout in seconds.
    #[serde(default = "default_page_timeout_secs")]
    pub page_timeout_secs: u64,

    /// Screenshot width in pixels.
    #[serde(default = "default_screenshot_width")]
    pub screenshot_width: u32,

    /// Maximum script steps per execution.
    #[serde(default = "default_max_script_steps")]
    pub max_script_steps: usize,

    /// Render cache TTL in seconds (0 = disabled).
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,

    /// Maximum render cache entries.
    #[serde(default = "default_cache_max_entries")]
    pub cache_max_entries: usize,

    /// Maximum concurrent tabs.
    #[serde(default = "default_max_concurrent_tabs")]
    pub max_concurrent_tabs: usize,

    /// Maximum output size in bytes (truncation threshold).
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,

    /// Maximum idle time (seconds) before a browse session auto-closes.
    /// 0 = no timeout (not recommended).
    #[serde(default = "default_session_idle_timeout_secs")]
    pub session_idle_timeout_secs: u64,

    /// Custom User-Agent string. `None` uses the browser default.
    #[serde(default)]
    pub user_agent: Option<String>,

    /// Whether to respect robots.txt. Defaults to `true`.
    #[serde(default = "default_obey_robots")]
    pub obey_robots: bool,

    /// JavaScript evaluation timeout in milliseconds.
    #[serde(default = "default_js_timeout_ms")]
    pub js_timeout_ms: u64,
}

impl Default for BrowseConfig {
    fn default() -> Self {
        Self {
            default_wait_timeout_ms: default_wait_timeout_ms(),
            page_timeout_secs: default_page_timeout_secs(),
            screenshot_width: default_screenshot_width(),
            max_script_steps: default_max_script_steps(),
            cache_ttl_secs: default_cache_ttl_secs(),
            cache_max_entries: default_cache_max_entries(),
            max_concurrent_tabs: default_max_concurrent_tabs(),
            max_output_bytes: default_max_output_bytes(),
            session_idle_timeout_secs: default_session_idle_timeout_secs(),
            user_agent: None,
            obey_robots: default_obey_robots(),
            js_timeout_ms: default_js_timeout_ms(),
        }
    }
}

// ── Default value functions (for serde defaults) ─────────────────

fn default_wait_timeout_ms() -> u64 {
    10_000
}
fn default_page_timeout_secs() -> u64 {
    30
}
fn default_screenshot_width() -> u32 {
    800
}
fn default_max_script_steps() -> usize {
    100
}
fn default_cache_ttl_secs() -> u64 {
    300
}
fn default_cache_max_entries() -> usize {
    50
}
fn default_max_concurrent_tabs() -> usize {
    4
}
fn default_max_output_bytes() -> usize {
    512_000
}
fn default_session_idle_timeout_secs() -> u64 {
    300 // 5 minutes
}
fn default_obey_robots() -> bool {
    true
}
fn default_js_timeout_ms() -> u64 {
    10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BrowseConfig::default();
        assert_eq!(config.default_wait_timeout_ms, 10_000);
        assert_eq!(config.page_timeout_secs, 30);
        assert_eq!(config.screenshot_width, 800);
        assert_eq!(config.max_script_steps, 100);
        assert_eq!(config.cache_ttl_secs, 300);
        assert_eq!(config.cache_max_entries, 50);
        assert_eq!(config.max_concurrent_tabs, 4);
        assert_eq!(config.max_output_bytes, 512_000);
        assert_eq!(config.session_idle_timeout_secs, 300);
        assert!(config.user_agent.is_none());
        assert!(config.obey_robots);
        assert_eq!(config.js_timeout_ms, 10_000);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = BrowseConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let restored: BrowseConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.default_wait_timeout_ms,
            config.default_wait_timeout_ms
        );
        assert_eq!(restored.max_concurrent_tabs, config.max_concurrent_tabs);
    }

    #[test]
    fn test_config_custom_values() {
        let config = BrowseConfig {
            default_wait_timeout_ms: 30_000,
            max_concurrent_tabs: 8,
            cache_ttl_secs: 0, // disabled
            session_idle_timeout_secs: 600,
            ..Default::default()
        };
        assert_eq!(config.default_wait_timeout_ms, 30_000);
        assert_eq!(config.max_concurrent_tabs, 8);
        assert_eq!(config.cache_ttl_secs, 0);
        assert_eq!(config.session_idle_timeout_secs, 600);
    }
}
