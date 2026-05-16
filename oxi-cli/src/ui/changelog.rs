//! Changelog parsing utilities.
//!
//! Provides functions for reading and parsing CHANGELOG.md files.

use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// Cached regex for parsing version headers in changelog
static VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").expect("static regex verified at compile time by tests")
});

/// A parsed changelog entry
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogEntry {
    /// Major version number
    pub major: u32,
    /// Minor version number
    pub minor: u32,
    /// Patch version number
    pub patch: u32,
    /// Full content of the entry
    pub content: String,
}

impl ChangelogEntry {
    /// Create a new changelog entry
    pub fn new(major: u32, minor: u32, patch: u32, content: impl Into<String>) -> Self {
        Self {
            major,
            minor,
            patch,
            content: content.into(),
        }
    }

    /// Get the version as a string (e.g., "1.2.3")
    pub fn version_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parse changelog entries from CHANGELOG.md
///
/// Scans for `##` headers and collects content until next `##` or EOF.
pub fn parse_changelog(changelog_path: impl AsRef<Path>) -> Vec<ChangelogEntry> {
    let path = changelog_path.as_ref();

    if !path.exists() {
        return Vec::new();
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Could not read changelog at {:?}: {}", path, e);
            return Vec::new();
        }
    };

    parse_changelog_content(&content)
}

/// Parse changelog entries from a string content
pub fn parse_changelog_content(content: &str) -> Vec<ChangelogEntry> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut entries: Vec<ChangelogEntry> = Vec::new();

    let version_regex = &VERSION_REGEX;

    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_version: Option<(u32, u32, u32)> = None;

    for line in &lines {
        // Check if this is a version header
        if let Some(caps) = version_regex.captures(line) {
            // Save previous entry if exists
            if let Some((major, minor, patch)) = current_version {
                if !current_lines.is_empty() {
                    entries.push(ChangelogEntry::new(
                        major,
                        minor,
                        patch,
                        current_lines.join("\n").trim().to_string(),
                    ));
                }
            }

            // Parse new version
            current_version = Some((
                caps.get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0),
                caps.get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0),
                caps.get(3)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0),
            ));
            current_lines.clear();
            current_lines.push(*line);
        } else if current_version.is_some() {
            // Collect lines for current version
            current_lines.push(*line);
        }
    }

    // Save last entry
    if let Some((major, minor, patch)) = current_version {
        if !current_lines.is_empty() {
            entries.push(ChangelogEntry::new(
                major,
                minor,
                patch,
                current_lines.join("\n").trim().to_string(),
            ));
        }
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CHANGELOG: &str = r#"# Changelog

## [1.0.0] - 2024-01-01

### Added
- Initial release
- Feature A

## [0.9.0] - 2023-12-01

### Changed
- Updated dependencies

### Fixed
- Bug in feature B

## [0.8.0] - 2023-11-01

### Added
- Beta features
"#;

    #[test]
    fn test_parse_changelog() {
        let entries = parse_changelog_content(SAMPLE_CHANGELOG);

        assert_eq!(entries.len(), 3);

        // Check first entry (1.0.0)
        assert_eq!(entries[0].major, 1);
        assert_eq!(entries[0].minor, 0);
        assert_eq!(entries[0].patch, 0);
        assert!(entries[0].content.contains("Initial release"));

        // Check second entry (0.9.0)
        assert_eq!(entries[1].major, 0);
        assert_eq!(entries[1].minor, 9);
        assert_eq!(entries[1].patch, 0);

        // Check third entry (0.8.0)
        assert_eq!(entries[2].major, 0);
        assert_eq!(entries[2].minor, 8);
        assert_eq!(entries[2].patch, 0);
    }

    #[test]
    fn test_version_string() {
        let entry = ChangelogEntry::new(1, 2, 3, "");
        assert_eq!(entry.version_string(), "1.2.3");
    }
}
