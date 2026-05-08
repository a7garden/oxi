/// Web search tool — searches via a3s-search library (DuckDuckGo, Wikipedia, Bing, Brave).
///
/// Uses a3s-search as a Rust library (not CLI), so no external binary is needed.
/// Results are structured types — no text parsing required.
///
/// Features:
/// - Multiple search engines (ddg, wiki, bing, brave)
/// - Result caching with search IDs for later retrieval via `get_search_results`
/// - Configurable engine selection and result count
/// - Zero-config: no API keys, no external binary needed

use super::{AgentTool, AgentToolResult, ToolError};
use super::search_cache::{SearchCache, SearchResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Maximum number of results to return by default.
const DEFAULT_MAX_RESULTS: usize = 10;

/// Maximum number of results allowed.
const MAX_RESULTS: usize = 30;

/// Default search engines.
const DEFAULT_ENGINES: &str = "ddg,wiki";

// ── Engine shortcut → a3s engine mapping ──────────────────────────

/// Build a3s-search engine instances from shortcuts and add to Search.
fn add_engines(search: &mut a3s_search::Search, shortcuts: &str) {
    for shortcut in shortcuts.split(',') {
        match shortcut.trim() {
            "ddg" => search.add_engine(a3s_search::engines::DuckDuckGo::new()),
            "wiki" => search.add_engine(a3s_search::engines::Wikipedia::new()),
            "bing" => search.add_engine(a3s_search::engines::Bing::new()),
            "brave" => search.add_engine(a3s_search::engines::Brave::new()),
            s if !s.is_empty() => tracing::warn!("Unknown search engine: {}", s),
            _ => {}
        }
    }
}

// ── WebSearchTool ─────────────────────────────────────────────────

/// Multi-engine web search tool using a3s-search library.
pub struct WebSearchTool {
    cache: Arc<SearchCache>,
}

impl WebSearchTool {
    /// Create a new WebSearchTool with the given search cache.
    pub fn new(cache: Arc<SearchCache>) -> Self {
        Self { cache }
    }

    /// Execute search using a3s-search library.
    async fn do_search(
        &self,
        query: &str,
        engines: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, ToolError> {
        let mut search = a3s_search::Search::new();
        add_engines(&mut search, engines);

        if search.engine_count() == 0 {
            return Err(
                "No valid engines specified. Available: ddg, wiki, bing, brave".to_string()
            );
        }

        search.set_timeout(std::time::Duration::from_secs(15));

        let a3s_query = a3s_search::SearchQuery::new(query);
        let results = search
            .search(a3s_query)
            .await
            .map_err(|e| format!("Search failed: {}", e))?;

        let formatted: Vec<SearchResult> = results
            .items()
            .iter()
            .take(limit)
            .map(|r| SearchResult {
                title: r.title.clone(),
                url: r.url.clone(),
                snippet: r.content.clone(),
                engines: r.engines.iter().cloned().collect(),
                score: r.score,
            })
            .collect();

        // Log engine errors if any
        for (engine, error) in results.errors() {
            tracing::warn!("Search engine {} error: {}", engine, error);
        }

        Ok(formatted)
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
        "Search the web using multiple engines (DuckDuckGo, Wikipedia, Bing, Brave). No server or API key needed. Returns results with titles, URLs, and snippets."
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
        _signal: Option<oneshot::Receiver<()>>,
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

        let results = self.do_search(query, engines, limit).await?;

        if results.is_empty() {
            return Ok(AgentToolResult::success(format!(
                "No results found for: {}",
                query
            )));
        }

        // Cache results and generate a search ID
        let search_id = self.cache.insert(query, results.clone());

        let output = format_results(&results);

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
    fn test_add_engines_ddg() {
        let mut search = a3s_search::Search::new();
        add_engines(&mut search, "ddg");
        assert_eq!(search.engine_count(), 1);
    }

    #[test]
    fn test_add_engines_multiple() {
        let mut search = a3s_search::Search::new();
        add_engines(&mut search, "ddg,wiki,brave");
        assert_eq!(search.engine_count(), 3);
    }

    #[test]
    fn test_add_engines_unknown() {
        let mut search = a3s_search::Search::new();
        add_engines(&mut search, "ddg,unknown,wiki");
        assert_eq!(search.engine_count(), 2);
    }

    #[test]
    fn test_add_engines_empty() {
        let mut search = a3s_search::Search::new();
        add_engines(&mut search, "");
        assert_eq!(search.engine_count(), 0);
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
