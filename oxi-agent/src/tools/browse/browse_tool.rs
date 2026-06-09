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
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Render a web page using the built-in headless browser.
///
/// Returns page content as markdown, html, text, or a list of links.
pub struct BrowseTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    /// Shared callback management (progress + browse progress).
    callbacks: super::callback_mixin::BrowseCallbacks,
    /// Shared slot for the current tab's ID. The agent loop creates the slot
    /// and passes it via `set_tab_id_slot`; BrowseTool writes `Some(tab_id)`
    /// when it opens a tab and `None` on close.
    tab_id_slot: Mutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}

impl BrowseTool {
    /// Create with the given engine and default config.
    pub fn new(engine: Arc<dyn BrowserEngine>) -> Self {
        Self {
            engine,
            config: BrowseConfig::default(),
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(engine: Arc<dyn BrowserEngine>, config: BrowseConfig) -> Self {
        Self {
            engine,
            config,
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
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
        self.callbacks.store_progress(callback);
    }

    fn on_browse_progress(&self, callback: Arc<dyn Fn(super::BrowseProgress) + Send + Sync>) {
        self.callbacks.store_browse(callback);
    }

    /// Sequential execution preserved for stability.
    ///
    /// Per-tab routing via `TabCallbackRegistry` now correctly routes
    /// progress events by `tab_id`, making parallel execution safe.
    /// However, `SequentialOnly` is kept for now unless a concrete
    /// multi-tab use case requires parallel browse calls.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    fn current_tab_id(&self) -> Option<uuid::Uuid> {
        *self.tab_id_slot.lock().lock()
    }

    fn set_tab_id_slot(&self, slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>>) {
        *self.tab_id_slot.lock() = slot;
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

        // Store the tab_id so the agent loop's progress callback can
        // include it in `ToolExecutionUpdate` events.
        let tab_id = raw_tab.tab_id();
        *self.tab_id_slot.lock().lock() = Some(tab_id);

        // Register the pending callbacks on this tab.
        self.callbacks.register_on_tab(raw_tab.as_ref());

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

        // Explicitly close the tab and clear the tab_id slot
        guard.close().await;
        *self.tab_id_slot.lock().lock() = None;

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
    use std::future::Future;
    use std::pin::Pin;

    /// Minimal `BrowserEngine` stub. We never call `new_tab` in the test,
    /// so the trait methods are allowed to return `Err` — the goal is just
    /// to be able to construct a `BrowseTool` and read `execution_mode()`.
    struct MockEngine;

    impl BrowserEngine for MockEngine {
        fn new_tab<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<Box<dyn BrowserTab>, BrowserError>> + Send + 'a>>
        {
            Box::pin(
                async move { Err(BrowserError::Backend("MockEngine: no real browser".into())) },
            )
        }

        fn close<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }

        fn is_alive<'a>(&'a self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async move { false })
        }
    }

    #[test]
    fn browse_tool_is_sequential_only() {
        let tool = BrowseTool::new(std::sync::Arc::new(MockEngine));
        assert!(matches!(
            tool.execution_mode(),
            crate::tools::ToolExecutionMode::SequentialOnly
        ));
    }

    #[test]
    fn browse_tool_tab_id_slot_receives_id_from_agent_loop() {
        // Simulate the agent loop's flow: set_tab_id_slot → write tab_id →
        // read current_tab_id → clear.
        let tool = BrowseTool::new(std::sync::Arc::new(MockEngine));

        // Initially no tab_id
        assert!(tool.current_tab_id().is_none());

        // Agent loop creates a slot and passes it
        let slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>> =
            Arc::new(parking_lot::Mutex::new(None));
        tool.set_tab_id_slot(Arc::clone(&slot));

        // Simulate BrowseTool::execute opening a tab
        let tab_id = uuid::Uuid::new_v4();
        *slot.lock() = Some(tab_id);

        // Agent loop's progress callback reads the slot
        assert_eq!(tool.current_tab_id(), Some(tab_id));

        // BrowseTool::execute closes the tab
        *slot.lock() = None;
        assert!(tool.current_tab_id().is_none());
    }
}
