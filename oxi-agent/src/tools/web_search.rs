/// Web search tool — searches via a3s-search binary (DuckDuckGo, Wikipedia, Bing, Brave).
///
/// Falls back to direct DuckDuckGo HTML scraping when a3s-search is unavailable.
///
/// Features:
/// - Multiple search engines (ddg, wiki, bing, brave)
/// - Result caching with search IDs for later retrieval via `get_search_results`
/// - Configurable engine selection and result count
/// - Zero-config: no API keys or server needed

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;

use super::search_cache::SearchCache;

/// Maximum number of results to return by default
const DEFAULT_MAX_RESULTS: usize = 10;

/// Maximum number of results allowed
const MAX_RESULTS: usize = 30;

/// Default search engines
const DEFAULT_ENGINES: &str = "ddg,wiki";

// ── a3s-search binary resolution ──────────────────────────────────

/// Find the a3s-search binary. Returns `None` if not found.
pub fn find_a3s_binary() -> Option<PathBuf> {
    // 1. A3S_SEARCH_BIN env var
    if let Ok(env_bin) = std::env::var("A3S_SEARCH_BIN") {
        let p = PathBuf::from(&env_bin);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. ~/.cargo/bin/a3s-search
    if let Some(home) = dirs::home_dir() {
        let cargo_bin = home.join(".cargo").join("bin"). join("a3s-search");
        if cargo_bin.exists() {
            return Some(cargo_bin);
        }
    }

    // 3. PATH lookup via `which`
    if let Ok(output) = std::process::Command::new("which")
        .arg("a3s-search")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }

    None
}

// ── Result types ──────────────────────────────────────────────────

/// A single search result from any engine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Short snippet / description.
    pub snippet: String,
    /// Which engines returned this result.
    #[serde(default)]
    pub engines: Vec<String>,
    /// Relevance score from a3s-search.
    #[serde(default)]
    pub score: f64,
}

// ── a3s-search output parsing ─────────────────────────────────────

/// Parse a3s-search CLI output into structured results.
fn parse_a3s_output(raw: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut current: Option<PartialResult> = None;

    for line in raw.lines() {
        let trimmed = line.trim();

        // Skip header line like "Search results for ..."
        if trimmed.starts_with("Search results for") {
            continue;
        }

        // Match result header: "N. Title"
        if let Some(caps) = regex::Regex::new(r"^(\d+)\.\s+(.+)$")
            .ok()
            .and_then(|re| re.captures(trimmed))
        {
            // Flush previous
            if let Some(prev) = current.take() {
                if !prev.title.is_empty() && !prev.url.is_empty() {
                    results.push(prev.finalize());
                }
            }
            current = Some(PartialResult {
                title: caps[2].to_string(),
                url: String::new(),
                snippet: String::new(),
                engines: Vec::new(),
                score: 0.0,
            });
            continue;
        }

        let Some(ref mut cur) = current else { continue };

        // URL line: "URL: ..." or "↳ ..."
        if trimmed.starts_with("URL:") || trimmed.starts_with("↳") {
            cur.url = trimmed
                .trim_start_matches("URL:")
                .trim_start_matches("↳")
                .trim()
                .to_string();
            continue;
        }

        // Engines line: "Engines: {DuckDuckGo} | Score: 1.20"
        if trimmed.starts_with("Engines:") {
            if let Some(engines_str) = trimmed.strip_prefix("Engines:") {
                let engines_part = engines_str.split('|').next().unwrap_or("");
                // Parse engines like {"DuckDuckGo", "Wikipedia"}
                cur.engines = parse_engine_names(engines_part);
            }
            // Score extraction
            if let Some(score_str) = trimmed.split("Score:").nth(1) {
                cur.score = score_str.trim().parse().unwrap_or(0.0);
            }
            continue;
        }

        // Snippet (indented non-empty content after URL)
        if !trimmed.is_empty() && !cur.url.is_empty() && !trimmed.starts_with("Score:") {
            cur.snippet = if cur.snippet.is_empty() {
                trimmed.to_string()
            } else {
                format!("{} {}", cur.snippet, trimmed)
            };
        }
    }

    // Flush last
    if let Some(prev) = current.take() {
        if !prev.title.is_empty() && !prev.url.is_empty() {
            results.push(prev.finalize());
        }
    }

    results
}

/// Partial result during parsing.
struct PartialResult {
    title: String,
    url: String,
    snippet: String,
    engines: Vec<String>,
    score: f64,
}

impl PartialResult {
    fn finalize(self) -> SearchResult {
        SearchResult {
            title: self.title,
            url: self.url,
            snippet: self.snippet,
            engines: self.engines,
            score: self.score,
        }
    }
}

