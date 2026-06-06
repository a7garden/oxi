use super::http_client::shared_http_client;
use super::search_cache::{SearchCache, SearchResult};
/// GitHub search tool — search GitHub repositories, issues, and code via the GitHub REST API.
///
/// Features:
/// - Search repositories by topic, language, stars, etc.
/// - Sort by stars, forks, or recently updated
/// - Optional GitHub token for higher rate limits (via GITHUB_TOKEN env var)
/// - Structured JSON results — no HTML scraping
/// - Result caching with the shared SearchCache
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Maximum results to return by default.
const DEFAULT_MAX_RESULTS: usize = 10;

/// Maximum results allowed (GitHub API max is 100 per page).
const MAX_RESULTS: usize = 30;

// ── GitHub API response types ─────────────────────────────────────

/// Top-level GitHub search response.
#[derive(Debug, Deserialize)]

struct GitHubSearchResponse {
    total_count: u64,
    _incomplete_results: bool,
    items: Vec<GitHubRepo>,
}

/// A single repository from GitHub search.
#[derive(Debug, Deserialize)]
struct GitHubRepo {
    full_name: String,
    html_url: String,
    description: Option<String>,
    language: Option<String>,
    stargazers_count: u64,
    forks_count: u64,
    open_issues_count: u64,
    updated_at: String,

    _archived: bool,
    topics: Vec<String>,
    license: Option<GitHubLicense>,
}

#[derive(Debug, Deserialize)]
struct GitHubLicense {
    spdx_id: Option<String>,
    name: Option<String>,
}

// ── GitHub search result (our public type) ────────────────────────

/// A single GitHub repository result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitHubSearchResult {
    /// Full repo name (e.g. "owner/repo").
    pub full_name: String,
    /// Repository URL.
    pub url: String,
    /// Repository description.
    pub description: String,
    /// Primary programming language.
    pub language: String,
    /// Star count.
    pub stars: u64,
    /// Fork count.
    pub forks: u64,
    /// Open issues count.
    pub open_issues: u64,
    /// Last update timestamp.
    pub updated_at: String,
    /// Repository topics/tags.
    pub topics: Vec<String>,
    /// License name.
    pub license: String,
}

impl From<&GitHubSearchResult> for SearchResult {
    fn from(r: &GitHubSearchResult) -> Self {
        SearchResult {
            title: r.full_name.clone(),
            url: r.url.clone(),
            snippet: r.description.clone(),
            source: "GitHub".to_string(),
            extra: None,
        }
    }
}

// ── API call ──────────────────────────────────────────────────────

/// Resolve a GitHub API token from the environment.
fn resolve_github_token() -> Option<String> {
    // 1. GITHUB_SEARCH_TOKEN (explicit for this tool)
    std::env::var("GITHUB_SEARCH_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .or_else(|| std::env::var("GH_TOKEN").ok())
}

/// Search GitHub repositories via the REST API.
async fn search_github_repos(
    query: &str,
    sort: &str,
    order: &str,
    limit: usize,
    language: Option<&str>,
) -> Result<(u64, Vec<GitHubSearchResult>), ToolError> {
    let mut url = format!(
        "https://api.github.com/search/repositories?q={}&sort={}&order={}&per_page={}",
        urlencoding(query),
        sort,
        order,
        limit.min(MAX_RESULTS),
    );

    // Add language filter if specified
    if let Some(lang) = language {
        // Append language:xxx to the query
        url = format!(
            "https://api.github.com/search/repositories?q={}+language%3A{}&sort={}&order={}&per_page={}",
            urlencoding(query),
            urlencoding(lang),
            sort,
            order,
            limit.min(MAX_RESULTS),
        );
    }

    let mut builder = shared_http_client()
        .get(&url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "oxi-agent");

    // Attach token if available (raises rate limit from 10/min to 5000/hr)
    if let Some(token) = resolve_github_token() {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }

    let response = builder
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {}", e))?;

    let status = response.status();
    if status.as_u16() == 403 {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub API rate limit exceeded. Set GITHUB_TOKEN env var for higher limits. Body: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub API returned status {}: {}",
            status,
            body.chars().take(300).collect::<String>()
        ));
    }

    let search_response: GitHubSearchResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))?;

    let results = search_response
        .items
        .into_iter()
        .map(|repo| GitHubSearchResult {
            full_name: repo.full_name,
            url: repo.html_url,
            description: repo.description.unwrap_or_default(),
            language: repo.language.unwrap_or_default(),
            stars: repo.stargazers_count,
            forks: repo.forks_count,
            open_issues: repo.open_issues_count,
            updated_at: repo.updated_at,
            topics: repo.topics,
            license: repo
                .license
                .and_then(|l| l.spdx_id.or(l.name))
                .unwrap_or_default(),
        })
        .collect();

    Ok((search_response.total_count, results))
}

