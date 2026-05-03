//! Git worktree management skill for oxi
//!
//! Provides typed Rust abstractions over `git worktree` operations:
//!
//! - **Create** worktrees for parallel feature/hotfix branches
//! - **List** existing worktrees with branch, commit, and status info
//! - **Merge** a worktree's branch back into a target branch
//! - **Clean up** (remove) worktrees that are no longer needed
//! - **Branch** — create a new branch + worktree in one step
//!
//! All operations are carried out by spawning `git` subprocesses — this
//! module does not link against `libgit2`.
//!
//! # Example
//!
//! ```rust,no_run
//! use oxi::skills::worktree::{WorktreeManager, WorktreeCreateOpts};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let mgr = WorktreeManager::for_current_repo()?;
//!
//!     // Create a worktree for a feature branch
//!     let opts = WorktreeCreateOpts::feature_branch("feat/auth", &std::path::PathBuf::from("."));
//!     let wt = mgr.create(&opts).await?;
//!     println!("Created worktree at {}", wt.path.display());
//!
//!     // List all worktrees
//!     let list = mgr.list().await?;
//!     for wt in &list.worktrees {
//!         println!("{} [{}] at {}", wt.branch(), wt.commit_short(), wt.path.display());
//!     }
//!
//!     // Merge and remove
//!     mgr.merge_and_remove("feat/auth", "main").await?;
//!     Ok(())
//! }
//! ```

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::process::Command;

// ── Worktree info ─────────────────────────────────────────────────────

/// Information about a single git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Branch name (detached HEAD → `None`).
    pub branch: Option<String>,
    /// Full commit SHA.
    pub commit: String,
    /// Whether this is the main/primary worktree.
    pub is_main: bool,
    /// Whether this worktree is bare.
    pub is_bare: bool,
    /// Whether the worktree has a detached HEAD.
    pub is_detached: bool,
    /// Whether the worktree is prunable (directory missing/deleted).
    pub is_prunable: bool,
    /// Whether the working directory is locked.
    pub is_locked: bool,
}

impl WorktreeInfo {
    /// Short (7-char) commit hash.
    pub fn commit_short(&self) -> String {
        self.commit.chars().take(7).collect()
    }

    /// Branch name, falling back to the short commit for detached HEAD.
    pub fn branch(&self) -> String {
        self.branch
            .clone()
            .unwrap_or_else(|| format!("(detached {})", self.commit_short()))
    }

    /// Human-readable label: the directory name of the worktree.
    pub fn dir_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

impl fmt::Display for WorktreeInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let main_flag = if self.is_main { " [main]" } else { "" };
        let locked_flag = if self.is_locked { " [locked]" } else { "" };
        let prunable_flag = if self.is_prunable { " [prunable]" } else { "" };
        write!(
            f,
            "{} {} at {}{}{}{}",
            self.branch(),
            self.commit_short(),
            self.path.display(),
            main_flag,
            locked_flag,
            prunable_flag,
        )
    }
}

// ── Create options ────────────────────────────────────────────────────

/// Options for creating a new worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeCreateOpts {
    /// Branch name to create (or check out if it already exists).
    pub branch: String,
    /// Target path for the new worktree.
    /// Can be absolute or relative to the repository root.
    pub path: PathBuf,
    /// Starting point for the new branch (default: HEAD).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_point: Option<String>,
    /// If `true`, create a new branch even if one with the same name exists
    /// (forces `-B` instead of `-b`). Default: `false`.
    #[serde(default)]
    pub force_branch: bool,
    /// If `true`, detach HEAD in the new worktree instead of creating a branch.
    /// Default: `false`.
    #[serde(default)]
    pub detach: bool,
}

impl WorktreeCreateOpts {
    /// Build options for a simple feature-branch worktree.
    ///
    /// The path is derived by appending the branch name (with `/` → `-`) as a
    /// sibling directory next to the repo root.
    pub fn feature_branch(branch: &str, repo_root: &Path) -> Self {
        let safe_name = branch.replace('/', "-");
        let parent = repo_root.parent().unwrap_or(repo_root);
        let path = parent.join(safe_name);
        Self {
            branch: branch.to_string(),
            path,
            start_point: None,
            force_branch: false,
            detach: false,
        }
    }

