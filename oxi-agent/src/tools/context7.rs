//! Context7 documentation tools.
//!
//! Built-in tools that call the Context7 API directly (no MCP).
//! - `context7_resolve_library_id`: Search for libraries and get a Context7-compatible ID
//! - `context7_query_docs`: Fetch up-to-date documentation for a library
//!
//! API reference: <https://context7.com/api>
//! Configuration: `CONTEXT7_API_KEY` env var or agent config.

use crate::tools::{AgentTool, AgentToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::oneshot;

// ── Constants ────────────────────────────────────────────────────────

const API_BASE_URL: &str = "https://context7.com/api";

// ── Shared API client ───────────────────────────────────────────────

/// Resolve the API key from environment or config.
fn api_key() -> Option<String> {
    std::env::var("CONTEXT7_API_KEY").ok()
}

/// Build an authenticated reqwest client.
fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

// ── API types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<LibraryResult>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LibraryResult {
    id: String,
    title: String,
    description: String,
    total_snippets: Option<u64>,
    benchmark_score: Option<u64>,
    versions: Option<Vec<String>>,
    trust_score: Option<f64>,
}

// ── Tool 1: resolve-library-id ───────────────────────────────────────

/// Resolve a library name to a Context7-compatible library ID.
pub struct Context7ResolveLibraryIdTool;

impl Context7ResolveLibraryIdTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentTool for Context7ResolveLibraryIdTool {
    fn name(&self) -> &str {
        "context7_resolve-library-id"
    }

    fn label(&self) -> &str {
        "Context7: Resolve Library ID"
    }

    fn description(&self) -> &str {
        "Resolves a package/product name to a Context7-compatible library ID and returns matching libraries.\n\n\
         You MUST call this function before 'Query Documentation' tool to obtain a valid Context7-compatible library ID UNLESS the user explicitly provides a library ID in the format '/org/project' or '/org/project/version' in their query.\n\n\
         Each result includes:\n\
         - Library ID: Context7-compatible identifier (format: /org/project)\n\
         - Name: Library or package name\n\
         - Description: Short summary\n\
         - Code Snippets: Number of available code examples\n\
         - Source Reputation: Authority indicator (High, Medium, Low, or Unknown)\n\
         - Benchmark Score: Quality indicator (100 is the highest score)\n\
         - Versions: List of versions if available. Use one of those versions if the user provides a version in their query. The format of the version is /org/project/version.\n\n\
         For best results, select libraries based on name match, source reputation, snippet coverage, benchmark score, and relevance to your use case.\n\n\
         Selection Process:\n\
         1. Analyze the query to understand what library/package the user is looking for\n\
         2. Return the most relevant match based on:\n\
            - Name similarity to the query (exact matches prioritized)\n\
            - Description relevance to the query's intent\n\
            - Documentation coverage (prioritize libraries with higher Code Snippet counts)\n\
            - Source reputation (consider libraries with High or Medium reputation more authoritative)\n\
            - Benchmark Score: Quality indicator (100 is the highest score)\n\n\
         IMPORTANT: Do not call this tool more than 3 times per question. If you cannot find what you need after 3 calls, use the best result you have."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The question or task you need help with. This is used to rank library results by relevance to what the user is trying to accomplish."
                },
                "libraryName": {
                    "type": "string",
                    "description": "Library name to search for and retrieve a Context7-compatible library ID. Use the official library name with proper punctuation — e.g. 'Next.js' instead of 'nextjs', 'Customer.io' instead of 'customerio', 'Three.js' instead of 'threejs'."
                }
            },
            "required": ["query", "libraryName"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, String> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: query")?
            .to_string();
        let library_name = params
            .get("libraryName")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: libraryName")?
            .to_string();

        let client = build_client();
        let mut request = client
            .get(format!("{}/v2/libs/search", API_BASE_URL))
            .query(&[("query", query.as_str()), ("libraryName", library_name.as_str())]);

        if let Some(ref key) = api_key() {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Context7 API request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let msg = match status.as_u16() {
                429 => "Rate limited or quota exceeded. Set CONTEXT7_API_KEY for higher limits.".to_string(),
                401 => "Invalid API key. Check CONTEXT7_API_KEY.".to_string(),
                _ => format!("Context7 API error ({}): {}", status, body.chars().take(200).collect::<String>()),
            };
            return Ok(AgentToolResult::error(msg));
        }

        let search: SearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Context7 response: {}", e))?;

        if let Some(error) = search.error {
            return Ok(AgentToolResult::error(error));
        }

        if search.results.is_empty() {
            return Ok(AgentToolResult::success(format!(
                "No libraries found matching \"{}\". Try a different search term.",
                library_name
            )));
        }

        // Format results
        let mut text = String::from("Available Libraries:\n\n");
        for lib in &search.results {
            text.push_str(&format!("**{}**\n", lib.title));
            text.push_str(&format!("  Library ID: {}\n", lib.id));
            if let Some(snippets) = lib.total_snippets {
                text.push_str(&format!("  Code Snippets: {}\n", snippets));
            }
            if let Some(score) = lib.benchmark_score {
                text.push_str(&format!("  Benchmark Score: {}/100\n", score));
            }
            if let Some(trust) = lib.trust_score {
                let label = if trust >= 0.8 {
                    "High"
                } else if trust >= 0.5 {
                    "Medium"
                } else {
                    "Low"
                };
                text.push_str(&format!("  Source Reputation: {}\n", label));
            }
            if let Some(ref versions) = lib.versions {
                if !versions.is_empty() {
                    text.push_str(&format!("  Versions: {}\n", versions.join(", ")));
                }
            }
            text.push_str(&format!("  {}\n\n", lib.description));
        }

        Ok(AgentToolResult::success(text.trim_end().to_string()))
    }
}

