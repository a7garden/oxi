//! Path utility functions.
//!
//! Provides path canonicalization and validation utilities.

use std::fs;
use std::path::{Path, PathBuf};

/// Known non-local path prefixes (URLs, package sources, etc.)
const NON_LOCAL_PREFIXES: &[&str] = &[
    "npm:",
    "git:",
    "github:",
    "http:",
    "https:",
    "ssh:",
];

/// Resolve a path to its canonical (real) form, following symlinks.
///
/// Falls back to the raw path if resolution fails (e.g., the target does
/// not exist yet), so that callers never crash on missing filesystem entries.
pub fn canonicalize_path(path: impl AsRef<Path>) -> PathBuf {
    match fs::canonicalize(path.as_ref()) {
        Ok(canonical) => canonical,
        Err(_) => path.as_ref().to_path_buf(),
    }
}

/// Returns true if the value is NOT a package source (npm:, git:, etc.)
/// or a URL protocol. Bare names and relative paths without ./ prefix
/// are considered local.
pub fn is_local_path(value: &str) -> bool {
    let trimmed = value.trim();

    // Check for known non-local prefixes
    for prefix in NON_LOCAL_PREFIXES {
        if trimmed.starts_with(prefix) {
            return false;
        }
    }

    true
}

/// Expand tilde (~) at the start of a path to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
    } else if let Some(suffix) = path.strip_prefix("~/") {
        dirs::home_dir()
            .map(|home| home.join(suffix))
            .unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    }
}

/// Normalize a path for comparison (resolve relative components, handle tilde).
pub fn normalize_path(path: &str) -> PathBuf {
    expand_tilde(path)
}

/// Check if a path exists and is a file.
pub fn is_file(path: impl AsRef<Path>) -> bool {
    path.as_ref().is_file()
}

/// Check if a path exists and is a directory.
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    path.as_ref().is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_local_path() {
        assert!(is_local_path("my-file.txt"));
        assert!(is_local_path("./relative/path"));
        assert!(is_local_path("../parent/path"));
        assert!(is_local_path("/absolute/path"));
        assert!(is_local_path("src/main.rs"));

        assert!(!is_local_path("npm:package-name"));
        assert!(!is_local_path("git:github.com/user/repo"));
        assert!(!is_local_path("github:user/repo"));
        assert!(!is_local_path("http://example.com"));
        assert!(!is_local_path("https://example.com"));
        assert!(!is_local_path("ssh:git@github.com/user/repo"));

        // Whitespace handling
        assert!(!is_local_path("  npm:package"));
        assert!(!is_local_path("npm:package  "));
    }

    #[test]
    fn test_expand_tilde() {
        let home = dirs::home_dir().unwrap();
        let expected = home.join("some/path");

        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/some/path"), expected);
        assert_eq!(expand_tilde("/absolute/path"), PathBuf::from("/absolute/path"));
    }
}