    /// Build options for a hotfix worktree based on a specific commit/tag.
    pub fn hotfix(branch: &str, start_point: &str, repo_root: &Path) -> Self {
        let safe_name = branch.replace('/', "-");
        let parent = repo_root.parent().unwrap_or(repo_root);
        let path = parent.join(safe_name);
        Self {
            branch: branch.to_string(),
            path,
            start_point: Some(start_point.to_string()),
            force_branch: false,
            detach: false,
        }
    }
}

// ── Merge result ──────────────────────────────────────────────────────

/// Outcome of merging a worktree's branch into a target branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    /// Branch that was merged (source).
    pub source_branch: String,
    /// Branch that received the merge (target).
    pub target_branch: String,
    /// Whether the merge produced a merge commit (vs fast-forward).
    pub was_merge_commit: bool,
    /// Merge commit SHA (if a merge commit was created).
    pub merge_commit: Option<String>,
    /// Whether there were conflicts during the merge.
    pub had_conflicts: bool,
    /// Conflict details (file paths) if `had_conflicts` is true.
    pub conflicts: Vec<String>,
}

// ── List result ───────────────────────────────────────────────────────

/// Summary of all worktrees in a repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeList {
    /// All worktrees found.
    pub worktrees: Vec<WorktreeInfo>,
    /// Path to the main repository.
    pub repo_root: PathBuf,
}

impl WorktreeList {
    /// Number of worktrees.
    pub fn len(&self) -> usize {
        self.worktrees.len()
    }

    /// Whether there are no worktrees (should always have at least the main one).
    pub fn is_empty(&self) -> bool {
        self.worktrees.is_empty()
    }

    /// The main (primary) worktree.
    pub fn main(&self) -> Option<&WorktreeInfo> {
        self.worktrees.iter().find(|w| w.is_main)
    }

    /// Non-main worktrees (feature/hotfix worktrees).
    pub fn linked(&self) -> Vec<&WorktreeInfo> {
        self.worktrees.iter().filter(|w| !w.is_main).collect()
    }

    /// Find a worktree by branch name.
    pub fn by_branch(&self, branch: &str) -> Option<&WorktreeInfo> {
        self.worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch))
    }

    /// Find a worktree by its directory path.
    pub fn by_path(&self, path: &Path) -> Option<&WorktreeInfo> {
        self.worktrees.iter().find(|w| w.path == path)
    }
}

// ── Worktree manager ─────────────────────────────────────────────────

/// Manages git worktree operations for a repository.
///
/// All commands are run via the `git` CLI in async subprocesses.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    /// Root directory of the git repository (the `.git` location).
    repo_root: PathBuf,
}

impl WorktreeManager {
    /// Create a manager for the repository at `repo_root`.
    ///
    /// Verifies that the directory is inside a git repository.
    pub fn new(repo_root: &Path) -> Result<Self> {
        // Canonicalize to resolve symlinks
        let canonical = repo_root
            .canonicalize()
            .with_context(|| format!("Cannot resolve path: {}", repo_root.display()))?;

        Ok(Self {
            repo_root: canonical,
        })
    }

    /// Create a manager by auto-detecting the git repo from the current
    /// directory (or any directory).
    pub fn for_current_repo() -> Result<Self> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Failed to run `git rev-parse`. Is git installed?")?;