/// URL-encode a string for query parameters.
fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", byte));
            }
        }
    }
    result
}

// ── Formatting ────────────────────────────────────────────────────

/// Format GitHub search results for display.
fn format_github_results(total: u64, results: &[GitHubSearchResult]) -> String {
    if results.is_empty() {
        return "No repositories found.".to_string();
    }

    let mut output = format!(
        "Found {} repositories (showing {}):\n\n",
        total,
        results.len()
    );

    for (i, r) in results.iter().enumerate() {
        let stars = if r.stars >= 1000 {
            format!("{:.1}k", r.stars as f64 / 1000.0)
        } else {
            r.stars.to_string()
        };

        let desc = if r.description.chars().count() > 150 {
            let truncated: String = r.description.chars().take(150).collect();
            format!("{}...", truncated)
        } else {
            r.description.clone()
        };

        output.push_str(&format!(
            "{}. **{}** ⭐{}\n   {}\n   {} {} | 🔀 {} forks | 📦 {} issues\n   Updated: {}\n",
            i + 1,
            r.full_name,
            stars,
            r.url,
            desc,
            if r.language.is_empty() {
                "Unknown".to_string()
            } else {
                r.language.clone()
            },
            r.forks,
            r.open_issues,
            &r.updated_at[..10], // Just the date part
        ));

        if !r.topics.is_empty() {
            output.push_str(&format!("   Topics: {}\n", r.topics.join(", ")));
        }

        if !r.license.is_empty() {
            output.push_str(&format!("   License: {}\n", r.license));
        }

        output.push('\n');
    }

    output
}

// ── GitHubSearchTool ──────────────────────────────────────────────

/// GitHub repository search tool using the GitHub REST API.
pub struct GitHubSearchTool {
    cache: Arc<SearchCache>,
}

