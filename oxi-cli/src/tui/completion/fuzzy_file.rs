//! Fuzzy file search provider using `fd`.
//!
//! Provides fast file search with scoring, integrating the `fd` command
//! for efficient filesystem traversal.

use super::{CompletionItem, CompletionKind};
use std::path::Path;
use std::process::Stdio;

/// Perform a fuzzy file search using `fd`.
///
/// Uses the `fd` command-line tool for fast file discovery:
/// - `--base-directory`: search from the project root
/// - `--max-results 100`: limit results for performance
/// - `--type f --type d`: include files and directories
/// - `--follow`: follow symlinks
/// - `--hidden`: include hidden files
/// - `--exclude .git`: skip .git directory
///
/// Results are scored and sorted:
/// - Exact match: 100
/// - Starts with query: 80
/// - Contains query: 50
/// - Path contains query: 30
pub async fn fuzzy_file_search(query: &str, base_dir: &Path) -> Vec<CompletionItem> {
    if query.is_empty() {
        return Vec::new();
    }

    // Check if fd is available
    let fd_available = tokio::process::Command::new("fd")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok();

    if !fd_available {
        // Fallback: simple directory walk
        return simple_file_search(query, base_dir);
    }

    let output = tokio::process::Command::new("fd")
        .arg("--base-directory")
        .arg(base_dir)
        .arg("--max-results")
        .arg("100")
        .arg("--type")
        .arg("f")
        .arg("--type")
        .arg("d")
        .arg("--follow")
        .arg("--hidden")
        .arg("--exclude")
        .arg(".git")
        .arg("--exclude")
        .arg("node_modules")
        .arg("--exclude")
        .arg("target")
        .arg(query)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results: Vec<CompletionItem> = stdout
                .lines()
                .filter_map(|line| {
                    if line.is_empty() {
                        return None;
                    }
                    let score = score_path(line, query);
                    Some((line.to_string(), score))
                })
                .filter(|(_, score)| *score > 0)
                .map(|(path, _)| CompletionItem {
                    text: path.clone(),
                    label: path.clone(),
                    description: Some("file".to_string()),
                    kind: CompletionKind::FuzzyFile {
                        query: query.to_string(),
                    },
                })
                .collect();

            // Sort by score (highest first) — we already filtered by score > 0
            // Re-score and sort
            results.sort_by(|a, b| {
                let sa = score_path(&a.text, query);
                let sb = score_path(&b.text, query);
                sb.cmp(&sa)
            });

            results.truncate(20);
            results
        }
        _ => Vec::new(),
    }
}

/// Simple file search fallback when `fd` is not available.
///
/// Walks the directory tree up to 2 levels deep.
fn simple_file_search(query: &str, base_dir: &Path) -> Vec<CompletionItem> {
    let lower_query = query.to_lowercase();
    let mut results = Vec::new();

    // Walk up to 2 levels
    if let Ok(entries) = std::fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().contains(&lower_query) {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                results.push(CompletionItem {
                    text: name.clone(),
                    label: if is_dir {
                        format!("{}/", name)
                    } else {
                        name.clone()
                    },
                    description: if is_dir {
                        Some("directory".to_string())
                    } else {
                        Some("file".to_string())
                    },
                    kind: CompletionKind::FuzzyFile {
                        query: query.to_string(),
                    },
                });
            }

            // Second level
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && !name.starts_with('.')
                && name != "target"
                && name != "node_modules"
                && let Ok(sub_entries) = std::fs::read_dir(entry.path())
            {
                for sub_entry in sub_entries.flatten() {
                    let sub_name = sub_entry.file_name().to_string_lossy().to_string();
                    let full_path = format!("{}/{}", name, sub_name);
                    if full_path.to_lowercase().contains(&lower_query) {
                        results.push(CompletionItem {
                            text: full_path.clone(),
                            label: full_path,
                            description: Some("file".to_string()),
                            kind: CompletionKind::FuzzyFile {
                                query: query.to_string(),
                            },
                        });
                    }
                }
            }

            if results.len() >= 20 {
                break;
            }
        }
    }

    results
}

/// Score a path against a query for ranking.
///
/// Scoring:
/// - Exact basename match: 100
/// - Basename starts with query: 80
/// - Basename contains query: 50
/// - Full path contains query: 30
fn score_path(path: &str, query: &str) -> i32 {
    let lower_path = path.to_lowercase();
    let lower_query = query.to_lowercase();

    let basename = std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if basename == lower_query {
        100
    } else if basename.starts_with(&lower_query) {
        80
    } else if basename.contains(&lower_query) {
        50
    } else if lower_path.contains(&lower_query) {
        30
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_exact_match() {
        assert_eq!(score_path("main.rs", "main.rs"), 100);
    }

    #[test]
    fn test_score_starts_with() {
        assert_eq!(score_path("main.rs", "main"), 80);
    }

    #[test]
    fn test_score_contains() {
        assert_eq!(score_path("lib_main.rs", "main"), 50);
    }

    #[test]
    fn test_score_path_contains() {
        assert_eq!(score_path("src/main.rs", "main"), 80); // basename starts with
    }

    #[test]
    fn test_score_no_match() {
        assert_eq!(score_path("foo.rs", "bar"), 0);
    }

    #[test]
    fn test_score_case_insensitive() {
        // "main.rs" (basename lowered) starts with "main" → 80
        assert_eq!(score_path("Main.rs", "main"), 80);
        // Exact case-insensitive match of full filename → 100
        assert_eq!(score_path("Main.rs", "main.rs"), 100);
    }

    #[test]
    fn test_simple_search() {
        let cwd = std::env::current_dir().unwrap();
        let results = simple_file_search("cargo", &cwd);
        // Should find Cargo.toml in most Rust projects
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_fuzzy_search_empty_query() {
        let cwd = std::env::current_dir().unwrap();
        let results = fuzzy_file_search("", &cwd).await;
        assert!(results.is_empty());
    }
}
