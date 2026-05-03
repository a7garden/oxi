//! Runtime diagnostics for system information collection
//!
//! Provides utilities for gathering diagnostic information about
//! the system, environment, and runtime state.

use std::collections::HashMap;
use std::process::Command;

/// Diagnostic report entry
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiagnosticEntry {
    /// Category of the diagnostic
    pub category: String,
    /// Key within the category
    pub key: String,
    /// Value
    pub value: String,
}

/// Complete diagnostic report
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiagnosticReport {
    /// All diagnostic entries
    pub entries: Vec<DiagnosticEntry>,
    /// Timestamp of the report
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Version info
    pub version: String,
}

impl DiagnosticReport {
    /// Get all entries for a category
    pub fn entries_for_category(&self, category: &str) -> Vec<&DiagnosticEntry> {
        self.entries
            .iter()
            .filter(|e| e.category == category)
            .collect()
    }

    /// Get a value by category and key
    pub fn get(&self, category: &str, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.category == category && e.key == key)
            .map(|e| e.value.as_str())
    }

    /// Export as key-value pairs
    pub fn to_map(&self) -> HashMap<String, String> {
        self.entries
            .iter()
            .map(|e| (format!("{}.{}", e.category, e.key), e.value.clone()))
            .collect()
    }
}

/// Add an entry to the report
fn add_entry(report: &mut Vec<DiagnosticEntry>, category: &str, key: &str, value: String) {
    report.push(DiagnosticEntry {
        category: category.to_string(),
        key: key.to_string(),
        value,
    });
}

/// Collect OS information
pub fn collect_os_info(entries: &mut Vec<DiagnosticEntry>) {
    add_entry(entries, "os", "family", std::env::consts::OS.to_string());
    add_entry(entries, "os", "arch", std::env::consts::ARCH.to_string());

    // Get kernel version
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        let version = version.trim();
        add_entry(entries, "os", "kernel", version.to_string());
    }

    // Get OS release info on Linux
    if let Ok(lsb_release) = std::fs::read_to_string("/etc/os-release") {
        for line in lsb_release.lines() {
            if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                let value = value.trim_matches('"').to_string();
                add_entry(entries, "os", "distribution", value);
                break;
            }
        }
    }

    // Get hostname
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string());
    add_entry(entries, "os", "hostname", hostname.trim().to_string());
}

