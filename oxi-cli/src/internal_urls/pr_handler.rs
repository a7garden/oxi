//! `pr://` protocol handler — resolves GitHub pull request references to markdown.
//!
//! URL formats:
//! - `N` — PR N in the current repo (detected from git remote)
//! - `owner/repo/N` — explicit repo
//! - `owner/repo/N/diff/M` — diff view (M = file index, usually 0 for full)
//!
//! Uses the GitHub REST API with optional `GITHUB_TOKEN`/`GH_TOKEN` auth.

use async_trait::async_trait;
use oxi_sdk::SdkError;
use oxi_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};
use serde::Deserialize;

use super::{detect_github_repo, github_token};
use crate::util::http_client::shared_http_client;

/// Protocol handler for `pr://` URLs.
#[derive(Debug, Clone, Default)]
pub struct PrProtocolHandler;

/// Parsed PR URL components.
struct PrUrl {
    owner: String,
    repo: String,
    pr_number: u64,
    /// If true, fetch and include the diff.
    diff: bool,
}

/// GitHub REST API PR response (subset).
#[derive(Debug, Deserialize)]
struct GhPr {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    user: Option<GhUser>,
    labels: Option<Vec<GhLabel>>,
    created_at: Option<String>,
    merged_at: Option<String>,
    closed_at: Option<String>,
    draft: Option<bool>,
    head: Option<GhRef>,
    base: Option<GhRef>,
    mergeable: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhRef {
    #[serde(rename = "ref")]
    ref_name: String,
}

impl PrProtocolHandler {
    /// Parse the URL path into owner/repo/pr_number + optional diff flag.
    fn parse_url(url: &str) -> Result<PrUrl, SdkError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(SdkError::Internal(anyhow::anyhow!("empty PR URL")));
        }

        let parts: Vec<&str> = url.split('/').collect();

        // Check for diff suffix: owner/repo/N/diff/M
        let (core_parts, wants_diff) = if parts.len() >= 5 && parts[parts.len() - 2] == "diff" {
            // Last two parts are "diff" and "M"
            (&parts[..parts.len() - 2], true)
        } else {
            (parts.as_slice(), false)
        };

