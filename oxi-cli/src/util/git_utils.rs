//! Git utilities for branch detection and status

use std::path::Path;

/// Get the current git branch name
pub fn get_current_branch(repo_dir: &Path) -> Option<String> {
    std::process::Command::new("git")
        .current_dir(repo_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?
        .stdout
        .split(|&b| b == b'\n')
        .next()
        .filter(|s| !s.is_empty() && *s != b"HEAD")
        .map(|s| String::from_utf8_lossy(s).into_owned())
}
