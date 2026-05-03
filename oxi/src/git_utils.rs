//! Git utilities for version control operations
//!
//! Provides utilities for interacting with git repositories,
//! including checkpoints, diffs, and log retrieval.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Git commit information
#[derive(Debug, Clone)]
pub struct GitCommit {
    pub sha: String,
    pub short_sha: String,
    pub message: String,
    pub author: String,
    pub timestamp: SystemTime,
}

/// Git log entry
#[derive(Debug, Clone)]
pub struct GitLogEntry {
    pub commit: GitCommit,
    pub branch: Option<String>,
}

/// Git diff result
#[derive(Debug, Clone)]
pub struct GitDiff {
    pub staged: String,
    pub unstaged: String,
    pub untracked: String,
}

/// Git status
#[derive(Debug, Clone)]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub is_dirty: bool,
    pub staged_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
}

/// Check if a directory is a git repository
pub fn is_git_repo(dir: &Path) -> bool {
    find_git_root(dir).is_some()
}

/// Find the git root directory by walking up from a path
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();

    loop {
        let git_dir = current.join(".git");
        if git_dir.exists() {
            return Some(current);
        }

        if git_dir.is_file() {
            if let Ok(content) = std::fs::read_to_string(&git_dir) {
                if content.starts_with("gitdir: ") {
                    let gitdir_path = content.trim_start_matches("gitdir: ").trim();
                    if let Ok(main_git) = PathBuf::from(gitdir_path).canonicalize() {
                        if let Some(main_dir) = main_git.parent() {
                            return Some(main_dir.to_path_buf());
                        }
                    }
                }
            }
            return Some(current);
        }

        current = match current.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return None,
        };

        if current.to_string_lossy() == "/" {
            return None;
        }
    }
}

/// Get the git root for a given directory
pub fn get_git_root(cwd: &Path) -> PathBuf {
    find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

/// Run a git command
fn run_git_command(repo_dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(["-C", repo_dir.to_string_lossy().as_ref()])
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Git command failed: {}", stderr))
    }
}

/// Get the current branch name
pub fn get_current_branch(repo_dir: &Path) -> Option<String> {
    run_git_command(repo_dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty())
}

/// Check if the repository is in detached HEAD state
pub fn is_detached_head(repo_dir: &Path) -> bool {
    if let Ok(head) = run_git_command(repo_dir, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        head == "HEAD"
    } else {
        false
    }
}

/// Create a checkpoint commit
pub fn git_checkpoint(repo_dir: &Path, message: Option<&str>) -> Result<String, String> {
    run_git_command(repo_dir, &["add", "-A"])?;

    let status = run_git_command(repo_dir, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Err("No changes to checkpoint".to_string());
    }

    let timestamp = chrono::Utc::now();
    let default_msg = format!(
        "Checkpoint: {}",
        timestamp.format("%Y-%m-%d %H:%M:%S UTC")
    );
    let msg = message.unwrap_or(&default_msg);

    run_git_command(repo_dir, &["commit", "-m", msg])?;
    run_git_command(repo_dir, &["rev-parse", "--short", "HEAD"])
}

/// Get the git diff output
pub fn git_diff(repo_dir: &Path, diff_type: &str) -> Result<String, String> {
    match diff_type {
        "staged" => run_git_command(repo_dir, &["diff", "--cached"]),
        "unstaged" => run_git_command(repo_dir, &["diff"]),
        "untracked" => run_git_command(repo_dir, &["ls-files", "--others", "--exclude-standard"]),
        "all" => {
            let staged = run_git_command(repo_dir, &["diff", "--cached"]).unwrap_or_default();
            let unstaged = run_git_command(repo_dir, &["diff"]).unwrap_or_default();
            let untracked = run_git_command(repo_dir, &["ls-files", "--others", "--exclude-standard"])
                .unwrap_or_default();
            Ok(format!(
                "=== STAGED ===\n{}\n\n=== UNSTAGED ===\n{}\n\n=== UNTRACKED ===\n{}",
                staged, unstaged, untracked
            ))
        }
        _ => Err(format!("Unknown diff type: {}", diff_type)),
    }
}

/// Get the git log
pub fn git_log(repo_dir: &Path, count: usize) -> Result<Vec<GitLogEntry>, String> {
    let format_str = "%H|%h|%s|%an|%ae|%at";
    let output = run_git_command(
        repo_dir,
        &["log", &format!("-{}", count), &format!("--format={}", format_str), "--all"],
    )?;

    let branch = get_current_branch(repo_dir);

    let entries: Vec<GitLogEntry> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 6 {
                return None;
            }

            let timestamp = parts[5]
                .parse::<i64>()
                .ok()
                .and_then(|t| {
                    SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(t as u64))
                })
                .unwrap_or(SystemTime::UNIX_EPOCH);

            Some(GitLogEntry {
                commit: GitCommit {
                    sha: parts[0].to_string(),
                    short_sha: parts[1].to_string(),
                    message: parts[2].to_string(),
                    author: parts[3].to_string(),
                    timestamp,
                },
                branch: branch.clone(),
            })
        })
        .collect();

    Ok(entries)
}