        match core_parts.len() {
            1 => {
                // Just a PR number — use current repo
                let pr_number: u64 = core_parts[0].parse().map_err(|_| {
                    SdkError::Internal(anyhow::anyhow!("invalid PR number: {}", core_parts[0]))
                })?;
                let repo = detect_github_repo().ok_or_else(|| {
                    SdkError::Internal(anyhow::anyhow!(
                        "could not detect GitHub repo from git remote; use owner/repo/N format"
                    ))
                })?;
                let (owner, repo_name) = split_owner_repo(&repo)?;
                Ok(PrUrl {
                    owner,
                    repo: repo_name,
                    pr_number,
                    diff: wants_diff,
                })
            }
            2 => {
                // owner/repo — missing PR number
                Err(SdkError::Internal(anyhow::anyhow!(
                    "PR URL requires a number: {url} (use owner/repo/N)"
                )))
            }
            3 => {
                // owner/repo/N
                let pr_number: u64 = core_parts[2].parse().map_err(|_| {
                    SdkError::Internal(anyhow::anyhow!("invalid PR number: {}", core_parts[2]))
                })?;
                Ok(PrUrl {
                    owner: core_parts[0].to_string(),
                    repo: core_parts[1].to_string(),
                    pr_number,
                    diff: wants_diff,
                })
            }
            _ => Err(SdkError::Internal(anyhow::anyhow!(
                "invalid PR URL format: {url}"
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
impl ProtocolHandler for PrProtocolHandler {
    fn scheme(&self) -> &str {
        "pr"
    }

    async fn resolve(
        &self,
        url: &str,
        _selector: Option<&str>,
        _ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError> {
        let parsed = Self::parse_url(url)?;

        let client = shared_http_client();
        let token = github_token();

        // Fetch PR details
        let pr_api_url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            parsed.owner, parsed.repo, parsed.pr_number
        );

        let mut request = client
            .get(&pr_api_url)
            .header("User-Agent", "oxi-cli")
            .header("Accept", "application/vnd.github.v3+json");

        if let Some(ref t) = token {
            request = request.header("Authorization", format!("Bearer {}", t));
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

        let pr: GhPr = response.json().await.map_err(|e| {
            SdkError::Internal(anyhow::anyhow!("failed to parse GitHub API response: {e}"))
        })?;

        let mut md = format_pr_markdown(&pr);

        // Optionally fetch diff
        if parsed.diff {
            md.push_str("\n\n## Diff\n\n");
            match fetch_pr_diff(&client, &parsed, token.as_deref()).await {
                Ok(diff) => {
                    md.push_str("```diff\n");
                    md.push_str(&diff);
                    md.push_str("\n```\n");
                }
                Err(e) => {
                    md.push_str(&format!("*Failed to fetch diff: {e}*\n"));
                }
            }
        }

        Ok(ResolvedUrl {
            url: format!(
                "https://github.com/{}/{}/pull/{}",
                parsed.owner, parsed.repo, parsed.pr_number
            ),
            content: md,
            content_type: "text/markdown".into(),
            size: None,
            source_path: None,
            notes: vec![],
            immutable: false,
        })
    }
}

async fn fetch_pr_diff(
    client: &reqwest::Client,
    pr: &PrUrl,
    token: Option<&str>,
) -> Result<String, SdkError> {
    let diff_url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}",
        pr.owner, pr.repo, pr.pr_number
    );

    let mut request = client
        .get(&diff_url)
        .header("User-Agent", "oxi-cli")
        .header("Accept", "application/vnd.github.v3.diff");

    if let Some(t) = token {
        request = request.header("Authorization", format!("Bearer {}", t));
    }

    let response = request
        .send()
        .await
        .map_err(|e| SdkError::Internal(anyhow::anyhow!("GitHub diff request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(SdkError::Internal(anyhow::anyhow!(
            "GitHub diff API returned {status}: {body}"
        )));
    }

    response
        .text()
        .await
        .map_err(|e| SdkError::Internal(anyhow::anyhow!("failed to read diff response: {e}")))
}

fn format_pr_markdown(pr: &GhPr) -> String {
    let mut md = format!("# PR #{}: {}\n\n", pr.number, pr.title);

    // State
    let state_label = match (pr.state.as_str(), pr.draft) {
        (_, Some(true)) => "📝 Draft",
        ("open", _) => "🟢 Open",
        ("closed", _) => "🔴 Closed",
        ("merged", _) => "🟣 Merged",
        (other, _) => other,
    };
    md.push_str(&format!("**State:** {}\n\n", state_label));

    // Author
    if let Some(ref user) = pr.user {
        md.push_str(&format!("**Author:** @{}\n\n", user.login));
    }

    // Branch info
    if let Some(ref head) = pr.head {
        if let Some(ref base) = pr.base {
            md.push_str(&format!(
                "**Branch:** `{}` → `{}`\n\n",
                head.ref_name, base.ref_name
            ));
        }
    }

    // Labels
    if let Some(ref labels) = pr.labels {
        if !labels.is_empty() {
            let label_names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
            md.push_str(&format!("**Labels:** {}\n\n", label_names.join(", ")));
        }
    }

    // Mergeable
    if let Some(mergeable) = pr.mergeable {
        md.push_str(&format!(
            "**Mergeable:** {}\n\n",
            if mergeable { "✅ Yes" } else { "❌ No" }
        ));
    }

    // Dates
    if let Some(ref created) = pr.created_at {
        md.push_str(&format!("**Created:** {}\n", created));
    }
    if let Some(ref merged) = pr.merged_at {
        md.push_str(&format!("**Merged:** {}\n", merged));
    } else if let Some(ref closed) = pr.closed_at {
        md.push_str(&format!("**Closed:** {}\n", closed));
    }

    md.push('\n');

    // Body
    if let Some(ref body) = pr.body {
        if !body.is_empty() {
            md.push_str("---\n\n");
            md.push_str(body);
            md.push('\n');
        }
    }

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_n() {
        let result = PrProtocolHandler::parse_url("42").unwrap();
        // This will fail without a git remote, but parsing succeeds
        // if we're not in a repo — let's just test the non-repo-dependent path
        // We can't test the 1-part case without a git repo, so skip it.
    }

    #[test]
    fn test_parse_url_owner_repo_n() {
        let result = PrProtocolHandler::parse_url("rust-lang/rust/12345").unwrap();
        assert_eq!(result.owner, "rust-lang");
        assert_eq!(result.repo, "rust");
        assert_eq!(result.pr_number, 12345);
        assert!(!result.diff);
    }

    #[test]
    fn test_parse_url_owner_repo_n_diff() {
        let result = PrProtocolHandler::parse_url("rust-lang/rust/12345/diff/0").unwrap();
        assert_eq!(result.owner, "rust-lang");
        assert_eq!(result.repo, "rust");
        assert_eq!(result.pr_number, 12345);
        assert!(result.diff);
    }

    #[test]
    fn test_parse_url_rejects_two_parts() {
        let result = PrProtocolHandler::parse_url("owner/repo");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_url_rejects_empty() {
        let result = PrProtocolHandler::parse_url("");
        assert!(result.is_err());
    }

    #[test]
    fn test_format_pr_markdown() {
        let pr = GhPr {
            number: 42,
            title: "Add new feature".into(),
            body: Some("Implements the new widget system.".into()),
            state: "open".into(),
            user: Some(GhUser {
                login: "coder".into(),
            }),
            labels: Some(vec![GhLabel {
                name: "enhancement".into(),
            }]),
            created_at: Some("2026-01-15T12:00:00Z".into()),
            merged_at: None,
            closed_at: None,
            draft: Some(false),
            head: Some(GhRef {
                ref_name: "feature/widget".into(),
            }),
            base: Some(GhRef {
                ref_name: "main".into(),
            }),
            mergeable: Some(true),
        };

        let md = format_pr_markdown(&pr);
        assert!(md.contains("# PR #42: Add new feature"));
        assert!(md.contains("🟢 Open"));
        assert!(md.contains("@coder"));
        assert!(md.contains("`feature/widget` → `main`"));
        assert!(md.contains("enhancement"));
        assert!(md.contains("✅ Yes"));
        assert!(md.contains("Implements the new widget system"));
    }

    #[test]
    fn test_format_pr_draft() {
        let pr = GhPr {
            number: 1,
            title: "Draft PR".into(),
            body: None,
            state: "open".into(),
            user: None,
            labels: None,
            created_at: None,
            merged_at: None,
            closed_at: None,
            draft: Some(true),
            head: None,
            base: None,
            mergeable: None,
        };

        let md = format_pr_markdown(&pr);
        assert!(md.contains("📝 Draft"));
    }
}