/// Parse engine names from a3s output like `{"DuckDuckGo", "Wikipedia"}`.
fn parse_engine_names(s: &str) -> Vec<String> {
    s.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split(',')
        .map(|e| e.trim().trim_matches('"').to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

// ── Search execution ──────────────────────────────────────────────

/// Search using a3s-search binary.
async fn search_a3s(
    query: &str,
    engines: &str,
    limit: usize,
    signal: Option<oneshot::Receiver<()>>,
) -> Result<Vec<SearchResult>, ToolError> {
    let bin = find_a3s_binary()
        .ok_or_else(|| "a3s-search binary not found. Install with: cargo install a3s-search --no-default-features".to_string())?;

    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg(query)
        .arg("-e")
        .arg(engines)
        .arg("-l")
        .arg(limit.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Set up timeout / cancellation
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        async {
            let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn a3s-search: {}", e))?;

            // If we have a cancellation signal, select on it
            let output_result = if let Some(mut sig) = signal {
                tokio::select! {
                    result = child.wait_with_output() => {
                        result.map_err(|e| format!("a3s-search execution failed: {}", e))
                    }
                    _ = &mut sig => {
                        // wait_with_output takes ownership, so we can't kill after move.
                        // Just return cancelled — the child will be reaped on drop.
                        Err("Search cancelled".to_string())
                    }
                }
            } else {
                child.wait_with_output().await.map_err(|e| format!("a3s-search execution failed: {}", e))
            };
            output_result
        },
    )
    .await
    .map_err(|_| "Search timed out after 30 seconds".to_string())??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("a3s-search error: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_a3s_output(&stdout))
}

// ── Fallback: DuckDuckGo HTML scraping ────────────────────────────

/// Direct DuckDuckGo HTML search as fallback when a3s-search is unavailable.
async fn search_duckduckgo(
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchResult>, ToolError> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Search request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Search returned status {}", response.status()));
    }

    let html = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    Ok(parse_ddg_html(&html, max_results))
}

/// Parse DuckDuckGo HTML search results.
fn parse_ddg_html(html: &str, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    for block in html.split("<div class=\"result__body") {
        if results.len() >= max {
            break;
        }

        if !block.contains("result__a") {
            continue;
        }

        let title = extract_between(block, "class=\"result__a\"", "</a>")
            .map(|s| strip_html_tags(s).trim().to_string())
            .unwrap_or_default();

        let url = extract_between(block, "class=\"result__url\"", "</a>")
            .map(|s| strip_html_tags(s).trim().to_string())
            .or_else(|| extract_href(block))
            .unwrap_or_default();

        let snippet = extract_between(block, "class=\"result__snippet\"", "</a>")
            .or_else(|| extract_between(block, "class=\"result__snippet\"", "</td>"))
            .map(|s| strip_html_tags(s).trim().to_string())
            .unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
                engines: vec!["DuckDuckGo".to_string()],
                score: 0.0,
            });
        }
    }

    results
}

fn extract_between<'a>(text: &'a str, start_tag: &str, end_tag: &str) -> Option<&'a str> {
    let start_idx = text.find(start_tag)?;
    let after_start = &text[start_idx + start_tag.len()..];
    let content_start = after_start.find('>')?;
    let content = &after_start[content_start + 1..];
    let end_idx = content.find(end_tag)?;
    Some(&content[..end_idx])
}

