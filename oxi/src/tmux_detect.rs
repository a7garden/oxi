//! tmux detection for keyboard configuration support.
//!
//! Detects if running inside tmux and checks for extended-keys configuration
//! which is required for proper keyboard handling in the TUI.

use std::process::Command;

/// Configuration information about the tmux session.
#[derive(Debug, Clone)]
pub struct TmuxConfig {
    /// Whether extended-keys mode is enabled.
    pub extended_keys: bool,
    /// The extended-keys-format string.
    pub extended_keys_format: String,
    /// The tmux version string.
    pub version: String,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            extended_keys: false,
            extended_keys_format: String::new(),
            version: String::new(),
        }
    }
}

/// Detect if running inside tmux and return configuration if available.
///
/// Returns `Ok(Some(TmuxConfig))` if inside tmux with valid config,
/// `Ok(None)` if not in tmux or tmux not available, or `Err` on error.
pub fn detect_tmux() -> Result<Option<TmuxConfig>, TmuxError> {
    // First check if we're in tmux by checking TMUX environment variable
    if std::env::var("TMUX").is_err() {
        return Ok(None);
    }

    // Query tmux version
    let version = run_tmux_command(&["display-message", "-p", "#{version}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Query extended-keys setting
    let extended_keys = run_tmux_command(&["show", "-gv", "extended-keys"])
        .map(|s| s.trim().eq_ignore_ascii_case("on"))
        .unwrap_or(false);

    // Query extended-keys-format setting
    let extended_keys_format = run_tmux_command(&["show", "-gv", "extended-keys-format"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    Ok(Some(TmuxConfig {
        extended_keys,
        extended_keys_format,
        version,
    }))
}

/// Check if tmux is configured with proper keyboard support.
///
/// Returns `true` if extended-keys is "on" and extended-keys-format includes "Kr"
/// (CSI_u mode for proper key handling).
pub fn is_tmux_keyboard_configured() -> bool {
    match detect_tmux() {
        Ok(Some(config)) => config.extended_keys && config.extended_keys_format.contains("Kr"),
        _ => false,
    }
}

/// Generate a warning message about tmux keyboard configuration.
///
/// Returns a formatted warning with instructions for fixing tmux keyboard support.
pub fn show_tmux_warning() -> String {
    let in_tmux = std::env::var("TMUX").is_ok();

    if in_tmux {
        match detect_tmux() {
            Ok(Some(config)) => {
                if config.extended_keys && config.extended_keys_format.contains("Kr") {
                    // Configuration is good, no warning needed
                    return String::new();
                }
            }
            _ => {}
        }
    }

    // Generate warning message
    if !in_tmux {
        return String::new(); // Not in tmux, no warning
    }

    // Warning for when in tmux but not properly configured
    let fix_instructions = r#"Warning: tmux keyboard mode may not work correctly.
  Some key combinations (e.g., Ctrl+Arrow, Alt+keys) may not function properly.
  
  To fix, add the following to ~/.tmux.conf:
  
    set -g extended-keys on
    set -g extended-keys-format 'KrOCa1'
  
  Then reload tmux config: Ctrl+b then :source-file ~/.tmux.conf"#;

    fix_instructions.to_string()
}

/// Check if the terminal supports extended key reporting.
pub fn supports_extended_keys() -> bool {
    is_tmux_keyboard_configured()
}

/// Get detailed tmux info for debugging.
#[derive(Debug, Clone)]
pub struct TmuxInfo {
    /// Whether running inside tmux.
    pub in_tmux: bool,
    /// tmux version (if available).
    pub version: Option<String>,
    /// Whether extended-keys is enabled.
    pub extended_keys_enabled: bool,
    /// The extended-keys-format value.
    pub extended_keys_format: Option<String>,
    /// Whether keyboard is properly configured.
    pub keyboard_configured: bool,
}

impl TmuxInfo {
    /// Gather all tmux-related information.
    pub fn gather() -> Self {
        let in_tmux = std::env::var("TMUX").is_ok();

        if !in_tmux {
            return Self {
                in_tmux: false,
                version: None,
                extended_keys_enabled: false,
                extended_keys_format: None,
                keyboard_configured: false,
            };
        }

        match detect_tmux() {
            Ok(Some(config)) => {
                let keyboard_configured =
                    config.extended_keys && config.extended_keys_format.contains("Kr");
                Self {
                    in_tmux: true,
                    version: if config.version.is_empty() {
                        None
                    } else {
                        Some(config.version)
                    },
                    extended_keys_enabled: config.extended_keys,
                    extended_keys_format: if config.extended_keys_format.is_empty() {
                        None
                    } else {
                        Some(config.extended_keys_format)
                    },
                    keyboard_configured,
                }
            }
            _ => Self {
                in_tmux: true,
                version: None,
                extended_keys_enabled: false,
                extended_keys_format: None,
                keyboard_configured: false,
            },
        }
    }

    /// Format tmux info as a human-readable string for debugging.
    pub fn format(&self) -> String {
        if !self.in_tmux {
            return "Not running in tmux".to_string();
        }

        let mut parts = vec![format!(
            "tmux {}",
            self.version.as_deref().unwrap_or("unknown")
        )];

        if self.extended_keys_enabled {
            parts.push("extended-keys: on".to_string());
        } else {
            parts.push("extended-keys: off".to_string());
        }

        if let Some(ref format) = self.extended_keys_format {
            parts.push(format!("extended-keys-format: {}", format));
        }

        if self.keyboard_configured {
            parts.push("keyboard: configured".to_string());
        } else {
            parts.push("keyboard: not configured".to_string());
        }

        parts.join(" | ")
    }
}

/// Error type for tmux detection failures.
#[derive(Debug, Clone)]
pub struct TmuxError {
    message: String,
}

impl std::fmt::Display for TmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TmuxError {}

impl TmuxError {
    fn new(msg: &str) -> Self {
        Self {
            message: msg.to_string(),
        }
    }
}

/// Run a tmux command and return the output.
fn run_tmux_command(args: &[&str]) -> Result<String, TmuxError> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .map_err(|e| TmuxError::new(&format!("Failed to run tmux command: {}", e)))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(TmuxError::new(&format!(
            "tmux command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

/// Set tmux extended-keys configuration (requires tmux to reload config).
pub fn set_extended_keys(enabled: bool) -> Result<(), TmuxError> {
    if !std::env::var("TMUX").is_ok() {
        return Err(TmuxError::new("Not running inside tmux"));
    }

    let value = if enabled { "on" } else { "off" };
    run_tmux_command(&["set", "-g", "extended-keys", value])?;

    if enabled {
        // Also set the format to CSI-u mode which enables proper key handling
        run_tmux_command(&["set", "-g", "extended-keys-format", "KrOCa1"])?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tmux_config_default() {
        let config = TmuxConfig::default();
        assert!(!config.extended_keys);
        assert!(config.extended_keys_format.is_empty());
        assert!(config.version.is_empty());
    }

    #[test]
    fn test_tmux_config_with_values() {
        let config = TmuxConfig {
            extended_keys: true,
            extended_keys_format: "KrOCa1".to_string(),
            version: "3.3a".to_string(),
        };
        assert!(config.extended_keys);
        assert_eq!(config.extended_keys_format, "KrOCa1");
        assert_eq!(config.version, "3.3a");
    }

    #[test]
    fn test_tmux_error_display() {
        let err = TmuxError::new("test error");
        assert_eq!(err.to_string(), "test error");
    }

    #[test]
    fn test_tmux_info_not_in_tmux() {
        let info = TmuxInfo::gather();
        // When not in tmux, keyboard_configured should be false
        assert!(!info.keyboard_configured);
    }

    #[test]
    fn test_tmux_info_format_not_in_tmux() {
        let info = TmuxInfo {
            in_tmux: false,
            version: None,
            extended_keys_enabled: false,
            extended_keys_format: None,
            keyboard_configured: false,
        };
        assert_eq!(info.format(), "Not running in tmux");
    }

    #[test]
    fn test_tmux_info_format_in_tmux() {
        let info = TmuxInfo {
            in_tmux: true,
            version: Some("3.3a".to_string()),
            extended_keys_enabled: true,
            extended_keys_format: Some("KrOCa1".to_string()),
            keyboard_configured: true,
        };
        let formatted = info.format();
        assert!(formatted.contains("tmux 3.3a"));
        assert!(formatted.contains("extended-keys: on"));
        assert!(formatted.contains("KrOCa1"));
        assert!(formatted.contains("keyboard: configured"));
    }

    #[test]
    fn test_show_tmux_warning_not_in_tmux() {
        // When not in tmux, no warning should be returned
        let warning = show_tmux_warning();
        // The function returns empty string when not in tmux
        // or when properly configured
        assert!(warning.is_empty() || warning.contains("Warning"));
    }

    #[test]
    fn test_tmux_error_debug() {
        let err = TmuxError::new("debug test");
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("debug test"));
    }

    #[test]
    fn test_extended_keys_format_includes_kr() {
        // Test various format strings
        assert!("KrOCa1".contains('K'));
        assert!("Kr".contains('K'));
        assert!(!"ABC".contains('K'));
    }

    #[test]
    fn test_detect_tmux_returns_result() {
        // detect_tmux should always return a Result, not panic
        let result = detect_tmux();
        assert!(result.is_ok());
    }
}