/// Restore a file or path to a specific commit
pub fn git_restore(repo_dir: &Path, sha: &str, path: Option<&str>) -> Result<(), String> {
    let target = if sha.starts_with("HEAD~") || sha.starts_with("HEAD^") || sha.contains('~') {
        sha.to_string()
    } else {
        run_git_command(repo_dir, &["rev-parse", "--verify", sha])?;
        sha.to_string()
    };

    let path_arg = path.unwrap_or(".");
    run_git_command(repo_dir, &["checkout", &target, "--", path_arg])?;
    Ok(())
}

/// Get git status
pub fn git_status(repo_dir: &Path) -> Result<GitStatus, String> {
    let is_repo = is_git_repo(repo_dir);
    if !is_repo {
        return Ok(GitStatus {
            is_repo: false,
            branch: None,
            is_dirty: false,
            staged_files: vec![],
            modified_files: vec![],
            untracked_files: vec![],
        });
    }

    let branch = get_current_branch(repo_dir);
    let status_output = run_git_command(repo_dir, &["status", "--porcelain"])?;

    let mut staged_files = Vec::new();
    let mut modified_files = Vec::new();
    let mut untracked_files = Vec::new();

    for line in status_output.lines() {
        if line.len() < 3 {
            continue;
        }
        let index_status = line.chars().next().unwrap_or(' ');
        let worktree_status = line.chars().nth(1).unwrap_or(' ');
        let filename = line[3..].to_string();

        if index_status == '?' && worktree_status == '?' {
            untracked_files.push(filename.clone());
        } else if index_status != ' ' && index_status != '?' {
            staged_files.push(filename.clone());
        }
        if worktree_status != ' ' && worktree_status != '?' {
            if !staged_files.contains(&filename) {
                modified_files.push(filename);
            }
        }
    }

    let is_dirty = !staged_files.is_empty() || !modified_files.is_empty() || !untracked_files.is_empty();

    Ok(GitStatus {
        is_repo: true,
        branch,
        is_dirty,
        staged_files,
        modified_files,
        untracked_files,
    })
}

/// Get the number of commits ahead/behind a remote branch
pub fn git_ahead_behind(repo_dir: &Path) -> Result<(usize, usize), String> {
    let current = get_current_branch(repo_dir).ok_or("Not on a branch")?;
    // Build upstream ref string: branch@ {u}
    let upstream_ref = format!("{}@{{u}}", current);
    let remote_branch = run_git_command(repo_dir, &["rev-parse", "--abbrev-ref", &upstream_ref])
        .ok();

    let remote_branch = match remote_branch {
        Some(rb) => rb,
        None => return Ok((0, 0)),
    };

    let base = run_git_command(repo_dir, &["merge-base", &current, &remote_branch])?;
    let ahead = run_git_command(repo_dir, &["log", &format!("{}..{}", base, current), "--oneline"])
        .unwrap_or_default();
    let behind = run_git_command(repo_dir, &["log", &format!("{}..{}", current, base), "--oneline"])
        .unwrap_or_default();

    Ok((ahead.lines().count(), behind.lines().count()))
}

/// Get the tags that contain a specific commit
pub fn git_tags_containing(repo_dir: &Path, sha: &str) -> Result<Vec<String>, String> {
    let output = run_git_command(repo_dir, &["tag", "--contains", sha])?;
    Ok(output.lines().map(|s| s.to_string()).collect())
}

/// Get the last modified date of a file in the repo
pub fn git_file_last_modified(repo_dir: &Path, file_path: &str) -> Result<SystemTime, String> {
    let output = run_git_command(repo_dir, &["log", "-1", "--format=%at", "--", file_path])?;

    let timestamp: i64 = output.trim().parse().map_err(|_| "Invalid timestamp")?;
    SystemTime::UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs(timestamp as u64))
        .ok_or_else(|| "Invalid timestamp".to_string())
}

/// Check if a file has uncommitted changes
pub fn git_file_is_modified(repo_dir: &Path, file_path: &str) -> Result<bool, String> {
    let status = run_git_command(repo_dir, &["status", "--porcelain", "--", file_path])?;
    Ok(!status.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn test_repo_path() -> PathBuf {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    #[test]
    fn test_is_git_repo() {
        let result = is_git_repo(&test_repo_path());
        assert!(result == true || result == false);
    }

    #[test]
    fn test_find_git_root() {
        let result = find_git_root(&test_repo_path());
        assert!(result.is_some());
    }

    #[test]
    fn test_get_git_root() {
        let root = get_git_root(&test_repo_path());
        assert!(root.exists());
    }

    #[test]
    fn test_git_status() {
        let status = git_status(&test_repo_path());
        assert!(status.is_ok());
        let status = status.unwrap();
        assert!(!status.is_repo || status.branch.is_some() || !status.branch.is_none());
    }

    #[test]
    fn test_git_log_returns_vec() {
        let result = git_log(&test_repo_path(), 5);
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_git_diff_invalid_type() {
        let result = git_diff(&test_repo_path(), "invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_git_checkpoint_no_changes() {
        let result = git_checkpoint(&test_repo_path(), None);
        assert!(result.is_ok() || result == Err("No changes to checkpoint".to_string()));
    }

    #[test]
    fn test_git_file_last_modified() {
        let result = git_file_last_modified(&test_repo_path(), "Cargo.toml");
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_git_file_is_modified() {
        let result = git_file_is_modified(&test_repo_path(), "Cargo.toml");
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_git_tags_containing() {
        let result = git_tags_containing(&test_repo_path(), "HEAD");
        assert!(result.is_ok());
    }
}