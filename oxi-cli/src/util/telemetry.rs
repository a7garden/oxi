//! Telemetry opt-in/out handling.
//!
//! Telemetry utilities for oxi.

/// Check whether install telemetry is enabled.
///
/// Resolution order:
/// 1. If the `OXI_TELEMETRY` environment variable is set, parse it as a truthy flag.
/// 2. Otherwise, fall back to the provided settings callback.
///
/// Truthy values: `"1"`, `"true"`, `"yes"` (case-insensitive).
pub fn is_install_telemetry_enabled<F>(settings_getter: F) -> bool
where
    F: FnOnce() -> bool,
{
    match std::env::var("OXI_TELEMETRY") {
        Ok(val) => is_truthy_env_flag(&val),
        Err(_) => settings_getter(),
    }
}

/// Parse a string value as a truthy environment flag.
///
/// Returns `true` for `"1"`, `"true"`, or `"yes"` (case-insensitive).
/// Returns `false` for empty strings or any other value.
pub fn is_truthy_env_flag(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    matches!(lower.as_str(), "1" | "true" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthy_values() {
        assert!(is_truthy_env_flag("1"));
        assert!(is_truthy_env_flag("true"));
        assert!(is_truthy_env_flag("True"));
        assert!(is_truthy_env_flag("TRUE"));
        assert!(is_truthy_env_flag("yes"));
        assert!(is_truthy_env_flag("Yes"));
        assert!(is_truthy_env_flag("YES"));
    }

    #[test]
    fn falsy_values() {
        assert!(!is_truthy_env_flag(""));
        assert!(!is_truthy_env_flag("0"));
        assert!(!is_truthy_env_flag("false"));
        assert!(!is_truthy_env_flag("no"));
    }

    #[test]
    fn env_var_takes_precedence() {
        // With env set to "1", settings getter should be ignored
        std::env::set_var("OXI_TELEMETRY", "1");
        assert!(is_install_telemetry_enabled(|| false));
        std::env::remove_var("OXI_TELEMETRY");

        // Without env, settings getter is used
        assert!(is_install_telemetry_enabled(|| true));
        assert!(!is_install_telemetry_enabled(|| false));
    }
}