        if !output.status.success() {
            bail!(
                "Not inside a git repository: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        Self::new(&root)
    }

    /// The repository root path.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    // ── Create ────────────────────────────────────────────────────────

    /// Create a new worktree with the given options.
    pub async fn create(&self, opts: &WorktreeCreateOpts) -> Result<WorktreeInfo> {
        let mut args = vec!["worktree".to_string(), "add".to_string()];

        if opts.detach {
            args.push("--detach".to_string());
        } else if opts.force_branch {
            args.push("-B".to_string());
            args.push(opts.branch.clone());
        } else {
            args.push("-b".to_string());
            args.push(opts.branch.clone());
        }

        args.push(opts.path.to_string_lossy().to_string());

        if let Some(ref sp) = opts.start_point {
            args.push(sp.clone());
        }

        let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.run_git(&str_args).await?;

        // Return the created worktree info
        let path = if opts.path.is_absolute() {
            opts.path.clone()
        } else {
            self.repo_root.join(&opts.path)
        };

        let canonical = path
            .canonicalize()
            .context("Worktree path was created but cannot be resolved")?;

        // Find it in the list
        let list = self.list().await?;
        list.by_path(&canonical)
            .cloned()
            .context("Worktree was created but not found in listing")
    }

    /// Create a worktree at `target_path` that checks out `branch`.
    ///
    /// If `branch` doesn't exist yet, it is created from `start_point` (or HEAD).
    pub async fn create_worktree(
        &self,
        branch: &str,
        target_path: &Path,
        start_point: Option<&str>,
    ) -> Result<WorktreeInfo> {
        let opts = WorktreeCreateOpts {
            branch: branch.to_string(),
            path: target_path.to_path_buf(),
            start_point: start_point.map(|s| s.to_string()),
            force_branch: false,
            detach: false,
        };
        self.create(&opts).await
    }

    // ── List ──────────────────────────────────────────────────────────

    /// List all worktrees in the repository.
    ///
    /// Uses `git worktree list --porcelain` for machine-readable output.
    pub async fn list(&self) -> Result<WorktreeList> {
        let output = self
            .run_git_output(&["worktree", "list", "--porcelain"])
            .await?;

        let worktrees = Self::parse_worktree_list(&output)?;

        Ok(WorktreeList {
            worktrees,
            repo_root: self.repo_root.clone(),
        })
    }

    // ── Remove / Prune ────────────────────────────────────────────────

    /// Remove a worktree by its path.
    ///
    /// Fails if the worktree has uncommitted changes unless `force` is `true`.
    pub async fn remove(&self, path: &Path, force: bool) -> Result<()> {
        let mut args = vec!["worktree".to_string(), "remove".to_string()];
        if force {
            args.push("--force".to_string());
        }
        args.push(path.to_string_lossy().to_string());

        let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.run_git(&str_args).await
    }

    /// Remove a worktree by branch name.
    ///
    /// Finds the worktree with the given branch and removes it.
    pub async fn remove_by_branch(&self, branch: &str, force: bool) -> Result<()> {
        let list = self.list().await?;
        let wt = list
            .by_branch(branch)
            .with_context(|| format!("No worktree found for branch '{}'", branch))?;
        self.remove(&wt.path, force).await
    }

    /// Prune deleted worktrees (clean up stale administrative files).
    pub async fn prune(&self) -> Result<()> {
        self.run_git(&["worktree", "prune"]).await
    }

    /// Remove a worktree and delete its branch.
    pub async fn remove_and_delete_branch(&self, branch: &str, force: bool) -> Result<()> {
        self.remove_by_branch(branch, force).await?;
        self.run_git(&["branch", "-d", branch]).await?;
        Ok(())
    }

    /// Remove a worktree and force-delete its branch (even if unmerged).
    pub async fn force_remove_and_delete_branch(&self, branch: &str) -> Result<()> {
        self.remove_by_branch(branch, true).await?;
        self.run_git(&["branch", "-D", branch]).await?;
        Ok(())
    }

    // ── Merge ─────────────────────────────────────────────────────────

    /// Merge a worktree's branch into a target branch.
    ///
    /// This checks out `target_branch` in the main worktree, merges
    /// `source_branch`, and returns the result.
    pub async fn merge(&self, source_branch: &str, target_branch: &str) -> Result<MergeResult> {
        // Checkout target branch in the main worktree
        self.run_git(&["checkout", target_branch]).await?;

        // Perform the merge
        let output = self
            .run_git_output(&["merge", source_branch, "--no-edit"])
            .await?;

        // Check for conflicts
        let had_conflicts = output.contains("CONFLICT") || output.contains("Merge conflict");

        let conflicts = if had_conflicts {
            Self::extract_conflicts(&output)
        } else {
            Vec::new()
        };

        // Determine if it was a merge commit vs fast-forward
        let was_merge_commit = output.contains("Merge made by")
            || output.contains("Merge:") // log line for merge commits
            || !output.contains("Fast-forward");

        // Get the merge commit SHA if applicable
        let merge_commit = if was_merge_commit {
            let rev_output = self.run_git_output(&["rev-parse", "HEAD"]).await?;
            Some(rev_output.trim().to_string())
        } else {
            None
        };

        Ok(MergeResult {
            source_branch: source_branch.to_string(),
            target_branch: target_branch.to_string(),
            was_merge_commit,
            merge_commit,
            had_conflicts,
            conflicts,
        })
    }

    /// Merge a worktree's branch into `target_branch`, then remove the
    /// worktree and delete the source branch.
    ///
    /// On merge conflict, the worktree is NOT removed — the caller must
    /// resolve conflicts and then call `remove_and_delete_branch`.
    pub async fn merge_and_remove(
        &self,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<MergeResult> {
        let result = self.merge(source_branch, target_branch).await?;

        if result.had_conflicts {
            // Don't clean up — the user needs to resolve conflicts first
            return Ok(result);
        }

        // Remove the worktree and delete the branch
        self.remove_and_delete_branch(source_branch, false).await?;

        Ok(result)
    }

    // ── Branching ─────────────────────────────────────────────────────

    /// Create a new branch + worktree in one operation.
    ///
    /// Equivalent to `git worktree add -b <branch> <path> [<start-point>]`.
    pub async fn branch(
        &self,
        branch: &str,
        target_path: &Path,
        start_point: Option<&str>,
    ) -> Result<WorktreeInfo> {
        self.create_worktree(branch, target_path, start_point).await
    }

    /// Create a feature worktree: sibling directory named after the branch.
    pub async fn feature(&self, branch: &str) -> Result<WorktreeInfo> {
        let opts = WorktreeCreateOpts::feature_branch(branch, &self.repo_root);
        self.create(&opts).await
    }

    /// Create a hotfix worktree: branching off a specific start point.
    pub async fn hotfix(&self, branch: &str, start_point: &str) -> Result<WorktreeInfo> {
        let opts = WorktreeCreateOpts::hotfix(branch, start_point, &self.repo_root);
        self.create(&opts).await
    }

    // ── Status helpers ────────────────────────────────────────────────

    /// Check whether a worktree has uncommitted changes.
    pub async fn is_dirty(&self, worktree_path: &Path) -> Result<bool> {
        let output = self
            .run_git_output_at(worktree_path, &["status", "--porcelain"])
            .await?;
        Ok(!output.trim().is_empty())
    }

    /// Get the current branch name of a worktree.
    pub async fn current_branch(&self, worktree_path: &Path) -> Result<String> {
        let output = self
            .run_git_output_at(worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await?;
        let branch = output.trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            bail!(
                "Worktree at {} has a detached HEAD",
                worktree_path.display()
            );
        }
        Ok(branch)
    }

    /// Get the commit SHA of HEAD in a worktree.
    pub async fn head_commit(&self, worktree_path: &Path) -> Result<String> {
        let output = self
            .run_git_output_at(worktree_path, &["rev-parse", "HEAD"])
            .await?;
        Ok(output.trim().to_string())
    }

    /// Check if a branch name already exists in the repo.
    pub async fn branch_exists(&self, branch: &str) -> Result<bool> {
        let output = self.run_git_output(&["branch", "--list", branch]).await?;
        Ok(!output.trim().is_empty())
    }

    // ── Parsing ───────────────────────────────────────────────────────

    /// Parse the porcelain output of `git worktree list --porcelain`.
    ///
    /// Format:
    /// ```text
    /// worktree /path/to/main
    /// HEAD abc123...
    /// branch refs/heads/main
    ///
    /// worktree /path/to/linked
    /// HEAD def456...
    /// branch refs/heads/feat/x
    /// ```
    fn parse_worktree_list(porcelain: &str) -> Result<Vec<WorktreeInfo>> {
        let mut worktrees = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_head: Option<String> = None;
        let mut current_branch: Option<String> = None;
        let mut current_is_bare = false;
        let mut current_is_detached = false;
        let mut current_is_prunable = false;
        let mut current_is_locked = false;

        for line in porcelain.lines() {
            let line = line.trim();

            if line.starts_with("worktree ") {
                // Flush previous entry
                if let Some(path) = current_path.take() {
                    let branch = current_branch.take().map(|b| {
                        // Strip "refs/heads/" prefix
                        if let Some(stripped) = b.strip_prefix("refs/heads/") {
                            stripped.to_string()
                        } else {
                            b
                        }
                    });
                    let commit = current_head.take().unwrap_or_default();
                    let is_main = worktrees.is_empty(); // First entry is the main worktree
                    worktrees.push(WorktreeInfo {
                        path,
                        branch,
                        commit,
                        is_main,
                        is_bare: current_is_bare,
                        is_detached: current_is_detached,
                        is_prunable: current_is_prunable,
                        is_locked: current_is_locked,
                    });
                }

                // Start new entry
                current_path = Some(PathBuf::from(line.strip_prefix("worktree ").unwrap()));
                current_head = None;
                current_branch = None;
                current_is_bare = false;
                current_is_detached = false;
                current_is_prunable = false;
                current_is_locked = false;
            } else if line.starts_with("HEAD ") {
                current_head = Some(line.strip_prefix("HEAD ").unwrap().to_string());
            } else if line.starts_with("branch ") {
                current_branch = Some(line.strip_prefix("branch ").unwrap().to_string());
            } else if line == "bare" {
                current_is_bare = true;
            } else if line == "detached" {
                current_is_detached = true;
            } else if line.starts_with("prunable") {
                current_is_prunable = true;
            } else if line.starts_with("locked") {
                current_is_locked = true;
            }
        }

        // Flush last entry
        if let Some(path) = current_path {
            let branch = current_branch.map(|b| {
                if let Some(stripped) = b.strip_prefix("refs/heads/") {
                    stripped.to_string()
                } else {
                    b
                }
            });
            let commit = current_head.unwrap_or_default();
            let is_main = worktrees.is_empty();
            worktrees.push(WorktreeInfo {
                path,
                branch,
                commit,
                is_main,
                is_bare: current_is_bare,
                is_detached: current_is_detached,
                is_prunable: current_is_prunable,
                is_locked: current_is_locked,
            });
        }

        Ok(worktrees)
    }

    /// Extract conflict file paths from merge output.
    fn extract_conflicts(merge_output: &str) -> Vec<String> {
        let mut conflicts = Vec::new();
        for line in merge_output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("CONFLICT") || trimmed.contains("Merge conflict") {
                // Try to extract the file path from the line
                // Format: "CONFLICT (content): Merge conflict in path/to/file"
                if let Some(pos) = trimmed.rfind(" in ") {
                    conflicts.push(trimmed[pos + 4..].trim().to_string());
                } else if let Some(pos) = trimmed.rfind(": ") {
                    conflicts.push(trimmed[pos + 2..].trim().to_string());
                }
            }
        }
        conflicts
    }

    // ── Command execution ─────────────────────────────────────────────

    /// Run a git command in the repo root, checking for a successful exit.
    async fn run_git(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .with_context(|| format!("Failed to execute `git {}`", args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`git {}` failed (exit {}): {}",
                args.join(" "),
                output.status.code().unwrap_or(-1),
                stderr.trim()
            );
        }

        Ok(())
    }

    /// Run a git command and return its stdout as a String.
    async fn run_git_output(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .with_context(|| format!("Failed to execute `git {}`", args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`git {}` failed (exit {}): {}",
                args.join(" "),
                output.status.code().unwrap_or(-1),
                stderr.trim()
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Run a git command at a specific working directory (e.g., a linked worktree).
    async fn run_git_output_at(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .with_context(|| {
                format!(
                    "Failed to execute `git {}` in {}",
                    args.join(" "),
                    cwd.display()
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`git {}` failed (exit {}): {}",
                args.join(" "),
                output.status.code().unwrap_or(-1),
                stderr.trim()
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

// ── Skill instructions ───────────────────────────────────────────────

/// Generate the skill instructions to be injected into the system prompt
/// when the worktree skill is active.
///
/// This tells the LLM how to manage git worktrees for parallel development.
pub fn skill_instructions() -> String {
    r#"# Git Worktree Skill

You are now operating in **worktree mode**. Your goal is to manage git worktrees
for parallel feature development, hotfixes, and branch isolation.

## Capabilities

### 1. Create Worktrees
- Create linked worktrees for parallel feature branches
- Create hotfix worktrees based on specific commits or tags
- Create detached worktrees for temporary exploration
- Automatic sibling-directory naming from branch names

### 2. List Worktrees
- List all worktrees with branch, commit, and status info
- Identify main vs linked worktrees
- Detect prunable (stale) and locked worktrees
- Find worktrees by branch name or path

### 3. Merge Worktrees
- Merge a worktree's branch into a target branch (e.g., main)
- Detect and report merge conflicts
- Fast-forward and merge-commit strategies
- Merge-and-remove workflow for completed features

### 4. Clean Up Worktrees
- Remove individual worktrees (with dirty-check protection)
- Prune stale worktree metadata
- Remove worktree and delete its branch in one step
- Force removal for unmerged branches

### 5. Worktree Branching
- Create branch + worktree in a single operation
- Feature and hotfix presets with sensible defaults
- Branch-existence checking before creation

## Workflow

### Parallel Feature Development
```bash
# Create worktrees for two independent features
git worktree add -b feat/auth ../auth-worktree
git worktree add -b feat/dashboard ../dashboard-worktree

# Work in each directory independently
cd ../auth-worktree && git commit -am "Add auth module"
cd ../dashboard-worktree && git commit -am "Add dashboard layout"

# Merge when ready
git checkout main && git merge feat/auth
git checkout main && git merge feat/dashboard

# Clean up
git worktree remove ../auth-worktree && git branch -d feat/auth
git worktree remove ../dashboard-worktree && git branch -d feat/dashboard
```

### Hotfix Workflow
```bash
# Create a hotfix worktree based on a tag
git worktree add -b hotfix/fix-crash ../hotfix-crash v1.2.0

# Fix the issue
cd ../hotfix-crash && git commit -am "Fix null pointer crash"

# Merge back to main and tag
git checkout main && git merge hotfix/fix-crash
git worktree remove ../hotfix-crash && git branch -d hotfix/fix-crash
```

## Guidelines

- **Always clean up** — remove worktrees and delete branches after merging
- **Check for dirty state** — don't remove worktrees with uncommitted changes
- **Use descriptive branch names** — `feat/`, `fix/`, `hotfix/`, `refactor/` prefixes
- **One feature per worktree** — keep concerns isolated
- **Prune regularly** — run `git worktree prune` to clean up stale metadata
- **Avoid nested worktrees** — place worktrees as sibling directories, not children
- **Resolve conflicts before removing** — merge conflicts block cleanup

## Common Commands

```bash
# List all worktrees
git worktree list

# Create a feature worktree
git worktree add -b feat/new-feature ../new-feature

# Create from specific commit
git worktree add -b hotfix/urgent ../urgent abc123

# Check status of a worktree
cd ../worktree-dir && git status

# Remove a clean worktree
git worktree remove ../worktree-dir

# Force remove (even if dirty)
git worktree remove --force ../worktree-dir

# Prune deleted worktrees
git worktree prune
```
"#
    .to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── WorktreeInfo tests ───────────────────────────────────────────

    #[test]
    fn test_commit_short() {
        let info = WorktreeInfo {
            path: PathBuf::from("/repo"),
            branch: Some("main".to_string()),
            commit: "abc123def456789".to_string(),
            is_main: true,
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_locked: false,
        };
        assert_eq!(info.commit_short(), "abc123d");
    }

    #[test]
    fn test_branch_display() {
        let mut info = WorktreeInfo {
            path: PathBuf::from("/repo"),
            branch: Some("feat/auth".to_string()),
            commit: "abc123def456789".to_string(),
            is_main: false,
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_locked: false,
        };

        assert_eq!(info.branch(), "feat/auth");

        // Detached HEAD
        info.branch = None;
        info.is_detached = true;
        assert!(info.branch().contains("detached"));
        assert!(info.branch().contains("abc123d"));
    }

    #[test]
    fn test_dir_name() {
        let info = WorktreeInfo {
            path: PathBuf::from("/projects/auth-worktree"),
            branch: Some("feat/auth".to_string()),
            commit: "abc123".to_string(),
            is_main: false,
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_locked: false,
        };
        assert_eq!(info.dir_name(), "auth-worktree");
    }

    #[test]
    fn test_display() {
        let info = WorktreeInfo {
            path: PathBuf::from("/repo"),
            branch: Some("main".to_string()),
            commit: "abc123def456789".to_string(),
            is_main: true,
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_locked: false,
        };
        let display = format!("{}", info);
        assert!(display.contains("main"));
        assert!(display.contains("abc123d"));
        assert!(display.contains("[main]"));
        assert!(!display.contains("[locked]"));
    }

    #[test]
    fn test_display_with_flags() {
        let info = WorktreeInfo {
            path: PathBuf::from("/repo"),
            branch: Some("feat".to_string()),
            commit: "def456".to_string(),
            is_main: false,
            is_bare: false,
            is_detached: false,
            is_prunable: true,
            is_locked: true,
        };
        let display = format!("{}", info);
        assert!(display.contains("[prunable]"));
        assert!(display.contains("[locked]"));
    }

    // ── WorktreeCreateOpts tests ─────────────────────────────────────

    #[test]
    fn test_feature_branch_opts() {
        let repo_root = PathBuf::from("/projects/myapp");
        let opts = WorktreeCreateOpts::feature_branch("feat/auth", &repo_root);
        assert_eq!(opts.branch, "feat/auth");
        assert_eq!(opts.path, PathBuf::from("/projects/feat-auth"));
        assert!(opts.start_point.is_none());
        assert!(!opts.force_branch);
        assert!(!opts.detach);
    }

    #[test]
    fn test_hotfix_opts() {
        let repo_root = PathBuf::from("/projects/myapp");
        let opts = WorktreeCreateOpts::hotfix("hotfix/crash", "v1.2.0", &repo_root);
        assert_eq!(opts.branch, "hotfix/crash");
        assert_eq!(opts.path, PathBuf::from("/projects/hotfix-crash"));
        assert_eq!(opts.start_point, Some("v1.2.0".to_string()));
    }

    #[test]
    fn test_create_opts_serde_roundtrip() {
        let opts = WorktreeCreateOpts {
            branch: "feat/x".to_string(),
            path: PathBuf::from("/tmp/wt"),
            start_point: Some("main".to_string()),
            force_branch: true,
            detach: false,
        };

        let json = serde_json::to_string(&opts).unwrap();
        let parsed: WorktreeCreateOpts = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.branch, "feat/x");
        assert_eq!(parsed.path, PathBuf::from("/tmp/wt"));
        assert_eq!(parsed.start_point, Some("main".to_string()));
        assert!(parsed.force_branch);
        assert!(!parsed.detach);
    }

    // ── Parsing tests ────────────────────────────────────────────────

    #[test]
    fn test_parse_worktree_list_single() {
        let porcelain = "worktree /home/user/project\nHEAD abc123def456789012345678901234567890abcd\nbranch refs/heads/main\n";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/project"));
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
        assert_eq!(
            worktrees[0].commit,
            "abc123def456789012345678901234567890abcd"
        );
        assert!(worktrees[0].is_main);
        assert!(!worktrees[0].is_detached);
    }

    #[test]
    fn test_parse_worktree_list_multiple() {
        let porcelain = "\
worktree /home/user/project
HEAD aaa111
branch refs/heads/main

worktree /home/user/feat-auth
HEAD bbb222
branch refs/heads/feat/auth

";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert_eq!(worktrees.len(), 2);

        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch, Some("main".to_string()));

        assert!(!worktrees[1].is_main);
        assert_eq!(worktrees[1].branch, Some("feat/auth".to_string()));
        assert_eq!(worktrees[1].path, PathBuf::from("/home/user/feat-auth"));
    }

    #[test]
    fn test_parse_worktree_detached() {
        let porcelain = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/main

worktree /home/user/explore
HEAD def456
detached

";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[1].is_detached);
        assert!(worktrees[1].branch.is_none());
    }

    #[test]
    fn test_parse_worktree_bare() {
        let porcelain = "worktree /home/user/repo.git\nHEAD abc123\nbare\n";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].is_bare);
    }

    #[test]
    fn test_parse_worktree_prunable() {
        let porcelain = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/main

worktree /home/user/deleted-dir
HEAD def456
branch refs/heads/orphan
prunable gitdir pointing to nowhere

";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[1].is_prunable);
    }

    #[test]
    fn test_parse_worktree_locked() {
        let porcelain = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/main

worktree /home/user/locked-wt
HEAD def456
branch refs/heads/locked-branch
locked

";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert!(worktrees[1].is_locked);
    }

    #[test]
    fn test_parse_empty_list() {
        let porcelain = "";
        let worktrees = WorktreeManager::parse_worktree_list(porcelain).unwrap();
        assert!(worktrees.is_empty());
    }

    // ── WorktreeList tests ───────────────────────────────────────────

    #[test]
    fn test_worktree_list_queries() {
        let list = WorktreeList {
            worktrees: vec![
                WorktreeInfo {
                    path: PathBuf::from("/main"),
                    branch: Some("main".to_string()),
                    commit: "aaa".to_string(),
                    is_main: true,
                    is_bare: false,
                    is_detached: false,
                    is_prunable: false,
                    is_locked: false,
                },
                WorktreeInfo {
                    path: PathBuf::from("/feat"),
                    branch: Some("feat/x".to_string()),
                    commit: "bbb".to_string(),
                    is_main: false,
                    is_bare: false,
                    is_detached: false,
                    is_prunable: false,
                    is_locked: false,
                },
            ],
            repo_root: PathBuf::from("/main"),
        };

        assert_eq!(list.len(), 2);
        assert!(list.main().is_some());
        assert_eq!(list.linked().len(), 1);
        assert!(list.by_branch("feat/x").is_some());
        assert!(list.by_branch("nonexistent").is_none());
        assert!(list.by_path(&PathBuf::from("/feat")).is_some());
    }

    // ── Conflict extraction ──────────────────────────────────────────

    #[test]
    fn test_extract_conflicts() {
        let output = "\
Auto-merging src/main.rs
CONFLICT (content): Merge conflict in src/main.rs
Auto-merging src/lib.rs
CONFLICT (content): Merge conflict in src/lib.rs
Automatic merge failed; fix conflicts and then commit the result.
";
        let conflicts = WorktreeManager::extract_conflicts(output);
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.contains(&"src/main.rs".to_string()));
        assert!(conflicts.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_extract_conflicts_empty() {
        let output = "Merge made by the 'ort' strategy.\n src/main.rs | 2 +-";
        let conflicts = WorktreeManager::extract_conflicts(output);
        assert!(conflicts.is_empty());
    }

    // ── MergeResult tests ────────────────────────────────────────────

    #[test]
    fn test_merge_result_serde() {
        let result = MergeResult {
            source_branch: "feat/auth".to_string(),
            target_branch: "main".to_string(),
            was_merge_commit: true,
            merge_commit: Some("abc123".to_string()),
            had_conflicts: false,
            conflicts: vec![],
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: MergeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source_branch, "feat/auth");
        assert!(parsed.was_merge_commit);
        assert!(!parsed.had_conflicts);
    }

    // ── Skill instructions ───────────────────────────────────────────

    #[test]
    fn test_skill_instructions_not_empty() {
        let instructions = skill_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.contains("worktree"));
        assert!(instructions.contains("Create"));
        assert!(instructions.contains("Merge"));
        assert!(instructions.contains("Clean Up"));
    }

    // ── Integration tests (require a git repo) ───────────────────────

    #[tokio::test]
    async fn test_manager_for_current_repo() {
        // This test only works inside a git repo (which pi2oxi is)
        let result = WorktreeManager::for_current_repo();
        // We're running inside the pi2oxi repo, so this should succeed
        assert!(result.is_ok());
        let mgr = result.unwrap();
        assert!(mgr.repo_root().exists());
    }

    #[tokio::test]
    async fn test_list_worktrees() {
        let mgr = WorktreeManager::for_current_repo().unwrap();
        let list = mgr.list().await.unwrap();
        // Should have at least the main worktree
        assert!(!list.is_empty());
        assert!(list.main().is_some());
    }
}
