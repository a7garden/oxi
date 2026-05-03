//! Output guard for checking assistant output for sensitive data
//!
//! Provides utilities to scan assistant output for potentially sensitive
//! information like API keys, passwords, and other secrets.

use regex::Regex;
use std::sync::LazyLock;

/// Pattern for detecting various sensitive data
static API_KEY_PATTERNS: LazyLock<Vec<(Regex, &str, &str)>> = LazyLock::new(|| {
    vec![
        // Generic API keys
        (
            Regex::new(r"(?i)(api[_-]?key|apikey|api[_-]?secret)\s*[:=]\s*\S{8,}").unwrap(),
            "api_key",
            "Potential API key detected",
        ),
        // AWS keys
        (
            Regex::new(r"(?i)AKIA[0-9A-Z]{16}").unwrap(),
            "aws_access_key",
            "AWS access key ID detected",
        ),
        // AWS secret
        (
            Regex::new(r"(?i)aws[_-]?secret[_-]?access[_-]?key\s*[:=]\s*[a-zA-Z0-9/+=]{40}").unwrap(),
            "aws_secret",
            "AWS secret access key detected",
        ),
        // GitHub tokens
        (
            Regex::new(r"ghp_[a-zA-Z0-9]{36}").unwrap(),
            "github_token",
            "GitHub personal access token detected",
        ),
        (
            Regex::new(r"gho_[a-zA-Z0-9]{36}").unwrap(),
            "github_token",
            "GitHub OAuth token detected",
        ),
        // Private keys
        (
            Regex::new(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----").unwrap(),
            "private_key",
            "Private key detected",
        ),
        // Bearer tokens
        (
            Regex::new(r"(?i)bearer\s+[a-zA-Z0-9_\-\.]{20,}").unwrap(),
            "bearer_token",
            "Bearer token detected",
        ),
        // Basic auth
        (
            Regex::new(r"(?i)basic\s+[a-zA-Z0-9+/=]{20,}").unwrap(),
            "basic_auth",
            "Basic auth credentials detected",
        ),
        // Database URLs with passwords
        (
            Regex::new(r"(?i)(postgres|mysql|mongodb|redis)://[^:]+:[^@]+@").unwrap(),
            "db_url",
            "Database URL with credentials detected",
        ),
        // Slack tokens
        (
            Regex::new(r"xox[baprs]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,}").unwrap(),
            "slack_token",
            "Slack token detected",
        ),
        // Discord tokens
        (
            Regex::new(r"[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27}").unwrap(),
            "discord_token",
            "Discord token detected",
        ),
        // JWT tokens
        (
            Regex::new(r"eyJ[a-zA-Z0-9_-]*\.eyJ[a-zA-Z0-9_-]*\.[a-zA-Z0-9_-]*").unwrap(),
            "jwt",
            "JWT token detected",
        ),
        // Generic secrets
        (
            Regex::new(r"(?i)(secret|password|passwd|pwd|token|auth)\s*[:=]\s*\S{8,}").unwrap(),
            "generic_secret",
            "Potential secret detected",
        ),
        // SSH keys
        (
            Regex::new(r"ssh-rsa\s+[A-Za-z0-9+/=]{30,}").unwrap(),
            "ssh_key",
            "SSH key detected",
        ),
    ]
});

/// Result of an output scan
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// Whether any sensitive data was found
    pub has_sensitive_data: bool,
    /// List of detected items
    pub findings: Vec<Finding>,
}

/// A single finding of sensitive data
#[derive(Debug, Clone)]
pub struct Finding {
    /// Type of sensitive data
    pub category: String,
    /// Description of the finding
    pub description: String,
    /// The matched text (redacted in output)
    pub matched_text: String,
    /// Start position in the original text
    pub start: usize,
    /// End position in the original text
    pub end: usize,
}

impl Finding {
    /// Get a redacted version of the matched text
    pub fn redacted(&self) -> String {
        if self.matched_text.len() <= 8 {
            "*".repeat(self.matched_text.len())
        } else {
            format!(
                "{}...{}",
                &self.matched_text[..4],
                &self.matched_text[self.matched_text.len() - 4..]
            )
        }
    }
}

/// Scan output for sensitive data
///
/// # Arguments
/// * `output` - The text to scan
/// * `strict` - If true, warn on more patterns (may have false positives)
///
/// # Returns
/// A scan result with findings
pub fn scan_output(output: &str, strict: bool) -> ScanResult {
    let mut findings = Vec::new();

    for (pattern, category, description) in API_KEY_PATTERNS.iter() {
        // In non-strict mode, skip generic patterns that may have false positives
        if !strict {
            if *category == "generic_secret" || *category == "api_key" {
                continue;
            }
        }

        for mat in pattern.find_iter(output) {
            findings.push(Finding {
                category: category.to_string(),
                description: description.to_string(),
                matched_text: mat.as_str().to_string(),
                start: mat.start(),
                end: mat.end(),
            });
        }
    }

    ScanResult {
        has_sensitive_data: !findings.is_empty(),
        findings,
    }
}

