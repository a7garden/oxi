/// Web search tool - search the web using DuckDuckGo HTML API
//!
/// Features:
/// - DuckDuckGo HTML search (no API key required)
/// - Configurable result count
/// - Title, URL, and snippet extraction
/// - Region/language support

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// Maximum number of results to return by default
const DEFAULT_MAX_RESULTS: usize = 10;

/// WebSearchTool.
pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }

    async fn search(
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

        Ok(parse_results(&html, max_results))
    }
}

/// A single search result.
#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Parse DuckDuckGo HTML search results.
fn parse_results(html: &str, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // DuckDuckGo HTML results are in <div class="result__body">
    // We use simple string-based parsing to avoid heavy HTML parser dependency
    for block in html.split("<div class=\"result__body") {
        if results.len() >= max {
            break;
        }

        // Skip the first split (before any result)
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
            results.push(SearchResult { title, url, snippet });
        }
    }

    results
}

fn extract_between<'a>(text: &'a str, start_tag: &str, end_tag: &str) -> Option<&'a str> {
    let start_idx = text.find(start_tag)?;
    let after_start = &text[start_idx + start_tag.len()..];
    // Skip the closing '>' of the opening tag
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
    // Decode common HTML entities
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

#[async_trait]
impl AgentTool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn label(&self) -> &str {
        "Web Search"
    }

    fn description(&self) -> &str {
        "Search the web using DuckDuckGo. Returns a list of results with titles, URLs, and snippets. No API key required."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query string"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 20)",
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
    ) -> Result<AgentToolResult, ToolError> {
        let query = params["query"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        let max_results = params["max_results"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_RESULTS as u64)
            .min(20) as usize;

        let results = Self::search(query, max_results).await?;

        if results.is_empty() {
            return Ok(AgentToolResult::success(format!(
                "No results found for: {}",
                query
            )));
        }

        let mut output = format!("Search results for: {}\n\n", query);
        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   {}\n\n",
                i + 1,
                result.title,
                result.url,
                result.snippet
            ));
        }

        // Also include structured JSON as metadata
        let results_json: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet
                })
            })
            .collect();

        Ok(AgentToolResult::success(output)
            .with_metadata(json!({ "results": results_json, "query": query })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_parse_results_empty() {
        let results = parse_results("<html><body>nothing</body></html>", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_results_single() {
        let html = r#"
        <div class="result__body">
            <a class="result__a" href="https://example.com">Example Title</a>
            <a class="result__url">example.com</a>
            <a class="result__snippet">This is a snippet</a>
        </div>
        "#;
        let results = parse_results(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].snippet, "This is a snippet");
    }

    #[test]
    fn test_parse_results_max() {
        let html = r#"
        <div class="result__body">
            <a class="result__a">Title 1</a>
            <a class="result__url">url1.com</a>
            <a class="result__snippet">Snippet 1</a>
        </div>
        <div class="result__body">
            <a class="result__a">Title 2</a>
            <a class="result__url">url2.com</a>
            <a class="result__snippet">Snippet 2</a>
        </div>
        <div class="result__body">
            <a class="result__a">Title 3</a>
            <a class="result__url">url3.com</a>
            <a class="result__snippet">Snippet 3</a>
        </div>
        "#;
        let results = parse_results(html, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding::encode("hello world"), "hello%20world");
        assert_eq!(urlencoding::encode("rust&cargo"), "rust%26cargo");
        assert_eq!(urlencoding::encode("abc-123"), "abc-123");
    }

    #[test]
    fn test_schema() {
        let tool = WebSearchTool::new();
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&json!("query")));
    }
}
