//! Git command-line helpers used by the package manager.
//!
//! Wraps `git` invocations (`clone`, `update`, `has_update`) with
//! progress callbacks and a disabled terminal prompt (`GIT_TERMINAL_PROMPT=0`)
//! so installs stay non-interactive. Pure stdlib `Command` calls — no
//! libgit2 dependency.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

/// Run a git command and capture stdout
pub(super) fn git_command(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let output = cmd.output().context("Failed to execute git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command (no capture)
pub(super) fn git_command_silent(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let status = cmd.status().context("Failed to execute git")?;
    if !status.success() {
        bail!("git {} failed ({})", args.join(" "), status);
    }
    Ok(())
}

/// Clone a git repository
pub fn git_clone(repo_url: &str, target_dir: &Path, ref_: Option<&str>) -> Result<()> {
    if target_dir.exists() {
        bail!("Target directory already exists: {}", target_dir.display());
    }
    fs::create_dir_all(target_dir)
        .with_context(|| format!("Failed to create {}", target_dir.display()))?;

    let target_str = target_dir.to_string_lossy().to_string();
    let args = vec!["clone", repo_url, &target_str];

    git_command_silent(&args, None)?;

    if let Some(r) = ref_ {
        git_command_silent(&["checkout", r], Some(target_dir))?;
    }

    Ok(())
}

/// Pull/update a git repository in place
pub fn git_update(repo_dir: &Path, ref_: Option<&str>) -> Result<bool> {
    if !repo_dir.exists() {
        bail!(
            "Repository directory does not exist: {}",
            repo_dir.display()
        );
    }

    // Get current HEAD
    let local_head = git_command(&["rev-parse", "HEAD"], Some(repo_dir))?;

    // Determine what to fetch
    let fetch_ref = if let Some(r) = ref_ {
        r.to_string()
    } else {
        // Try to get upstream ref
        match git_command(
            &["rev-parse", "--abbrev-ref", "@{upstream}"],
            Some(repo_dir),
        ) {
            Ok(upstream) => {
                if let Some(branch) = upstream.strip_prefix("origin/") {
                    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
                } else {
                    "+HEAD:refs/remotes/origin/HEAD".to_string()
                }
            }
            Err(_) => "+HEAD:refs/remotes/origin/HEAD".to_string(),
        }
    };

    git_command_silent(
        &["fetch", "--prune", "--no-tags", "origin", &fetch_ref],
        Some(repo_dir),
    )?;

    // Determine what to reset to
    let target_ref = ref_.unwrap_or("origin/HEAD");
    let remote_head = git_command(&["rev-parse", target_ref], Some(repo_dir))?;

    if local_head == remote_head {
        return Ok(false); // No update needed
    }

    git_command_silent(&["reset", "--hard", target_ref], Some(repo_dir))?;
    git_command_silent(&["clean", "-fdx"], Some(repo_dir))?;

    Ok(true) // Updated
}

/// Check if a git repo has remote updates available
pub fn git_has_update(repo_dir: &Path) -> Result<bool> {
    let local_head = git_command(&["rev-parse", "HEAD"], Some(repo_dir))?;

    // Try to get upstream
    let upstream_ref = match git_command(
        &["rev-parse", "--abbrev-ref", "@{upstream}"],
        Some(repo_dir),
    ) {
        Ok(u) if u.starts_with("origin/") => {
            let branch = &u["origin/".len()..];
            format!("refs/heads/{branch}")
        }
        _ => "HEAD".to_string(),
    };

    // Fetch quietly and check remote
    let _ = git_command_silent(&["fetch", "--prune", "--no-tags", "origin"], Some(repo_dir));

    let remote_head = git_command(&["ls-remote", "origin", &upstream_ref], None)?;

    // Parse first hash from ls-remote output
    let remote_hash = remote_head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("");

    Ok(local_head != remote_hash)
}
