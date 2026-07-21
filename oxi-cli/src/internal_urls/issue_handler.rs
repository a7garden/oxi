//! `issue://` protocol handler — resolves GitHub issue references to markdown.
//!
//! URL formats:
//! - `N` — issue N in the current repo (detected from git remote)
//! - `owner/repo/N` — explicit repo
//!
//! Uses the GitHub REST API with optional `GITHUB_TOKEN`/`GH_TOKEN` auth.

use async_trait::async_trait;
use oxi_sdk::SdkError;
use oxi_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};
use serde::Deserialize;

use super::{detect_github_repo, github_token};
use crate::util::http_client::shared_http_client;

/// Protocol handler for `issue://` URLs.
#[derive(Debug, Clone, Default)]
pub struct IssueProtocolHandler;

/// Parsed issue URL components.
struct IssueUrl {
    owner: String,
    repo: String,
    issue_number: u64,
}

/// GitHub REST API issue response (subset).
#[derive(Debug, Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    user: Option<GhUser>,
    labels: Option<Vec<GhLabel>>,
    created_at: Option<String>,
    closed_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

impl IssueProtocolHandler {
    /// Parse the URL path into owner/repo/issue_number.
    fn parse_url(url: &str) -> Result<IssueUrl, SdkError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(SdkError::Internal(anyhow::anyhow!("empty issue URL")));
        }

        let parts: Vec<&str> = url.split('/').collect();
        match parts.len() {
            1 => {
                // Just an issue number — use current repo
                let issue_number: u64 = parts[0].parse().map_err(|_| {
                    SdkError::Internal(anyhow::anyhow!("invalid issue number: {}", parts[0]))
                })?;
                let repo = detect_github_repo().ok_or_else(|| {
                    SdkError::Internal(anyhow::anyhow!(
                        "could not detect GitHub repo from git remote; use owner/repo/N format"
                    ))
                })?;
                let (owner, repo_name) = split_owner_repo(&repo)?;
                Ok(IssueUrl {
                    owner,
                    repo: repo_name,
                    issue_number,
                })
            }
            2 => {
                // owner/repo — missing issue number
                Err(SdkError::Internal(anyhow::anyhow!(
                    "issue URL requires a number: {url} (use owner/repo/N)"
                )))
            }
            3 => {
                // owner/repo/N
                let issue_number: u64 = parts[2].parse().map_err(|_| {
                    SdkError::Internal(anyhow::anyhow!("invalid issue number: {}", parts[2]))
                })?;
                Ok(IssueUrl {
                    owner: parts[0].to_string(),
                    repo: parts[1].to_string(),
                    issue_number,
                })
            }
            _ => Err(SdkError::Internal(anyhow::anyhow!(
                "invalid issue URL format: {url}"
            ))),
        }
    }
}

fn split_owner_repo(repo: &str) -> Result<(String, String), SdkError> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(SdkError::Internal(anyhow::anyhow!(
            "invalid repo format (expected owner/repo): {repo}"
        )));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

#[async_trait]
impl ProtocolHandler for IssueProtocolHandler {
    fn scheme(&self) -> &str {
        "issue"
    }

    async fn resolve(
        &self,
        url: &str,
        _selector: Option<&str>,
        _ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError> {
        let parsed = Self::parse_url(url)?;

        let api_url = format!(
            "https://api.github.com/repos/{}/{}/issues/{}",
            parsed.owner, parsed.repo, parsed.issue_number
        );

        let client = shared_http_client();
        let mut request = client
            .get(&api_url)
            .header("User-Agent", "oxi-cli")
            .header("Accept", "application/vnd.github.v3+json");

        if let Some(token) = github_token() {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = request
            .send()
            .await
            .map_err(|e| SdkError::Internal(anyhow::anyhow!("GitHub API request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SdkError::Internal(anyhow::anyhow!(
                "GitHub API returned {status}: {body}"
            )));
        }

        let issue: GhIssue = response.json().await.map_err(|e| {
            SdkError::Internal(anyhow::anyhow!("failed to parse GitHub API response: {e}"))
        })?;

        let content = format_issue_markdown(&issue);

        Ok(ResolvedUrl {
            url: format!(
                "https://github.com/{}/{}/issues/{}",
                parsed.owner, parsed.repo, parsed.issue_number
            ),
            content,
            content_type: "text/markdown".into(),
            size: None,
            source_path: None,
            notes: vec![],
            immutable: false,
        })
    }
}

fn format_issue_markdown(issue: &GhIssue) -> String {
    let mut md = format!("# Issue #{}: {}\n\n", issue.number, issue.title);

    // State badge
    let state_label = match issue.state.as_str() {
        "open" => "🟢 Open",
        "closed" => "🔴 Closed",
        other => other,
    };
    md.push_str(&format!("**State:** {}\n\n", state_label));

    // Author
    if let Some(ref user) = issue.user {
        md.push_str(&format!("**Author:** @{}\n\n", user.login));
    }

    // Labels
    if let Some(ref labels) = issue.labels
        && !labels.is_empty()
    {
        let label_names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
        md.push_str(&format!("**Labels:** {}\n\n", label_names.join(", ")));
    }

    // Dates
    if let Some(ref created) = issue.created_at {
        md.push_str(&format!("**Created:** {}\n", created));
    }
    if let Some(ref closed) = issue.closed_at {
        md.push_str(&format!("**Closed:** {}\n", closed));
    }

    md.push('\n');

    // Body
    if let Some(ref body) = issue.body
        && !body.is_empty()
    {
        md.push_str("---\n\n");
        md.push_str(body);
        md.push('\n');
    }

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_owner_repo_n() {
        let result = IssueProtocolHandler::parse_url("rust-lang/rust/12345").unwrap();
        assert_eq!(result.owner, "rust-lang");
        assert_eq!(result.repo, "rust");
        assert_eq!(result.issue_number, 12345);
    }

    #[test]
    fn test_parse_url_rejects_two_parts() {
        let result = IssueProtocolHandler::parse_url("owner/repo");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_url_rejects_empty() {
        let result = IssueProtocolHandler::parse_url("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_url_rejects_invalid_number() {
        let result = IssueProtocolHandler::parse_url("owner/repo/abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_format_issue_markdown() {
        let issue = GhIssue {
            number: 42,
            title: "Fix memory leak".into(),
            body: Some("This fixes the leak in the allocator.".into()),
            state: "open".into(),
            user: Some(GhUser {
                login: "dev".into(),
            }),
            labels: Some(vec![
                GhLabel { name: "bug".into() },
                GhLabel {
                    name: "high-priority".into(),
                },
            ]),
            created_at: Some("2026-01-15T12:00:00Z".into()),
            closed_at: None,
        };

        let md = format_issue_markdown(&issue);
        assert!(md.contains("# Issue #42: Fix memory leak"));
        assert!(md.contains("🟢 Open"));
        assert!(md.contains("@dev"));
        assert!(md.contains("bug, high-priority"));
        assert!(md.contains("This fixes the leak"));
    }
}
