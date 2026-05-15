//! Changelog parsing utilities.
//!
//! Provides functions for reading and parsing CHANGELOG.md files.

use regex::Regex;
use std::sync::LazyLock;
use std::fs;
use std::path::Path;

/// Cached regex for parsing version headers in changelog
static VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").unwrap() // SAFE: static regex verified at compile time by tests
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
                caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
                caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
                caps.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
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

/// Compare two versions.
///
/// Returns:
/// - `-1` if v1 < v2
/// - `0` if v1 == v2
/// - `1` if v1 > v2
pub fn compare_versions(v1: &ChangelogEntry, v2: &ChangelogEntry) -> i32 {
    if v1.major != v2.major {
        return (v1.major as i32) - (v2.major as i32);
    }
    if v1.minor != v2.minor {
        return (v1.minor as i32) - (v2.minor as i32);
    }
    (v1.patch as i32) - (v2.patch as i32)
}

/// Get entries newer than a given version string.
///
/// # Arguments
/// * `entries` - List of changelog entries
/// * `last_version` - Version string to compare against (e.g., "1.2.3")
pub fn get_new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry> {
    let parts: Vec<u32> = last_version
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    let last = ChangelogEntry::new(
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
        "",
    );

    entries
        .iter()
        .filter(|entry| compare_versions(entry, &last) > 0)
        .cloned()
        .collect()
}

/// Get changelog entries for a specific version.
pub fn get_entry_for_version<'a>(
    entries: &'a [ChangelogEntry],
    version: &str,
) -> Option<&'a ChangelogEntry> {
    let parts: Vec<u32> = version
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    let major = parts.first().copied().unwrap_or(0);
    let minor = parts.get(1).copied().unwrap_or(0);
    let patch = parts.get(2).copied().unwrap_or(0);

    entries.iter().find(|e| e.major == major && e.minor == minor && e.patch == patch)
}

/// Format a changelog entry for display.
pub fn format_changelog_entry(entry: &ChangelogEntry, include_header: bool) -> String {
    let mut output = String::new();

    if include_header {
        output.push_str(&format!("## {}\n\n", entry.version_string()));
    }

    output.push_str(&entry.content);

    output
}

/// Get the latest version from changelog entries.
pub fn get_latest_version(entries: &[ChangelogEntry]) -> Option<ChangelogEntry> {
    entries.iter().max_by(|a, b| compare_versions(a, b).cmp(&0)).cloned()
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
    fn test_compare_versions() {
        let v1_0_0 = ChangelogEntry::new(1, 0, 0, "");
        let v0_9_0 = ChangelogEntry::new(0, 9, 0, "");
        let v1_0_1 = ChangelogEntry::new(1, 0, 1, "");

        assert_eq!(compare_versions(&v1_0_0, &v0_9_0), 1);
        assert_eq!(compare_versions(&v0_9_0, &v1_0_0), -1);
        assert_eq!(compare_versions(&v1_0_0, &v1_0_0), 0);
        assert_eq!(compare_versions(&v1_0_1, &v1_0_0), 1);
    }

    #[test]
    fn test_get_new_entries() {
        let entries = parse_changelog_content(SAMPLE_CHANGELOG);
        let new_entries = get_new_entries(&entries, "0.9.0");

        assert_eq!(new_entries.len(), 1);
        assert_eq!(new_entries[0].major, 1);
        assert_eq!(new_entries[0].minor, 0);
        assert_eq!(new_entries[0].patch, 0);
    }

    #[test]
    fn test_get_latest_version() {
        let entries = parse_changelog_content(SAMPLE_CHANGELOG);
        let latest = get_latest_version(&entries);

        assert!(latest.is_some());
        assert_eq!(latest.unwrap().version_string(), "1.0.0");
    }

    #[test]
    fn test_version_string() {
        let entry = ChangelogEntry::new(1, 2, 3, "");
        assert_eq!(entry.version_string(), "1.2.3");
    }

    #[test]
    fn test_format_changelog_entry() {
        let entry = ChangelogEntry::new(1, 0, 0, "### Added\n- Feature A");
        
        let with_header = format_changelog_entry(&entry, true);
        assert!(with_header.starts_with("## 1.0.0"));
        assert!(with_header.contains("Feature A"));

        let without_header = format_changelog_entry(&entry, false);
        assert!(!without_header.starts_with("## "));
        assert!(without_header.contains("Feature A"));
    }
}