/// Scan and warn about sensitive data
///
/// Prints warnings to stderr but does not modify the output.
pub fn warn_about_sensitive_data(output: &str) -> ScanResult {
    let result = scan_output(output, false);

    if result.has_sensitive_data {
        for finding in &result.findings {
            eprintln!(
                "Warning: {} at position {}: {}",
                finding.description,
                finding.start,
                finding.redacted()
            );
        }
    }

    result
}

/// Redact sensitive data from output
///
/// Returns the output with sensitive data replaced by [REDACTED].
pub fn redact_sensitive_data(output: &str) -> String {
    let mut result = output.to_string();

    for (pattern, _, _) in API_KEY_PATTERNS.iter() {
        result = pattern.replace_all(&result, "[REDACTED]").to_string();
    }

    result
}

/// Check if a specific string looks like a sensitive value
pub fn is_sensitive_pattern(s: &str) -> bool {
    if s.len() < 8 {
        return false;
    }

    let patterns = [
        r"^[a-zA-Z0-9_\-]{20,}$",
        r"^xox[baprs]-",
        r"^gh[pso]_[a-zA-Z0-9]{36}",
        r"^AKIA[0-9A-Z]{16}$",
        r"^Bearer\s+",
    ];

    patterns.iter().any(|p| {
        Regex::new(p)
            .map(|re| re.is_match(s))
            .unwrap_or(false)
    })
}

/// Get a list of all supported categories
pub fn supported_categories() -> Vec<&'static str> {
    API_KEY_PATTERNS
        .iter()
        .map(|(_, category, _)| *category)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_no_sensitive_data() {
        let output = "Hello, this is a normal response without any secrets.";
        let result = scan_output(output, false);
        assert!(!result.has_sensitive_data);
        assert!(result.findings.is_empty());
    }

    #[test]
    fn test_scan_aws_key() {
        let output = "AWS Key: AKIAIOSFODNN7EXAMPLE";
        let result = scan_output(output, false);
        assert!(result.has_sensitive_data);
        assert_eq!(result.findings[0].category, "aws_access_key");
    }

    #[test]
    fn test_scan_github_token() {
        let output = "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let result = scan_output(output, false);
        assert!(result.has_sensitive_data);
        assert_eq!(result.findings[0].category, "github_token");
    }

    #[test]
    fn test_scan_private_key() {
        let output = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQ...\n-----END RSA PRIVATE KEY-----";
        let result = scan_output(output, false);
        assert!(result.has_sensitive_data);
        assert_eq!(result.findings[0].category, "private_key");
    }

    #[test]
    fn test_scan_jwt() {
        let output = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let result = scan_output(output, false);
        assert!(result.has_sensitive_data);
        assert_eq!(result.findings[0].category, "jwt");
    }

    #[test]
    fn test_scan_db_url() {
        let output = "postgres://user:password@localhost:5432/mydb";
        let result = scan_output(output, false);
        assert!(result.has_sensitive_data);
        assert_eq!(result.findings[0].category, "db_url");
    }

    #[test]
    fn test_redact() {
        let output = "My GitHub token is ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let redacted = redact_sensitive_data(output);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("ghp_"));
    }

    #[test]
    fn test_finding_redacted() {
        let finding = Finding {
            category: "github_token".to_string(),
            description: "GitHub token".to_string(),
            matched_text: "ghp_abcdefghij1234567890abcdefghij12".to_string(),
            start: 0,
            end: 45,
        };
        let redacted = finding.redacted();
        assert!(redacted.starts_with("ghp_"));
        assert!(redacted.ends_with("ij12"));
        assert!(redacted.contains("..."));
    }

    #[test]
    fn test_is_sensitive_pattern() {
        assert!(is_sensitive_pattern("ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"));
        assert!(is_sensitive_pattern("AKIAIOSFODNN7EXAMPLE"));
        assert!(!is_sensitive_pattern("hello"));
        assert!(!is_sensitive_pattern("short"));
    }

    #[test]
    fn test_supported_categories() {
        let categories = supported_categories();
        assert!(categories.contains(&"aws_access_key"));
        assert!(categories.contains(&"github_token"));
        assert!(categories.contains(&"private_key"));
    }

    #[test]
    fn test_strict_vs_non_strict() {
        let output = "secret = my-long-secret-value-here";
        let non_strict = scan_output(output, false);
        let strict = scan_output(output, true);
        // strict mode should detect more or equal to non_strict
        assert!(strict.has_sensitive_data || !strict.findings.is_empty());
        // non_strict should still catch this pattern
        assert!(non_strict.has_sensitive_data || !non_strict.findings.is_empty());
        // both should catch the same data
        assert!(strict.findings.len() >= non_strict.findings.len());
    }
}