//! File path completion provider.
//!
//! Completes file and directory paths from the filesystem.
//! Supports `~` expansion, relative paths, and case-insensitive matching.

use super::{CompletionItem, CompletionKind};
use std::path::{Path, PathBuf};

/// Complete a file path prefix.
///
/// Steps:
/// 1. Expand `~` to home directory
/// 2. Split into directory and prefix
/// 3. Read directory entries
/// 4. Filter by case-insensitive prefix match
/// 5. Sort directories first, then files
pub fn complete_path(input: &str, cwd: &Path) -> Vec<CompletionItem> {
    let expanded = expand_tilde(input);
    let (dir, prefix) = split_dir_prefix(&expanded, cwd);

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let lower_prefix = prefix.to_lowercase();
    let mut results: Vec<CompletionItem> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Filter by prefix (case-insensitive)
            if !lower_prefix.is_empty() && !name.to_lowercase().starts_with(&lower_prefix) {
                return None;
            }

            let is_dir = entry.file_type().ok()?.is_dir();
            let display_name = if is_dir {
                format!("{}/", name)
            } else {
                name.clone()
            };

            // Compute the replacement text
            let replacement = if input.starts_with('~') {
                format!("~/{}", name)
            } else if input.starts_with('/') {
                format!("{}/{}", dir.display(), name)
            } else {
                // Compute relative path from cwd
                let full_path = entry.path();
                let rel = make_relative(&full_path, cwd);
                rel.to_string_lossy().to_string()
            };

            Some(CompletionItem {
                text: replacement,
                label: display_name,
                description: if is_dir {
                    Some("directory".to_string())
                } else {
                    Some("file".to_string())
                },
                kind: CompletionKind::FilePath,
            })
        })
        .collect();

    // Sort: directories first, then alphabetical
    results.sort_by(|a, b| {
        let a_dir = a.description.as_deref() == Some("directory");
        let b_dir = b.description.as_deref() == Some("directory");
        b_dir.cmp(&a_dir).then_with(|| a.label.cmp(&b.label))
    });

    // Limit results
    results.truncate(20);
    results
}

/// Make a path relative to a base directory (simplified).
/// Falls back to the full path if no common prefix.
fn make_relative(path: &Path, base: &Path) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix(base) {
        stripped.to_path_buf()
    } else {
        path.to_path_buf()
    }
}

/// Expand `~` to the user's home directory.
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs::home_dir() {
            if path == "~" {
                return home;
            }
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Split a path into (directory, prefix) components.
///
/// For example:
/// - `"src/oxi/mai"` → `("src/oxi", "mai")`
/// - `"src/"` → `("src", "")`
/// - `"foo"` → `(".", "foo")`
fn split_dir_prefix(path: &Path, cwd: &Path) -> (PathBuf, String) {
    let path_str = path.to_string_lossy();

    // If it ends with '/', the whole thing is a directory
    if path_str.ends_with('/') {
        let dir = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        return (dir, String::new());
    }

    // Split at the last '/'
    if let Some(parent) = path.parent() {
        let prefix = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        let dir = if parent.as_os_str().is_empty() {
            cwd.to_path_buf()
        } else if parent.is_absolute() {
            parent.to_path_buf()
        } else {
            cwd.join(parent)
        };

        (dir, prefix)
    } else {
        (cwd.to_path_buf(), path_str.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/Documents");
        assert!(expanded.to_string_lossy().contains("Documents"));
        assert!(!expanded.starts_with("~"));
    }

    #[test]
    fn test_expand_no_tilde() {
        let expanded = expand_tilde("/usr/local");
        assert_eq!(expanded, PathBuf::from("/usr/local"));
    }

    #[test]
    fn test_split_dir_prefix_with_parent() {
        let (dir, prefix) = split_dir_prefix(Path::new("src/mai"), Path::new("/project"));
        assert!(dir.to_string_lossy().ends_with("src"));
        assert_eq!(prefix, "mai");
    }

    #[test]
    fn test_split_dir_prefix_no_parent() {
        let (dir, prefix) = split_dir_prefix(Path::new("foo"), Path::new("/project"));
        assert_eq!(dir, PathBuf::from("/project"));
        assert_eq!(prefix, "foo");
    }

    #[test]
    fn test_split_dir_prefix_trailing_slash() {
        let (dir, prefix) = split_dir_prefix(Path::new("src/"), Path::new("/project"));
        let dir_str = dir.to_string_lossy();
        // cwd.join("src/") may preserve trailing slash on some platforms,
        // so strip it before checking.
        assert!(dir_str.trim_end_matches('/').ends_with("src"),
            "dir should end with 'src', got: {}", dir_str);
        assert_eq!(prefix, "");
    }

    #[test]
    fn test_complete_path_runs() {
        // Just verify it doesn't panic
        let cwd = std::env::current_dir().unwrap();
        let results = complete_path("./", &cwd);
        // May or may not have results depending on directory contents
        let _ = results;
    }
}