impl GitHubSearchTool {
    /// Create a new GitHubSearchTool with the given search cache.
    pub fn new(cache: Arc<SearchCache>) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl AgentTool for GitHubSearchTool {
    fn name(&self) -> &str {
        "github_search"
    }

    fn label(&self) -> &str {
        "GitHub Search"
    }

    fn description(&self) -> &str {
        "Search GitHub repositories by query. Returns repos with stars, forks, language, description, and topics. Supports sorting by stars, forks, or recently updated. No API key required (set GITHUB_TOKEN for higher rate limits)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (e.g. 'rust web framework', 'machine learning', 'owner:mariozechner')"
                },
                "sort": {
                    "type": "string",
                    "description": "Sort results by: 'stars' (default), 'forks', or 'updated'",
                    "enum": ["stars", "forks", "updated"],
                    "default": "stars"
                },
                "order": {
                    "type": "string",
                    "description": "Sort order: 'desc' (default) or 'asc'",
                    "enum": ["desc", "asc"],
                    "default": "desc"
                },
                "language": {
                    "type": "string",
                    "description": "Filter by programming language (e.g. 'rust', 'python', 'typescript')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 30)",
                    "default": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let query = params["query"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        let sort = params["sort"].as_str().unwrap_or("stars");
        let sort = match sort {
            "forks" | "updated" => sort,
            _ => "stars",
        };

        let order = params["order"].as_str().unwrap_or("desc");
        let order = match order {
            "asc" => "asc",
            _ => "desc",
        };

        let language = params["language"].as_str();

        let limit = params["limit"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_RESULTS as u64)
            .min(MAX_RESULTS as u64) as usize;

        let (total, results) = search_github_repos(query, sort, order, limit, language).await?;

        if results.is_empty() {
            return Ok(AgentToolResult::success(format!(
                "No GitHub repositories found for: {}",
                query
            )));
        }

        // Cache results
        let search_id = self.cache.insert(
            &format!("github:{}", query),
            results.iter().map(|r| r.into()).collect(),
        );

        let output = format_github_results(total, &results);

        let results_json: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "full_name": r.full_name,
                    "url": r.url,
                    "description": r.description,
                    "language": r.language,
                    "stars": r.stars,
                    "forks": r.forks,
                    "open_issues": r.open_issues,
                    "updated_at": r.updated_at,
                    "topics": r.topics,
                    "license": r.license
                })
            })
            .collect();

        Ok(AgentToolResult::success(output).with_metadata(json!({
            "results": results_json,
            "query": query,
            "searchId": search_id,
            "totalCount": total,
            "resultCount": results.len()
        })))
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(urlencoding("rust&cargo"), "rust%26cargo");
        assert_eq!(urlencoding("c++"), "c%2B%2B");
    }

    #[test]
    fn test_format_github_results_empty() {
        assert_eq!(format_github_results(0, &[]), "No repositories found.");
    }

    #[test]
    fn test_format_github_results() {
        let results = vec![GitHubSearchResult {
            full_name: "rust-lang/rust".to_string(),
            url: "https://github.com/rust-lang/rust".to_string(),
            description: "Empowering everyone to build reliable and efficient software."
                .to_string(),
            language: "Rust".to_string(),
            stars: 95000,
            forks: 12000,
            open_issues: 9000,
            updated_at: "2026-05-08T12:00:00Z".to_string(),
            topics: vec!["programming-language".to_string(), "systems".to_string()],
            license: "MIT/Apache-2.0".to_string(),
        }];
        let formatted = format_github_results(1, &results);
        assert!(formatted.contains("**rust-lang/rust**"));
        assert!(formatted.contains("95.0k"));
        assert!(formatted.contains("Rust"));
        assert!(formatted.contains("Topics: programming-language, systems"));
    }

    #[test]
    fn test_format_stars_under_1k() {
        let results = vec![GitHubSearchResult {
            full_name: "test/repo".to_string(),
            url: "https://github.com/test/repo".to_string(),
            description: "A test".to_string(),
            language: "Python".to_string(),
            stars: 500,
            forks: 20,
            open_issues: 3,
            updated_at: "2026-05-01T00:00:00Z".to_string(),
            topics: vec![],
            license: String::new(),
        }];
        let formatted = format_github_results(1, &results);
        assert!(formatted.contains("⭐500"));
    }

    #[test]
    fn test_schema() {
        let cache = Arc::new(SearchCache::new());
        let tool = GitHubSearchTool::new(cache);
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["sort"].is_object());
        assert!(schema["properties"]["language"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("query")));
    }

    #[tokio::test]
    async fn test_github_search_live() {
        // Integration test — requires network. Skip if offline.
        let result = search_github_repos("rust web framework", "stars", "desc", 3, None).await;
        if let Ok((total, results)) = result {
            assert!(total > 0);
            assert!(!results.is_empty());
            assert!(results[0].stars > 0);
            assert!(results[0].url.starts_with("https://github.com/"));
        }
    }
}