fn extract_href(text: &str) -> Option<String> {
    let href_start = text.find("href=\"")?;
    let after = &text[href_start + 6..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result = result.replace("&amp;", "&");
    result = result.replace("&lt;", "<");
    result = result.replace("&gt;", ">");
    result = result.replace("&quot;", "\"");
    result = result.replace("&#39;", "'");
    result = result.replace("&nbsp;", " ");
    result
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
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
}

// ── Formatting ────────────────────────────────────────────────────

/// Format search results for display.
fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found.".to_string();
    }
    results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let snippet = if r.snippet.len() > 200 {
                format!("{}...", &r.snippet[..200])
            } else {
                r.snippet.clone()
            };
            format!(
                "{}. **{}**\n   {}\n   {}",
                i + 1,
                r.title,
                r.url,
                snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ── WebSearchTool ─────────────────────────────────────────────────

/// Multi-engine web search tool using a3s-search.
pub struct WebSearchTool {
    cache: Arc<SearchCache>,
}

impl WebSearchTool {
    /// Create a new WebSearchTool with the given search cache.
    pub fn new(cache: Arc<SearchCache>) -> Self {
        Self { cache }
    }

    /// Execute search, trying a3s-search first, falling back to DuckDuckGo HTML.
    async fn do_search(
        &self,
        query: &str,
        engines: &str,
        limit: usize,
        signal: Option<oneshot::Receiver<()>>,
    ) -> Result<Vec<SearchResult>, ToolError> {
        // Try a3s-search first
        if find_a3s_binary().is_some() {
            match search_a3s(query, engines, limit, signal).await {
                Ok(results) => return Ok(results),
                Err(e) => {
                    tracing::warn!("a3s-search failed, falling back to DuckDuckGo: {}", e);
                }
            }
        }

        // Fallback to DuckDuckGo HTML
        search_duckduckgo(query, limit).await
    }
}

#[async_trait]
impl AgentTool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn label(&self) -> &str {
        "Web Search"
    }

    fn description(&self) -> &str {
        "Search the web using a3s-search (DuckDuckGo, Wikipedia, Bing, Brave). No server needed. Returns results with titles, URLs, and snippets."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query string"
                },
                "engines": {
                    "type": "string",
                    "description": "Comma-separated engines (ddg,wiki,bing,brave). Default: ddg,wiki",
                    "default": "ddg,wiki"
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
        signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let query = params["query"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        let engines = params["engines"]
            .as_str()
            .unwrap_or(DEFAULT_ENGINES);

        let limit = params["limit"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_RESULTS as u64)
            .min(MAX_RESULTS as u64) as usize;

        let results = self.do_search(query, engines, limit, signal).await?;

        if results.is_empty() {
            return Ok(AgentToolResult::success(format!(
                "No results found for: {}",
                query
            )));
        }

        // Cache results and generate a search ID
        let search_id = self.cache.insert(query, results.clone());

        let output = format_results(&results);

        // Build structured metadata
        let results_json: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet,
                    "engines": r.engines,
                    "score": r.score
                })
            })
            .collect();

        Ok(AgentToolResult::success(output).with_metadata(json!({
            "results": results_json,
            "query": query,
            "searchId": search_id,
            "resultCount": results.len()
        })))
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_a3s_output() {
        let raw = "Search results for \"rust programming\" (3 results in 100ms):\n\n\
             1. Rust (programming language)\n\
             URL: https://en.wikipedia.org/wiki/Rust_(programming_language)\n\
             Rust is a general-purpose programming language...\n\
             Engines: {\"Wikipedia\"} | Score: 1.20\n\
             \n\
             2. Rust Book\n\
             URL: https://doc.rust-lang.org/book/\n\
             The Rust Programming Language book\n\
             Engines: {\"DuckDuckGo\"} | Score: 0.80\n";

        let results = parse_a3s_output(raw);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert_eq!(results[0].url, "https://en.wikipedia.org/wiki/Rust_(programming_language)");
        assert_eq!(results[0].engines, vec!["Wikipedia"]);
        assert!((results[0].score - 1.2).abs() < f64::EPSILON);
        assert_eq!(results[1].title, "Rust Book");
    }

    #[test]
    fn test_parse_a3s_empty() {
        let raw = "Search results for \"xyznonexistent\":\n\nNo results found.\n";
        let results = parse_a3s_output(raw);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_engine_names() {
        assert_eq!(
            parse_engine_names("{\"DuckDuckGo\", \"Wikipedia\"}"),
            vec!["DuckDuckGo", "Wikipedia"]
        );
        assert_eq!(parse_engine_names("{\"Brave\"}"), vec!["Brave"]);
        assert_eq!(parse_engine_names(""), Vec::<String>::new());
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<b>hello</b>"), "hello");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(
            strip_html_tags("<span class=\"x\">text &amp; more</span>"),
            "text & more"
        );
    }

    #[test]
    fn test_extract_between() {
        let html = "before<div class=\"result__a\">Title Text</a>after";
        let result = extract_between(html, "class=\"result__a\"", "</a>");
        assert_eq!(result, Some("Title Text"));
    }

    #[test]
    fn test_extract_href() {
        let html = "<a href=\"https://example.com\">link</a>";
        assert_eq!(extract_href(html), Some("https://example.com".to_string()));
    }

    #[test]
    fn test_parse_ddg_html_empty() {
        let results = parse_ddg_html("<html><body>nothing</body></html>", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_ddg_html_single() {
        let html = r#"
        <div class="result__body">
            <a class="result__a" href="https://example.com">Example Title</a>
            <a class="result__url">example.com</a>
            <a class="result__snippet">This is a snippet</a>
        </div>
        "#;
        let results = parse_ddg_html(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].snippet, "This is a snippet");
    }

    #[test]
    fn test_format_results_empty() {
        assert_eq!(format_results(&[]), "No results found.");
    }

    #[test]
    fn test_format_results() {
        let results = vec![SearchResult {
            title: "Test".to_string(),
            url: "https://example.com".to_string(),
            snippet: "A snippet".to_string(),
            engines: vec!["DuckDuckGo".to_string()],
            score: 1.0,
        }];
        let formatted = format_results(&results);
        assert!(formatted.contains("**Test**"));
        assert!(formatted.contains("https://example.com"));
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding::encode("hello world"), "hello%20world");
        assert_eq!(urlencoding::encode("rust&cargo"), "rust%26cargo");
        assert_eq!(urlencoding::encode("abc-123"), "abc-123");
    }

    #[test]
    fn test_schema() {
        let cache = Arc::new(SearchCache::new());
        let tool = WebSearchTool::new(cache);
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["engines"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&json!("query")));
    }
}