// ── Tool 2: query-docs ──────────────────────────────────────────────

/// Query up-to-date documentation from Context7.
pub struct Context7QueryDocsTool;

impl Context7QueryDocsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentTool for Context7QueryDocsTool {
    fn name(&self) -> &str {
        "context7_query-docs"
    }

    fn label(&self) -> &str {
        "Context7: Query Documentation"
    }

    fn description(&self) -> &str {
        "Retrieves and queries up-to-date documentation and code examples from Context7 for any programming library or framework.\n\n\
         You must call 'Resolve Context7 Library ID' tool first to obtain the exact Context7-compatible library ID required to use this tool, UNLESS the user explicitly provides a library ID in the format '/org/project' or '/org/project/version' in their query.\n\n\
         Workflow: call first without researchMode. If that doesn't answer the question, retry with researchMode: true. Do not call each tool more than 3 times per question"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "libraryId": {
                    "type": "string",
                    "description": "Exact Context7-compatible library ID (e.g. '/mongodb/docs', '/vercel/next.js', '/supabase/supabase', '/vercel/next.js/v14.3.0-canary.87') retrieved from 'resolve-library-id' or directly from user query in the format '/org/project' or '/org/project/version'."
                },
                "query": {
                    "type": "string",
                    "description": "The question or task you need help with. Be specific and include relevant details. Good: 'How to set up authentication with JWT in Express.js' or 'React useEffect cleanup function examples'. Bad: 'auth' or 'hooks'."
                },
                "researchMode": {
                    "type": "boolean",
                    "description": "Retry the query with deep research: spins up sandboxed agents that read the actual source repos and runs a live web search, then synthesizes a fresh answer. Set true on retry if you weren't satisfied with the first answer and want a more thorough one. Requires an API key — you can get one free at https://context7.com/."
                }
            },
            "required": ["libraryId", "query"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, String> {
        let library_id = params
            .get("libraryId")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: libraryId")?
            .to_string();
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: query")?
            .to_string();
        let research_mode = params
            .get("researchMode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let client = build_client();
        let mut request = client
            .get(format!("{}/v2/context", API_BASE_URL))
            .query(&[
                ("query", query.as_str()),
                ("libraryId", library_id.as_str()),
            ]);

        if let Some(ref key) = api_key() {
            request = request.bearer_auth(key);
        }

        if research_mode {
            request = request.query(&[("researchMode", "true")]);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Context7 API request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let msg = match status.as_u16() {
                404 => format!(
                    "Library '{}' not found. Use context7_resolve-library-id to get a valid ID.",
                    library_id
                ),
                429 => "Rate limited or quota exceeded. Set CONTEXT7_API_KEY for higher limits.".to_string(),
                401 => "Invalid API key. Check CONTEXT7_API_KEY.".to_string(),
                _ => format!("Context7 API error ({}): {}", status, body.chars().take(200).collect::<String>()),
            };
            return Ok(AgentToolResult::error(msg));
        }

        let text = response
            .text()
            .await
            .map_err(|e| format!("Failed to read Context7 response: {}", e))?;

        if text.is_empty() {
            return Ok(AgentToolResult::success(format!(
                "No documentation found for library \"{}\". \
                 This might be because the library ID is invalid. \
                 Use context7_resolve-library-id to get a valid ID.",
                library_id
            )));
        }

        Ok(AgentToolResult::success(text))
    }
}
