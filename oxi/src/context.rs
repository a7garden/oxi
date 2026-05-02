//! Context file loading for oxi
//!
//! Loads project-specific context files (AGENTS.md, CLAUDE.md) that contain
//! instructions and guidelines for the AI assistant. These files are discovered
//! by walking up from the current working directory to the filesystem root,
//! with files closer to the project root taking precedence (loaded last).

use anyhow::Result;
use std::path::{Path, PathBuf};

/// A loaded context file with its path and content.
#[derive(Debug, Clone)]
pub struct ContextFile {
    /// Absolute path to the context file.
    pub path: PathBuf,
    /// File content.
    pub content: String,
}

/// Candidate filenames to search for, in priority order.
const CONTEXT_FILE_NAMES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Load context files from the given working directory and agent config directory.
///
/// Discovery strategy (matching pi-mono's `loadProjectContextFiles`):
///
/// 1. **Global context**: Look in the agent config directory (`~/.oxi/`)
/// 2. **Project context**: Walk from `cwd` upward to the filesystem root,
///    collecting AGENTS.md / CLAUDE.md files from each directory.
///    Files are ordered from root → cwd so that project-level files
///    (closer to cwd) override ancestor files.
///
/// Returns files in the order they should be appended to the system prompt.
pub fn load_context_files(cwd: &Path, agent_dir: &Path) -> Result<Vec<ContextFile>> {
    let mut context_files = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    // 1. Global context from agent dir (e.g., ~/.oxi/AGENTS.md)
    if let Some(ctx) = load_context_file_from_dir(agent_dir) {
        seen_paths.insert(ctx.path.clone());
        context_files.push(ctx);
    }

    // 2. Walk from cwd upward, collecting ancestor context files
    let mut ancestor_files = Vec::new();
    let mut current = cwd.to_path_buf();
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let root = root.ancestors().last().unwrap_or(Path::new("/"));

    loop {
        if let Some(ctx) = load_context_file_from_dir(&current) {
            if !seen_paths.contains(&ctx.path) {
                seen_paths.insert(ctx.path.clone());
                ancestor_files.push(ctx);
            }
        }

        if current == root {
            break;
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }

    // Reverse so that files closer to cwd come last (higher precedence)
    ancestor_files.reverse();
    context_files.extend(ancestor_files);

    Ok(context_files)
}

/// Try to load a context file from a single directory.
///
/// Checks for AGENTS.md, CLAUDE.md (case-insensitive) in order.
fn load_context_file_from_dir(dir: &Path) -> Option<ContextFile> {
    for filename in CONTEXT_FILE_NAMES {
        let path = dir.join(filename);
        if path.exists() && path.is_file() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    return Some(ContextFile {
                        path: path.canonicalize().unwrap_or(path),
                        content,
                    });
                }
                Err(e) => {
                    tracing::warn!("Warning: Could not read {}: {}", path.display(), e);
                }
            }
        }
    }
    None
}

/// Load only the project-level context files (no global).
///
/// Useful for cases where the global agent directory is not relevant.
pub fn load_project_context_files(cwd: &Path) -> Result<Vec<ContextFile>> {
    let mut context_files = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    let mut current = cwd.to_path_buf();
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let root = root.ancestors().last().unwrap_or(Path::new("/"));

    loop {
        if let Some(ctx) = load_context_file_from_dir(&current) {
            if !seen_paths.contains(&ctx.path) {
                seen_paths.insert(ctx.path.clone());
                context_files.push(ctx);
            }
        }

        if current == root {
            break;
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }

    // Reverse: files closer to cwd come last
    context_files.reverse();

    Ok(context_files)
}

/// Format context files for inclusion in the system prompt.
///
/// Produces a section like:
/// ```text
/// # Project Context
///
/// Project-specific instructions and guidelines:
///
/// ## /path/to/AGENTS.md
///
/// (content)
/// ```
pub fn format_context_for_prompt(files: &[ContextFile]) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut prompt = String::from("# Project Context\n\n");
    prompt.push_str("Project-specific instructions and guidelines:\n\n");

    for file in files {
        prompt.push_str(&format!("## {}\n\n{}\n\n", file.path.display(), file.content));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_context_files_from_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let files = load_context_files(tmp.path(), tmp.path()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_load_context_file_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# Agent Rules\nBe helpful.").unwrap();

        let files = load_context_files(tmp.path(), tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Agent Rules"));
        assert!(files[0].path.to_string_lossy().contains("AGENTS.md"));
    }

    #[test]
    fn test_load_context_file_claude_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "# Claude Rules\nUse TypeScript.").unwrap();

        let files = load_context_files(tmp.path(), tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Claude Rules"));
    }

    #[test]
    fn test_agents_md_takes_precedence_over_claude_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "agents content").unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "claude content").unwrap();

        let files = load_context_files(tmp.path(), tmp.path()).unwrap();
        // Only one file per directory (AGENTS.md wins)
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "agents content");
    }

    #[test]
    fn test_global_and_project_context() {
        let global_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();

        std::fs::write(global_dir.path().join("AGENTS.md"), "Global rules").unwrap();
        std::fs::write(project_dir.path().join("AGENTS.md"), "Project rules").unwrap();

        let files = load_context_files(project_dir.path(), global_dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        // Global comes first, project comes second (higher precedence)
        assert!(files[0].content.contains("Global rules"));
        assert!(files[1].content.contains("Project rules"));
    }

    #[test]
    fn test_ancestor_directory_walk() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("a").join("b");
        std::fs::create_dir_all(&child).unwrap();

        // Put AGENTS.md in root and child
        std::fs::write(root.path().join("AGENTS.md"), "Root rules").unwrap();
        std::fs::write(child.join("AGENTS.md"), "Child rules").unwrap();

        let files = load_context_files(&child, root.path()).unwrap();
        // Should find: global (root as agent_dir), then ancestor walk
        // The child's AGENTS.md should be last (highest precedence)
        let child_file = files.last().unwrap();
        assert_eq!(child_file.content, "Child rules");
    }

    #[test]
    fn test_format_context_for_prompt_empty() {
        let result = format_context_for_prompt(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_context_for_prompt() {
        let files = vec![
            ContextFile {
                path: PathBuf::from("/project/AGENTS.md"),
                content: "Be helpful.".to_string(),
            },
        ];

        let result = format_context_for_prompt(&files);
        assert!(result.contains("# Project Context"));
        assert!(result.contains("Be helpful."));
        assert!(result.contains("/project/AGENTS.md"));
    }

    #[test]
    fn test_load_project_context_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "Project specific").unwrap();

        let files = load_project_context_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "Project specific");
    }
}
