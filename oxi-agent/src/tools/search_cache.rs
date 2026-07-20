use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::oneshot;
use crate::tools::typed::TypedTool;

// Re-export oxibrowser's SearchResult for all search tools.
pub use oxibrowser::SearchResult;

// ── Search cache ──────────────────────────────────────────────────

/// In-memory cache for search results, keyed by search ID.
#[derive(Debug)]
pub struct SearchCache {
    /// Map of search_id → (query, results).
    entries: Mutex<HashMap<String, CachedSearch>>,
    /// Maximum number of cached searches. Oldest arbitrary entry is evicted when full.
    max_entries: usize,
}

#[derive(Debug, Clone)]
struct CachedSearch {
    query: String,
    results: Vec<SearchResult>,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchCache {
    /// Create a new empty cache with default capacity (64 entries).
    pub fn new() -> Self {
        Self::with_capacity(64)
    }

    /// Create a new cache with the given maximum capacity.
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_entries,
        }
    }

    /// Insert search results and return the generated search ID.
    pub fn insert(&self, query: &str, results: Vec<SearchResult>) -> String {
        let id = generate_search_id();
        let cached = CachedSearch {
            query: query.to_string(),
            results,
        };

        let mut entries = self.entries.lock();

        // Evict oldest entries if at capacity
        while entries.len() >= self.max_entries {
            // Simple eviction: remove a random entry
            if let Some(key) = entries.keys().next().cloned() {
                entries.remove(&key);
            }
        }

        entries.insert(id.clone(), cached);
        id
    }

    /// Retrieve cached search results by ID.
    pub fn get(&self, search_id: &str) -> Option<(String, Vec<SearchResult>)> {
        let entries = self.entries.lock();
        entries
            .get(search_id)
            .map(|c| (c.query.clone(), c.results.clone()))
    }
}

/// Generate a short unique search ID.
fn generate_search_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand_part: u32 = rng::random();
    format!("{:x}{:06x}", ts, rand_part & 0xFFFFFF)
}

// ── GetSearchResultsTool ──────────────────────────────────────────

/// Tool for retrieving cached search results by ID.
pub struct GetSearchResultsTool {
    cache: Arc<SearchCache>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetSearchResultsArgs {
    #[serde(rename = "searchId")]
    search_id: String,
}

impl GetSearchResultsTool {

    /// Create a new GetSearchResultsTool with the given cache.
    pub fn new(cache: Arc<SearchCache>) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl AgentTool for GetSearchResultsTool {
    fn name(&self) -> &str {
        "get_search_results"
    }

    fn label(&self) -> &str {
        "Get Search Results"
    }

    fn description(&self) -> &str {
        "Retrieve previous search results by ID. Use this to look up results from a prior web_search call."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "searchId": {
                    "type": "string",
                    "description": "The search ID returned by a previous web_search call"
                }
            },
            "required": ["searchId"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: GetSearchResultsArgs = serde_json::from_value(params)
            .map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, _ctx).await
    }
}

#[async_trait]
impl TypedTool for GetSearchResultsTool {
    type Args = GetSearchResultsArgs;

    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let (query, results) = self.cache.get(&args.search_id)
            .ok_or_else(|| format!("Search not found for ID: {}", args.search_id))?;
        let mut output = format!("Cached results for: \"{}\"\n\n", query);
        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n", i + 1, result.title, result.url, result.snippet));
        }
        let results_json: Vec<Value> = results.iter().map(|r| {
            json!({"title": r.title, "url": r.url, "snippet": r.snippet, "source": r.source})
        }).collect();
        Ok(AgentToolResult::success(output).with_metadata(
            json!({ "results": results_json, "query": query, "searchId": args.search_id }),
        ))
    }
}
mod rng {
    use std::cell::Cell;
    use std::time::SystemTime;

    thread_local! {
        static SEED: Cell<u64> = const { Cell::new(0) };
    }

    /// Simple xorshift pseudo-random number generator.
    pub fn random() -> u32 {
        SEED.with(|s| {
            let mut x = if s.get() == 0 {
                let ns = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                ns ^ (thread_id() as u64)
            } else {
                s.get()
            };
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            s.set(x);
            (x & 0xFFFFFFFF) as u32
        })
    }

    fn thread_id() -> usize {
        thread_local! { static ANCHOR: () = const {  }; }
        ANCHOR.with(|_| &ANCHOR as *const _ as usize)
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(title: &str, url: &str, snippet: &str) -> SearchResult {
        SearchResult {
            title: title.to_string(),
            url: url.to_string(),
            snippet: snippet.to_string(),
            source: "test".to_string(),
            extra: None,
        }
    }

    #[test]
    fn test_cache_insert_and_get() {
        let cache = SearchCache::new();
        let results = vec![make_result("Test", "https://example.com", "Test snippet")];

        let id = cache.insert("test query", results.clone());
        let (query, retrieved) = cache.get(&id).unwrap();
        assert_eq!(query, "test query");
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].title, "Test");
    }

    #[test]
    fn test_cache_miss() {
        let cache = SearchCache::new();
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_cache_eviction() {
        let cache = SearchCache::with_capacity(3);

        let id1 = cache.insert("q1", vec![]);
        let id2 = cache.insert("q2", vec![]);
        let id3 = cache.insert("q3", vec![]);
        let _id4 = cache.insert("q4", vec![]);

        // At least one of the first 3 should have been evicted
        let found = [&id1, &id2, &id3]
            .iter()
            .filter(|id| cache.get(id).is_some())
            .count();
        assert!(found < 3);
        assert!(cache.get(&_id4).is_some());
    }

    #[test]
    fn test_generate_search_id_unique() {
        let id1 = generate_search_id();
        let id2 = generate_search_id();
        assert_ne!(id1, id2);
    }

    #[tokio::test]
    async fn test_get_search_results_tool() {
        let cache = Arc::new(SearchCache::new());
        let results = vec![make_result("Rust", "https://rust-lang.org", "A language")];
        let id = cache.insert("rust lang", results);

        let tool = GetSearchResultsTool::new(cache);
        let result = tool
            .execute(
                "test",
                json!({ "searchId": id }),
                None,
                &ToolContext::default(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Rust"));
        assert!(result.output.contains("rust-lang.org"));
    }

    #[tokio::test]
    async fn test_get_search_results_not_found() {
        let cache = Arc::new(SearchCache::new());
        let tool = GetSearchResultsTool::new(cache);
        let result = tool
            .execute(
                "test",
                json!({ "searchId": "bad-id" }),
                None,
                &ToolContext::default(),
            )
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn test_get_search_results_schema() {
        let cache = Arc::new(SearchCache::new());
        let tool = GetSearchResultsTool::new(cache);
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["searchId"].is_object());
    }
}
