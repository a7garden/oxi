//! Browse extract tool — extract structured data from a web page.
//!
//! All extraction is done via the already-loaded tab's JavaScript engine —
//! no engine-level methods that would open additional tabs.

use super::config::BrowseConfig;
use super::engine::{BrowserEngine, BrowserError, BrowserTab};
use super::helpers;
use super::tab_guard::TabGuard;
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Extract structured data from a web page using CSS selectors.
///
/// Returns links, text content, or element metadata for all elements
/// matching the given CSS selector.
pub struct BrowseExtractTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
}

impl BrowseExtractTool {
    /// Create with the given engine and default config.
    pub fn new(engine: Arc<dyn BrowserEngine>) -> Self {
        Self {
            engine,
            config: BrowseConfig::default(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(engine: Arc<dyn BrowserEngine>, config: BrowseConfig) -> Self {
        Self { engine, config }
    }
}

#[async_trait]
impl AgentTool for BrowseExtractTool {
    fn name(&self) -> &str {
        "browse_extract"
    }

    fn label(&self) -> &str {
        "Extract Page Data"
    }

    fn description(&self) -> &str {
        "Extract structured data from a web page: links, text content, or elements matching \
         a CSS selector. Use when you need specific data from a page rather than the full content. \
         Supports extracting all matching elements or just the first match."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL of the page to extract from"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector to match elements"
                },
                "extract": {
                    "type": "string",
                    "enum": ["links", "text", "elements", "markdown"],
                    "default": "text",
                    "description": "What to extract: 'links' (href + text), 'text' (textContent), 'elements' (tag + text + attrs), 'markdown' (innerHTML as markdown)"
                },
                "all": {
                    "type": "boolean",
                    "default": true,
                    "description": "Return all matches (true) or just the first (false)"
                },
                "timeout": {
                    "type": "integer",
                    "default": 30,
                    "description": "Maximum time in seconds"
                }
            },
            "required": ["url", "selector"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let url = params["url"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: url".to_string())?;

        let selector = params["selector"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: selector".to_string())?;

        let extract = params["extract"].as_str().unwrap_or("text");
        let all = params["all"].as_bool().unwrap_or(true);
        let timeout_secs = params["timeout"]
            .as_u64()
            .unwrap_or(self.config.page_timeout_secs);

        tracing::info!(url = %url, selector = %selector, extract = %extract, "extracting page data");

        // Wrap the entire operation in a timeout
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.extract_from_new_tab(url, selector, extract, all),
        )
        .await
        .map_err(|_| format!("Extract timed out after {}s", timeout_secs))??;

        Ok(output)
    }
}

impl BrowseExtractTool {
    /// Open a tab, navigate, extract, close — one tab, one flow.
    async fn extract_from_new_tab(
        &self,
        url: &str,
        selector: &str,
        extract: &str,
        all: bool,
    ) -> Result<AgentToolResult, ToolError> {
        let raw_tab = self
            .engine
            .new_tab()
            .await
            .map_err(|e| format!("Failed to open browser tab: {}", e))?;
        let guard = TabGuard::new(raw_tab);

        let page = guard
            .tab()
            .goto(url)
            .await
            .map_err(|e| format!("Navigation failed: {}", e))?;

        let output = extract_from_tab(guard.tab(), selector, extract, all)
            .await
            .map_err(|e: BrowserError| e.to_string())?;

        let metadata_url = page.url.clone();
        let metadata_title = page.title.clone();

        guard.close().await;

        Ok(AgentToolResult::success(output).with_metadata(json!({
            "url": metadata_url,
            "title": metadata_title,
            "selector": selector,
            "extract": extract,
        })))
    }
}

// ── Extraction logic ──────────────────────────────────────────────────────────

async fn extract_from_tab(
    tab: &dyn BrowserTab,
    selector: &str,
    extract: &str,
    all: bool,
) -> Result<String, BrowserError> {
    match extract {
        "links" => {
            let js = helpers::js_links_within(selector);
            let value = tab.evaluate(&js).await?;
            let links = helpers::parse_link_values(value);
            let links = if all {
                links
            } else {
                links.into_iter().take(1).collect()
            };
            let json_links: Vec<Value> = links
                .iter()
                .map(|(t, h)| json!({ "text": t, "href": h }))
                .collect();
            Ok(serde_json::to_string_pretty(&json_links).unwrap_or_default())
        }
        "elements" => {
            let js = helpers::js_query_elements(selector);
            let value = tab.evaluate(&js).await?;
            let elements = helpers::parse_element_values(value);
            let elements = if all {
                elements
            } else {
                elements.into_iter().take(1).collect()
            };
            let json_elems: Vec<Value> = elements
                .iter()
                .map(|(tag, text, attrs)| json!({ "tag": tag, "text": text, "attributes": attrs }))
                .collect();
            Ok(serde_json::to_string_pretty(&json_elems).unwrap_or_default())
        }
        "markdown" => {
            let texts = tab.query_all(selector).await?;
            let texts = if all {
                texts
            } else {
                texts.into_iter().take(1).collect()
            };
            Ok(texts.join("\n\n"))
        }
        _ => {
            // "text" (default)
            let texts = tab.query_all(selector).await?;
            let texts = if all {
                texts
            } else {
                texts.into_iter().take(1).collect()
            };
            Ok(texts.join("\n"))
        }
    }
}
