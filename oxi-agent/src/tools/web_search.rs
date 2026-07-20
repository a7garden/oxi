use super::search_cache::{SearchCache, SearchResult};
/// Web search tool — searches via oxibrowser's integrated search module.
///
/// Uses `oxibrowser::search::dispatch()` which provides multi-engine web
/// search (DuckDuckGo, Wikipedia, Bing) and GitHub search, all powered by
/// lightweight HTTP requests — no external binary or API keys needed.
///
/// Features:
/// - Multiple search engines (ddg, wiki, bing)
/// - Result caching with search IDs for later retrieval via `get_search_results`
/// - Configurable engine selection and result count
/// - Zero-config: no API keys, no external binary needed
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::oneshot;
use crate::tools::typed::TypedTool;

/// Maximum number of results to return by default.
const DEFAULT_MAX_RESULTS: usize = 10;

/// Maximum number of results allowed.
const MAX_RESULTS: usize = 30;

/// Default search engines.
const DEFAULT_ENGINES: &str = "ddg,wiki";

/// Search timeout in seconds.
const SEARCH_TIMEOUT_SECS: u64 = 15;

#[derive(Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    query: String,
    #[serde(default = "default_web_engines")]
    engines: String,
    #[serde(default = "default_web_limit")]
    limit: u64,
}

fn default_web_engines() -> String { "ddg,wiki".to_string() }
fn default_web_limit() -> u64 { 10 }

// ── WebSearchTool ─────────────────────────────────────────────────

/// Multi-engine web search tool using oxibrowser's search module.
pub struct WebSearchTool {
    cache: Arc<SearchCache>,
}

impl WebSearchTool {
    /// Create a new WebSearchTool with the given search cache.
    pub fn new(cache: Arc<SearchCache>) -> Self {
        Self { cache }
    }

    /// Execute search using oxibrowser's dispatch.
    async fn do_search(
        &self,
        query: &str,
        engines: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let output = oxibrowser::search::dispatch(
            query,
            "web",   // source: web search
            engines, // "ddg,wiki,bing"
            None,    // repo (not used for web)
            None,    // token (not used for web)
            limit,
            SEARCH_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| format!("Search failed: {}", e))?;

        Ok(output.results)
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
            let snippet = if r.snippet.chars().count() > 200 {
                let truncated: String = r.snippet.chars().take(200).collect();
                format!("{}...", truncated)
            } else {
                r.snippet.clone()
            };
            format!("{}. **{}**\n   {}\n   {}", i + 1, r.title, r.url, snippet)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ── AgentTool impl ────────────────────────────────────────────────

#[async_trait]
impl AgentTool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn label(&self) -> &str {
        "Web Search"
    }

    fn description(&self) -> &str {
        "Search the web using multiple engines (DuckDuckGo, Wikipedia, Bing). No server or API key needed. Returns results with titles, URLs, and snippets."
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
                    "description": "Comma-separated engines (ddg,wiki,bing). Default: ddg,wiki",
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
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: WebSearchArgs = serde_json::from_value(params)
            .map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, _ctx).await
    }
}

#[async_trait]
impl TypedTool for WebSearchTool {
    type Args = WebSearchArgs;

    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let limit = args.limit.min(MAX_RESULTS as u64) as usize;
        let results = self.do_search(&args.query, &args.engines, limit).await?;
        if results.is_empty() {
            return Ok(AgentToolResult::success(format!("No results found for: {}", args.query)));
        }
        let search_id = self.cache.insert(&args.query, results.clone());
        let output = format_results(&results);
        let results_json: Vec<Value> = results.iter().map(|r| {
            json!({"title": r.title, "url": r.url, "snippet": r.snippet, "source": r.source})
        }).collect();
        Ok(AgentToolResult::success(output).with_metadata(json!({
            "searchId": search_id, "results": results_json, "query": args.query
        })))
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
            source: "DuckDuckGo".to_string(),
            extra: None,
        }];
        let formatted = format_results(&results);
        assert!(formatted.contains("**Test**"));
        assert!(formatted.contains("https://example.com"));
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
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("query"))
        );
    }
}
