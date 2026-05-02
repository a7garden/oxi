//! FileCompleter - provides file path autocompletion.
//!
//! This module provides intelligent file path completion with support
//! for tilde expansion, relative paths, and directory traversal.

use std::fs;
use std::path::{Path, PathBuf};
use crate::autocomplete::FuzzyMatcher;

/// A completion suggestion with metadata.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The text to insert.
    pub text: String,
    /// Display text (may include indicators).
    pub display: String,
    /// Whether this is a directory (for display purposes).
    pub is_dir: bool,
    /// Score for ranking (higher = better).
    pub score: usize,
}

impl Completion {
    /// Create a new completion.
    pub fn new(text: String, is_dir: bool, score: usize) -> Self {
        let display = if is_dir {
            format!("{}/", text)
        } else {
            text.clone()
        };
        Self { text, display, is_dir, score }
    }

    /// Create a file completion.
    pub fn file(text: String, score: usize) -> Self {
        Self::new(text, false, score)
    }

    /// Create a directory completion.
    pub fn dir(text: String, score: usize) -> Self {
        Self::new(text, true, score)
    }
}

/// FileCompleter provides file path autocompletion.
#[derive(Debug, Clone)]
pub struct FileCompleter {
    /// Base path for relative completions.
    base_path: PathBuf,
    /// Fuzzy matcher instance.
    matcher: FuzzyMatcher,
    /// Maximum results to return.
    max_results: usize,
}

impl FileCompleter {
    /// Create a new FileCompleter with the given base path.
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            matcher: FuzzyMatcher::new(),
            max_results: 20,
        }
    }

    /// Get completions for a prefix.
    /// Handles tilde expansion, relative paths, and absolute paths.
    pub fn completions(&self, prefix: &str) -> Vec<Completion> {
        if prefix.is_empty() {
            // Return current directory contents
            return self.list_directory(&PathBuf::from("."), "", 100);
        }

        // Resolve the prefix to a directory and pattern
        let (dir_pattern, base_dir) = self.parse_prefix(prefix);
        
        // List directory and filter by pattern
        let candidates = self.list_directory(&base_dir, &dir_pattern, self.max_results * 2);
        
        // Apply fuzzy matching if pattern has multiple characters
        if dir_pattern.len() > 1 {
            let mut results: Vec<Completion> = candidates
                .into_iter()
                .filter_map(|c| {
                    let score = self.matcher.matches(&dir_pattern, &c.text)?;
                    Some(Completion {
                        text: c.text,
                        display: c.display,
                        is_dir: c.is_dir,
                        score,
                    })
                })
                .collect();
            
            results.sort_by(|a, b| b.score.cmp(&a.score));
            results.truncate(self.max_results);
            results
        } else {
            // Just use prefix matching for single character
            let filtered: Vec<Completion> = candidates
                .into_iter()
                .filter(|c| c.text.to_lowercase().starts_with(&dir_pattern.to_lowercase()))
                .take(self.max_results)
                .collect();
            filtered
        }
    }

    /// Parse a prefix into directory and filename pattern.
    fn parse_prefix(&self, prefix: &str) -> (String, PathBuf) {
        // Handle tilde expansion
        if prefix.starts_with('~') {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let remainder = &prefix[1..];
            
            if remainder.is_empty() || remainder.starts_with('/') {
                // ~ or ~/...
                let path = PathBuf::from(&home);
                let pattern = if remainder.is_empty() {
                    String::new()
                } else {
                    remainder[1..].to_string()
                };
                return (pattern, path);
            }
            return (remainder.to_string(), PathBuf::from(home));
        }

        // Handle absolute paths
        if prefix.starts_with('/') {
            let parts: Vec<&str> = prefix.split('/').collect();
            if parts.len() == 1 {
                return (prefix[1..].to_string(), PathBuf::from("/"));
            }
            let pattern = parts.last().unwrap_or(&"").to_string();
            let dir = parts[..parts.len() - 1].join("/");
            return (pattern, PathBuf::from(if dir.is_empty() { "/" } else { &dir }));
        }

        // Handle ./ and ../
        if prefix.starts_with("./") || prefix.starts_with("../") {
            let (pattern, rel_path) = if prefix.starts_with("./") {
                (&prefix[2..], PathBuf::from("."))
            } else {
                (&prefix[3..], self.base_path.clone())
            };

            // Navigate up for ../
            if prefix.starts_with("../") {
                if let Some(parent) = rel_path.parent() {
                    return (pattern.to_string(), parent.to_path_buf());
                }
            }
            return (pattern.to_string(), rel_path);
        }

        // Relative path
        if let Some(last_slash) = prefix.rfind('/') {
            let pattern = prefix[last_slash + 1..].to_string();
            let dir = &prefix[..last_slash];
            let base_dir = if dir.is_empty() {
                self.base_path.clone()
            } else {
                self.base_path.join(dir)
            };
            (pattern, base_dir)
        } else {
            // No directory component - search in base_path
            (prefix.to_string(), self.base_path.clone())
        }
    }

    /// List directory contents matching a pattern.
    fn list_directory(&self, dir: &Path, pattern: &str, max: usize) -> Vec<Completion> {
        let mut results = Vec::new();
        
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return results,
        };

        for entry in entries.flatten() {
            if results.len() >= max {
                break;
            }

            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            
            // Filter by pattern
            if !pattern.is_empty() && !name.to_lowercase().starts_with(&pattern.to_lowercase()) {
                continue;
            }

            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            
            // Skip hidden files unless pattern starts with .
            if name.starts_with('.') && !pattern.starts_with('.') {
                continue;
            }

            let score = if is_dir { 5 } else { 0 };
            results.push(Completion::new(name.to_string(), is_dir, score));
        }

        // Sort: directories first, then alphabetically
        results.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.text.to_lowercase().cmp(&b.text.to_lowercase()),
            }
        });

        results
    }

    /// Set the maximum number of results.
    pub fn with_max_results(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Get a single best completion for a prefix.
    pub fn complete(&self, prefix: &str) -> Option<String> {
        let completions = self.completions(prefix);
        completions.first().map(|c| c.text.clone())
    }

    /// Expand a path prefix to full path.
    /// Returns the expanded path if it's a complete match.
    pub fn expand(&self, prefix: &str) -> Option<PathBuf> {
        let completions = self.completions(prefix);
        
        // Find a completion that exactly matches the remaining text
        for completion in completions {
            // Reconstruct the full path
            let dir_pattern = if let Some(last_slash) = prefix.rfind('/') {
                &prefix[..last_slash + 1]
            } else {
                ""
            };
            
            let full = format!("{}{}", dir_pattern, completion.text);
            if full.ends_with('/') || !completion.is_dir {
                return Some(self.base_path.join(&full));
            }
        }
        
        None
    }
}

impl Default for FileCompleter {
    fn default() -> Self {
        Self::new(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_creation() {
        let c = Completion::file("test.rs".to_string(), 10);
        assert_eq!(c.text, "test.rs");
        assert!(!c.is_dir);
        assert_eq!(c.display, "test.rs");
    }

    #[test]
    fn test_dir_completion() {
        let c = Completion::dir("src".to_string(), 5);
        assert_eq!(c.text, "src");
        assert!(c.is_dir);
        assert_eq!(c.display, "src/");
    }

    #[test]
    fn test_file_completer_default() {
        let completer = FileCompleter::default();
        assert_eq!(completer.max_results, 20);
    }

    #[test]
    fn test_file_completer_with_max_results() {
        let completer = FileCompleter::new(".").with_max_results(10);
        assert_eq!(completer.max_results, 10);
    }
}