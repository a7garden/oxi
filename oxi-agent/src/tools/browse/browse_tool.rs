//! Browse tool — render a web page and return its content.
//!
//! Opens exactly **one** tab per request and extracts all content from it.
//! Never calls engine-level methods that would open additional tabs.

use super::config::BrowseConfig;
use super::engine::BrowserEngine;
use super::helpers;
use super::tab_guard::TabGuard;
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
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
