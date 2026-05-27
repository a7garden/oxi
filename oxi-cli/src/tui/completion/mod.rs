//! General completion system for the TUI.
//!
//! Extends the existing slash command completion in `slash.rs` with
//! file path completion and fuzzy file search. Uses a unified
//! `CompletionManager` that dispatches to the appropriate provider.

#![allow(dead_code)]

pub mod fuzzy_file;
pub mod path;

// ---------------------------------------------------------------------------
// Completion types
// ---------------------------------------------------------------------------

/// Kind of completion being performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKind {
    /// Slash command completion (existing system).
    SlashCommand,
    /// Slash command argument completion.
    SlashArgument {
        /// The slash command name.
        command: String,
    },
    /// File path completion.
    FilePath,
    /// Fuzzy file search (fd-based).
    FuzzyFile {
        /// The search query.
        query: String,
    },
}

/// A single completion item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// The text to insert.
    pub text: String,
    /// Display label (may differ from text, e.g., with icon).
    pub label: String,
    /// Optional description shown in the completion popup.
    pub description: Option<String>,
    /// The kind of completion.
    pub kind: CompletionKind,
}

impl std::fmt::Display for CompletionItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.description {
            Some(desc) => write!(f, "{} — {}", self.label, desc),
            None => write!(f, "{}", self.label),
        }
    }
}

// ---------------------------------------------------------------------------
// CompletionManager
// ---------------------------------------------------------------------------

/// Manages completion providers and dispatches queries.
///
/// Wraps the existing slash completion system and adds file path
/// and fuzzy file search providers.
pub struct CompletionManager {
    /// Current working directory for file completions.
    cwd: std::path::PathBuf,
}

impl CompletionManager {
    /// Create a new CompletionManager for the given working directory.
    pub fn new(cwd: std::path::PathBuf) -> Self {
        CompletionManager { cwd }
    }

    /// Get completions for the given input text.
    ///
    /// Determines the completion kind from the input prefix and
    /// dispatches to the appropriate provider.
    pub fn get_completions(&self, input: &str) -> Vec<CompletionItem> {
        // File path completion: starts with ./ or ../ or /
        if input.starts_with("./") || input.starts_with("../") || input.starts_with("~") {
            return path::complete_path(input, &self.cwd);
        }

        // If input contains a path separator, try file completion
        if input.contains('/') && !input.starts_with('/') {
            return path::complete_path(input, &self.cwd);
        }

        // No completions for empty input or non-path text
        Vec::new()
    }

    /// Get fuzzy file search results.
    ///
    /// Uses `fd` for fast file search with scoring.
    pub async fn fuzzy_search(&self, query: &str) -> Vec<CompletionItem> {
        fuzzy_file::fuzzy_file_search(query, &self.cwd).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_item_display() {
        let item = CompletionItem {
            text: "src/main.rs".to_string(),
            label: "src/main.rs".to_string(),
            description: Some("Rust source file".to_string()),
            kind: CompletionKind::FilePath,
        };
        assert_eq!(format!("{}", item), "src/main.rs — Rust source file");
    }

    #[test]
    fn test_completion_item_no_desc() {
        let item = CompletionItem {
            text: "/model".to_string(),
            label: "/model".to_string(),
            description: None,
            kind: CompletionKind::SlashCommand,
        };
        assert_eq!(format!("{}", item), "/model");
    }

    #[test]
    fn test_manager_path_completion() {
        let mgr = CompletionManager::new(std::env::current_dir().unwrap());
        let results = mgr.get_completions("./");
        // Should find at least some files in the current directory
        // (may be empty in some environments, so just verify it doesn't panic)
        let _ = results;
    }

    #[test]
    fn test_manager_no_completion_for_plain_text() {
        let mgr = CompletionManager::new(std::env::current_dir().unwrap());
        let results = mgr.get_completions("hello world");
        assert!(results.is_empty());
    }
}