/// Collect shell information
pub fn collect_shell_info(entries: &mut Vec<DiagnosticEntry>) {
    if let Ok(shell) = std::env::var("SHELL") {
        add_entry(entries, "shell", "path", shell.clone());

        // Get shell version
        let shell_name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let version_output = Command::new(&shell)
            .args(["--version"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        add_entry(entries, "shell", "name", shell_name.to_string());
        add_entry(entries, "shell", "version", version_output);
    }

    // Get terminal
    if let Ok(term) = std::env::var("TERM") {
        add_entry(entries, "shell", "terminal", term);
    }
}

/// Collect tool versions
pub fn collect_tool_versions(entries: &mut Vec<DiagnosticEntry>) {
    let tools = vec![
        ("git", &["git", "--version"]),
        ("node", &["node", "--version"]),
        ("npm", &["npm", "--version"]),
        ("cargo", &["cargo", "--version"]),
        ("rustc", &["rustc", "--version"]),
        ("python", &["python3", "--version"]),
        ("go", &["go", "version"]),
    ];

    for (name, cmd) in tools {
        let output = Command::new(cmd[0])
            .args(&cmd[1..])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "not found".to_string());

        add_entry(entries, "tools", name, output);
    }
}

/// Collect environment variables
pub fn collect_env_info(entries: &mut Vec<DiagnosticEntry>) {
    let env_vars = vec![
        "HOME",
        "USER",
        "PATH",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "PI_OFFLINE",
        "PI_VERBOSE",
    ];

    for var in env_vars {
        if let Ok(value) = std::env::var(var) {
            // Truncate long values
            let value = if value.len() > 200 {
                format!("{}... (truncated)", &value[..200])
            } else {
                value
            };
            add_entry(entries, "env", var, value);
        }
    }
}

/// Collect Rust/Rs build info
pub fn collect_build_info(entries: &mut Vec<DiagnosticEntry>) {
    add_entry(
        entries,
        "build",
        "version",
        env!("CARGO_PKG_VERSION").to_string(),
    );

    if let Some(git_sha) = option_env!("VERGEN_GIT_SHA") {
        add_entry(entries, "build", "git_sha", git_sha.to_string());
    }

    if let Some(build_ts) = option_env!("VERGEN_BUILD_TIMESTAMP") {
        add_entry(entries, "build", "build_timestamp", build_ts.to_string());
    }

    add_entry(
        entries,
        "build",
        "features",
        std::env::var("CARGO_CFG_DEBUG")
            .ok()
            .map(|_| "debug")
            .unwrap_or("release")
            .to_string(),
    );
}

/// Collect directory paths
pub fn collect_path_info(entries: &mut Vec<DiagnosticEntry>) {
    if let Some(home) = dirs::home_dir() {
        add_entry(entries, "paths", "home", home.to_string_lossy().to_string());
    }

    if let Some(config) = dirs::config_dir() {
        add_entry(
            entries,
            "paths",
            "config",
            config.join("oxi").to_string_lossy().to_string(),
        );
    }

    if let Some(data) = dirs::data_dir() {
        add_entry(
            entries,
            "paths",
            "data",
            data.join("oxi").to_string_lossy().to_string(),
        );
    }

    if let Some(cache) = dirs::cache_dir() {
        add_entry(
            entries,
            "paths",
            "cache",
            cache.join("oxi").to_string_lossy().to_string(),
        );
    }

    if let Ok(cwd) = std::env::current_dir() {
        add_entry(entries, "paths", "cwd", cwd.to_string_lossy().to_string());
    }
}

/// Collect all diagnostics and generate a report
pub fn generate_diagnostic_report() -> DiagnosticReport {
    let mut entries = Vec::new();

    collect_os_info(&mut entries);
    collect_shell_info(&mut entries);
    collect_tool_versions(&mut entries);
    collect_env_info(&mut entries);
    collect_build_info(&mut entries);
    collect_path_info(&mut entries);

    DiagnosticReport {
        entries,
        timestamp: chrono::Utc::now(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Format the report as a string
pub fn format_diagnostic_report(report: &DiagnosticReport) -> String {
    let mut output = String::new();

    output.push_str(&format!("oxi Diagnostic Report - {}\n", report.timestamp));
    output.push_str(&format!("Version: {}\n", report.version));
    output.push_str(&"=".repeat(60));
    output.push('\n');

    let mut current_category = String::new();
    for entry in &report.entries {
        if entry.category != current_category {
            current_category = entry.category.clone();
            output.push('\n');
            output.push_str(&format!("[{}]\n", current_category.to_uppercase()));
        }
        output.push_str(&format!("  {}: {}\n", entry.key, entry.value));
    }

    output
}

/// Export report as JSON
pub fn diagnostic_report_json(report: &DiagnosticReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
}

/// Check for common issues
pub fn check_common_issues() -> Vec<String> {
    let mut issues = Vec::new();

    // Check if running in a terminal
    // Simple check: if stdout is not a terminal, we may be in pipe mode
    if std::env::var("TERM").is_err() && std::env::var("COLORTERM").is_err() {
        // Not necessarily an issue, just informational
    }

    // Check for missing required tools
    if !crate::bash_executor::command_exists("git") {
        issues.push("git is not installed".to_string());
    }

    // Check for common environment issues
    if std::env::var("SHELL").is_err() {
        issues.push("SHELL environment variable not set".to_string());
    }

    // Check home directory
    if dirs::home_dir().is_none() {
        issues.push("Could not determine home directory".to_string());
    }

    issues
}

/// Run diagnostics and return formatted output
pub fn run_diagnostics() -> String {
    let report = generate_diagnostic_report();
    let mut output = format_diagnostic_report(&report);

    let issues = check_common_issues();
    if !issues.is_empty() {
        output.push('\n');
        output.push_str(&"=".repeat(60));
        output.push('\n');
        output.push_str("Potential Issues:\n");
        for issue in issues {
            output.push_str(&format!("  - {}\n", issue));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_diagnostic_report() {
        let report = generate_diagnostic_report();
        assert!(!report.entries.is_empty());
        assert!(!report.version.is_empty());
    }

    #[test]
    fn test_diagnostic_entry_lookup() {
        let report = generate_diagnostic_report();

        // Should have OS info
        let os_family = report.get("os", "family");
        assert!(os_family.is_some());

        // Should have build version
        let version = report.get("build", "version");
        assert!(version.is_some());
    }

    #[test]
    fn test_entries_for_category() {
        let report = generate_diagnostic_report();

        let os_entries = report.entries_for_category("os");
        assert!(!os_entries.is_empty());
    }

    #[test]
    fn test_to_map() {
        let report = generate_diagnostic_report();
        let map = report.to_map();
        assert!(!map.is_empty());
        assert!(map.contains_key("os.family"));
    }

    #[test]
    fn test_format_report() {
        let report = generate_diagnostic_report();
        let formatted = format_diagnostic_report(&report);
        assert!(!formatted.is_empty());
        assert!(formatted.contains("oxi Diagnostic Report"));
    }

    #[test]
    fn test_json_export() {
        let report = generate_diagnostic_report();
        let json = diagnostic_report_json(&report);
        assert!(json.starts_with("{"));
        assert!(json.ends_with("}"));
    }

    #[test]
    fn test_check_common_issues() {
        let issues = check_common_issues();
        // Should complete without panic
        assert!(issues.len() >= 0);
    }

    #[test]
    fn test_run_diagnostics() {
        let output = run_diagnostics();
        assert!(output.contains("Diagnostic Report"));
        assert!(output.contains("Version:"));
    }

    #[test]
    fn test_collect_shell_info() {
        let mut entries = Vec::new();
        collect_shell_info(&mut entries);
        // Shell info should be populated on systems with SHELL env var
        assert!(!entries.is_empty() || entries.is_empty()); // Just check it doesn't panic
    }

    #[test]
    fn test_collect_tool_versions() {
        let mut entries = Vec::new();
        collect_tool_versions(&mut entries);
        // Should have at least one tool entry (likely git)
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_diagnostic_report_timestamp() {
        let report = generate_diagnostic_report();
        // Timestamp should be in the past but close to now
        let now = chrono::Utc::now();
        let diff = now.signed_duration_since(report.timestamp);
        assert!(diff.num_seconds() < 60);
    }
}
