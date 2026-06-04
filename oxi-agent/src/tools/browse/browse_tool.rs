//! Browse tool — render a web page and return its content.
//!
//! Opens exactly **one** tab per request and extracts all content from it.
//! Never calls engine-level methods that would open additional tabs.

use super::config::BrowseConfig;
use super::engine::BrowserEngine;
use super::helpers;
use super::tab_guard::TabGuard;
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Render a web page using the built-in headless browser.
///
/// Returns page content as markdown, html, text, or a list of links.
pub struct BrowseTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
}

impl BrowseTool {
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
impl AgentTool for BrowseTool {
    fn name(&self) -> &str {
        "browse"
    }

    fn label(&self) -> &str {
        "Browse"
    }

    fn description(&self) -> &str {
        "Browse a web page with a built-in headless browser. Renders JavaScript-powered \
         pages and returns content as markdown (default), html, or links. Use when \
         web_search results are insufficient and you need to read the actual page content. \
         Supports waiting for dynamic content via CSS selectors."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to browse"
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "html", "text", "links"],
                    "default": "markdown",
                    "description": "Output format: markdown (default), html, plain text, or list of links"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector to extract only matching elements"
                },
                "wait_for": {
                    "type": "string",
                    "description": "CSS selector to wait for before extracting (for JS-rendered content)"
                },
                "screenshot": {
                    "type": "boolean",
                    "default": false,
                    "description": "Include a PNG screenshot as an image block"
                }
            },
            "required": ["url"]
        })
    }

    fn on_progress(&self, callback: crate::tools::ProgressCallback) {
        // The agent loop calls this *before* `execute`. The engine's
        // background task (spawned by `OxiBrowserEngine::with_config`) will
        // invoke `callback` with each browser event's `short_label()` for
        // the duration of this tool call. The next tool call's `on_progress`
        // will replace this one — there is no fan-out.
        self.engine.progress_forwarder().set(callback);
    }

    /// Run sequentially — never in parallel with other tool calls.
    ///
    /// The `OxiBrowserEngine`'s `ProgressForwarder` is single-tenant: a
    /// single `Mutex<Option<ProgressCallback>>` shared by all callers. If
    /// two `BrowseTool::execute` calls overlapped, the second's
    /// `on_progress` would overwrite the first's callback in the forwarder,
    /// and progress events would be delivered to the wrong `tool_call_id`
    /// (events for tool A would surface on tool B's UI). Sequential mode
    /// is the simplest, safest fix: the agent loop serializes BrowseTool
    /// calls so only one is in flight at a time.
    ///
    /// Future work: a per-`tool_call_id` forwarder (or per-tab routing via
    /// a `tab_id` field on `oxibrowser_core::BrowserEvent`) is the proper
    /// long-term fix and would let BrowseTool run in parallel again.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
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

        let format = params["format"].as_str().unwrap_or("markdown");
        let selector = params["selector"].as_str();
        let wait_for = params["wait_for"].as_str();
        let want_screenshot = params["screenshot"].as_bool().unwrap_or(false);

        tracing::info!(url = %url, format = %format, "browsing page");

        // Open exactly one tab for this request
        let raw_tab = self
            .engine
            .new_tab()
            .await
            .map_err(|e| format!("Failed to open browser tab: {}", e))?;
        let guard = TabGuard::new(raw_tab);
        let tab = guard.tab();

        // Navigate
        let page = tab
            .goto(url)
            .await
            .map_err(|e| format!("Navigation failed: {}", e))?;

        // Wait for dynamic content if requested
        if let Some(sel) = wait_for {
            tab.wait_for(sel, self.config.default_wait_timeout_ms)
                .await
                .map_err(|e| format!("wait_for '{}' failed: {}", sel, e))?;
        }

        // Build output — all from the same tab
        let output = match format {
            "html" => {
                if let Some(sel) = selector {
                    tab.query_all(sel)
                        .await
                        .map_err(|e| e.to_string())?
                        .join("\n\n")
                } else {
                    page.html.clone()
                }
            }
            "links" => {
                let links = helpers::extract_links(tab).await?;
                helpers::format_links(&links)
            }
            "text" => {
                if let Some(sel) = selector {
                    tab.query_all(sel)
                        .await
                        .map_err(|e| e.to_string())?
                        .join("\n")
                } else {
                    page.markdown.clone()
                }
            }
            _ => {
                // "markdown" (default)
                if let Some(sel) = selector {
                    tab.query_all(sel)
                        .await
                        .map_err(|e| e.to_string())?
                        .join("\n\n")
                } else {
                    page.markdown.clone()
                }
            }
        };

        let title = page.title.clone();
        let final_url = page.url.clone();
        let status = page.status;

        // Screenshot from the same tab (no re-render)
        let screenshot_blocks = if want_screenshot {
            match tab.screenshot(self.config.screenshot_width).await {
                Ok(png) => {
                    let b64 =
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
                    let img =
                        oxi_ai::ContentBlock::Image(oxi_ai::ImageContent::new(b64, "image/png"));
                    Some(vec![img])
                }
                Err(e) => {
                    tracing::warn!("screenshot failed for {}: {}", final_url, e);
                    None
                }
            }
        } else {
            None
        };

        // Explicitly close the tab
        guard.close().await;

        let mut result = AgentToolResult::success(output).with_metadata(json!({
            "url": final_url,
            "title": title,
            "status": status,
        }));

        if let Some(blocks) = screenshot_blocks {
            result = result.with_content_blocks(blocks);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browse::engine::{BrowserError, BrowserTab};
    use async_trait::async_trait;

    /// Minimal `BrowserEngine` stub. We never call `new_tab` in the test,
    /// so the trait methods are allowed to return `Err` — the goal is just
    /// to be able to construct a `BrowseTool` and read `execution_mode()`.
    struct MockEngine;

    #[async_trait]
    impl BrowserEngine for MockEngine {
        async fn new_tab(&self) -> Result<Box<dyn BrowserTab>, BrowserError> {
            Err(BrowserError::Backend("MockEngine: no real browser".into()))
        }

        async fn close(&self) -> Result<(), BrowserError> {
            Ok(())
        }

        async fn is_alive(&self) -> bool {
            false
        }
    }

    #[test]
    fn browse_tool_is_sequential_only() {
        // The BrowseTool must run sequentially because the OxiBrowserEngine's
        // progress forwarder is single-tenant. If two BrowseTool executions
        // ran in parallel, the second's on_progress would overwrite the first's
        // callback and progress events would be routed to the wrong tool_call_id.
        let tool = BrowseTool::new(std::sync::Arc::new(MockEngine));
        assert!(matches!(
            tool.execution_mode(),
            crate::tools::ToolExecutionMode::SequentialOnly
        ));
    }
}
