//! Internal URL scheme handlers for `issue://` and `pr://` virtual paths.
//!
//! Each handler implements [`oxi_sdk::ports::ProtocolHandler`] and is
//! registered with the `InternalUrlRouter` port so the `read`/`search`
//! tools can resolve scheme URIs to markdown text.

pub mod issue_handler;
pub mod memory_handler;
pub mod pr_handler;
/// Detect the current Git repo's `owner/repo` from the `origin` remote.
///
/// Returns `None` when `git remote get-url origin` fails or the URL is
/// not a recognizable GitHub remote.
pub(crate) fn detect_github_repo() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        return None;
    }
    parse_github_owner_repo(&url)
}

/// Extract `owner/repo` from a GitHub remote URL.
fn parse_github_owner_repo(url: &str) -> Option<String> {
    // Handles:
    //   https://github.com/owner/repo.git
    //   https://github.com/owner/repo
    //   git@github.com:owner/repo.git
    //   git@github.com:owner/repo
    //   ssh://git@github.com/owner/repo.git
    let url = url.trim();

    // Strip leading/trailing whitespace and newlines
    let stripped = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;

    let stripped = stripped.strip_suffix(".git").unwrap_or(stripped);
    let stripped = stripped.trim_end_matches('/');

    // Validate owner/repo form
    let parts: Vec<&str> = stripped.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
}

/// Get a GitHub token from environment (GITHUB_TOKEN or GH_TOKEN).
pub(crate) fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|t| !t.is_empty())
}
