//! Interactive browser session tool — persistent tab across tool calls.
//!
//! Manages a single browser tab that persists between `execute()` calls,
//! enabling multi-step workflows where the agent can reason between actions.
//! Uses `TabGuard` for RAII cleanup on drop.

use super::config::BrowseConfig;
use super::engine::{BrowserEngine, BrowserError};
use super::helpers;
use super::tab_guard::TabGuard;

use crate::tools::typed::TypedTool;
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use parking_lot::Mutex as SyncMutex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, oneshot};
/// Typed arguments for [`BrowseSessionTool`].
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct BrowseSessionArgs {
    action: String,
    url: Option<String>,
    selector: Option<String>,
    value: Option<String>,
    combo: Option<String>,
    #[serde(default = "default_pixels")]
    pixels: u64,
    javascript: Option<String>,
    #[serde(default = "default_format")]
    format: String,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_wait_condition")]
    wait_condition: String,
    #[serde(rename = "fromSelector")]
    from_selector: Option<String>,
    #[serde(rename = "toSelector")]
    to_selector: Option<String>,
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    #[serde(default = "default_width")]
    width: u64,
}

fn default_pixels() -> u64 {
    300
}
fn default_format() -> String {
    "markdown".to_string()
}
fn default_timeout_ms() -> u64 {
    10000
}
fn default_wait_condition() -> String {
    "network_idle".to_string()
}
fn default_width() -> u64 {
    800
}

/// Interactive browser session with a persistent tab across calls.
///
/// Open a session, perform multiple operations (goto, click, fill, etc.),
/// read page content between steps, then close when done. The tab retains
/// cookies, localStorage, and DOM state between actions.
pub struct BrowseSessionTool {
    engine: Arc<dyn BrowserEngine>,
    tab: Arc<Mutex<Option<TabGuard>>>,
    config: BrowseConfig,
    last_action: Arc<Mutex<Option<Instant>>>,
    /// Shared callback management (progress + browse progress).
    callbacks: super::callback_mixin::BrowseCallbacks,
    /// Shared slot for the current tab's ID. The agent loop creates the
    /// slot and passes it via `set_tab_id_slot`; the tool writes
    /// `Some(tab_id)` on open and `None` on close.
    tab_id_slot: SyncMutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}

impl BrowseSessionTool {
    /// Create with the given engine and default config.
    pub fn new(engine: Arc<dyn BrowserEngine>) -> Self {
        Self {
            engine,
            tab: Arc::new(Mutex::new(None)),
            config: BrowseConfig::default(),
            last_action: Arc::new(Mutex::new(None)),
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: SyncMutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(engine: Arc<dyn BrowserEngine>, config: BrowseConfig) -> Self {
        Self {
            engine,
            tab: Arc::new(Mutex::new(None)),
            config,
            last_action: Arc::new(Mutex::new(None)),
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: SyncMutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }

    /// Update the last-action timestamp to now.
    async fn touch(&self) {
        *self.last_action.lock().await = Some(Instant::now());
    }

    /// Check idle timeout. Returns Ok if session is still valid or
    /// if idle timeout is disabled (0). Auto-closes stale sessions.
    async fn check_idle_timeout(&self) -> Result<(), ToolError> {
        if self.config.session_idle_timeout_secs == 0 {
            return Ok(());
        }
        let elapsed = {
            let last = self.last_action.lock().await;
            match *last {
                Some(instant) => instant.elapsed().as_secs(),
                None => return Ok(()), // No action yet, session is fresh
            }
        };
        if elapsed >= self.config.session_idle_timeout_secs {
            // Auto-close stale session
            let mut slot = self.tab.lock().await;
            if let Some(guard) = slot.take() {
                tracing::warn!(
                    elapsed_secs = elapsed,
                    timeout_secs = self.config.session_idle_timeout_secs,
                    "browse_session: auto-closing stale session"
                );
                guard.close().await;
                // Clear the tab_id slot since the tab is gone
                *self.tab_id_slot.lock().lock() = None;
            }
            return Err(format!(
                "Session timed out after {}s of inactivity",
                elapsed
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentTool for BrowseSessionTool {
    fn name(&self) -> &str {
        "browse_session"
    }

    fn label(&self) -> &str {
        "Browser Session"
    }

    fn description(&self) -> &str {
        "Interactive browser session with a persistent tab across calls. \
         Open a session, perform multiple operations, then close when done. \
         The tab retains cookies, localStorage, and DOM state between actions. \
         Use for multi-step interactions like form filling, login flows, and \
         SPA exploration where reasoning is needed between steps."
    }

    fn on_progress(&self, callback: crate::tools::ProgressCallback) {
        // If a tab is already open, register directly on the engine's
        // callback registry so browser events from the next action
        // route to this callback (which carries the current tool_call_id).
        let tab_id = self.current_tab_id();
        if let Some(tid) = tab_id {
            self.callbacks.store_progress(callback);
            self.callbacks
                .register_progress_on_registry(tid, self.engine.callback_registry().as_ref());
        } else {
            self.callbacks.store_progress(callback);
        }
    }

    fn on_browse_progress(&self, callback: Arc<dyn Fn(super::BrowseProgress) + Send + Sync>) {
        let tab_id = self.current_tab_id();
        if let Some(tid) = tab_id {
            self.callbacks.store_browse(callback);
            self.callbacks
                .register_browse_on_registry(tid, self.engine.callback_registry().as_ref());
        } else {
            self.callbacks.store_browse(callback);
        }
    }

    fn set_tab_id_slot(&self, slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>>) {
        *self.tab_id_slot.lock() = slot;
    }

    fn current_tab_id(&self) -> Option<uuid::Uuid> {
        *self.tab_id_slot.lock().lock()
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "open",
                        "goto",
                        "back",
                        "forward",
                        "reload",
                        "click",
                        "fill",
                        "type",
                        "clear",
                        "press",
                        "select",
                        "check",
                        "uncheck",
                        "scroll",
                        "scroll_into_view",
                        "hover",
                        "double_click",
                        "right_click",
                        "drag",
                        "upload_file",
                        "wait_for",
                        "wait",
                        "observe",
                        "content",
                        "query_all",
                        "extract_links",
                        "evaluate",
                        "evaluate_await",
                        "get_value",
                        "screenshot",
                        "close"
                    ],
                    "description": "Session action to perform"
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (goto action)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector (click, fill, type, clear, select, check, uncheck, wait_for, query_all, extract_links)"
                },
                "value": {
                    "type": "string",
                    "description": "Value to fill/type/select (fill, type, select actions)"
                },
                "combo": {
                    "type": "string",
                    "description": "Key combo (press action, e.g. 'Enter', 'Control+a')"
                },
                "pixels": {
                    "type": "integer",
                    "description": "Scroll distance in pixels (scroll action, positive = down)"
                },
                "javascript": {
                    "type": "string",
                    "description": "JS expression to evaluate (evaluate, evaluate_await actions)"
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "html", "text", "links"],
                    "default": "markdown",
                    "description": "Output format for content action"
                },
                "timeout_ms": {
                    "type": "integer",
                    "default": 10000,
                    "description": "Timeout in ms (wait_for action)"
                },
                "wait_condition": {
                    "type": "string",
                    "enum": ["network_idle", "dom_content_loaded", "load"],
                    "default": "network_idle",
                    "description": "Structured wait condition (wait action): network_idle, dom_content_loaded, or load"
                },
                "from_selector": {
                    "type": "string",
                    "description": "Source CSS selector (drag action)"
                },
                "to_selector": {
                    "type": "string",
                    "description": "Target CSS selector (drag action)"
                },
                "file_path": {
                    "type": "string",
                    "description": "Local file path to upload (upload_file action)"
                },
                "width": {
                    "type": "integer",
                    "default": 800,
                    "description": "Viewport width for screenshot (default: 800)"
                }
            },
            "required": ["action"]
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: BrowseSessionArgs =
            serde_json::from_value(params).map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, _ctx).await
    }
}

#[async_trait]
impl TypedTool for BrowseSessionTool {
    type Args = BrowseSessionArgs;

    #[allow(clippy::too_many_lines)]
    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let params = serde_json::to_value(&args).map_err(|e| format!("serialize: {e}"))?;
        self.dispatch_action(params).await
    }
}

impl BrowseSessionTool {
    /// Inner dispatch that takes raw JSON params — reused by both
    /// AgentTool::execute and TypedTool::execute_typed.
    #[allow(clippy::too_many_lines)]
    async fn dispatch_action(&self, params: Value) -> Result<AgentToolResult, ToolError> {
        let action = params["action"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: action".to_string())?;

        let url = params["url"].as_str();
        let selector = params["selector"].as_str();
        let value = params["value"].as_str();
        let combo = params["combo"].as_str();
        let pixels = params["pixels"].as_u64().unwrap_or(300);
        let javascript = params["javascript"].as_str();
        let format = params["format"].as_str().unwrap_or("markdown");
        let timeout_ms = params["timeout_ms"]
            .as_u64()
            .unwrap_or(self.config.default_wait_timeout_ms);
        let width = params["width"]
            .as_u64()
            .unwrap_or(self.config.screenshot_width as u64) as u32;
        let from_selector = params["from_selector"].as_str();
        let to_selector = params["to_selector"].as_str();
        let file_path = params["file_path"].as_str();

        tracing::info!(action = %action, "browse_session action");

        self.touch().await;

        match action {
            // ── Lifecycle ────────────────────────────────────────────
            "open" => {
                let mut slot = self.tab.lock().await;
                if let Some(old_guard) = slot.take() {
                    tracing::warn!("browse_session: closing previous session on re-open");
                    old_guard.close().await;
                }
                let raw_tab = self
                    .engine
                    .new_tab()
                    .await
                    .map_err(|e| format!("Failed to open browser tab: {}", e))?;

                let tab_id = raw_tab.tab_id();
                *self.tab_id_slot.lock().lock() = Some(tab_id);

                self.callbacks
                    .register_on_registry(tab_id, self.engine.callback_registry().as_ref());

                let guard = TabGuard::new(raw_tab);
                *slot = Some(guard);
                Ok(json_ok())
            }

            "close" => {
                let mut slot = self.tab.lock().await;
                match slot.take() {
                    Some(guard) => {
                        guard.close().await;
                        *self.tab_id_slot.lock().lock() = None;
                        Ok(json_ok())
                    }
                    None => Ok(json_error("no active session to close")),
                }
            }

            // ── Navigation ──────────────────────────────────────────
            "goto" => {
                self.check_idle_timeout().await?;
                let url = url.ok_or_else(|| "Missing required parameter: url".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let page = tab.goto(url).await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "url": page.url,
                    "title": page.title,
                    "status_code": page.status,
                }))))
            }

            "back" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let _ = tab.evaluate("history.back()").await;
                let page = tab.content().await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "url": page.url,
                    "title": page.title,
                }))))
            }

            "forward" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let _ = tab.evaluate("history.forward()").await;
                let page = tab.content().await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "url": page.url,
                    "title": page.title,
                }))))
            }

            "reload" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let _ = tab.evaluate("location.reload()").await;
                let page = tab.content().await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "url": page.url,
                    "title": page.title,
                }))))
            }

            // ── DOM interaction ─────────────────────────────────────
            "click" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.click(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "fill" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let val = value.ok_or_else(|| "Missing required parameter: value".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.fill(sel, val).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "type" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let val = value.ok_or_else(|| "Missing required parameter: value".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.type_(sel, val).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "clear" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.clear(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "press" => {
                self.check_idle_timeout().await?;
                let c = combo.ok_or_else(|| "Missing required parameter: combo".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.press(c).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "select" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let val = value.ok_or_else(|| "Missing required parameter: value".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.select_option(sel, val).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "check" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.check(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "uncheck" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.uncheck(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "scroll" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.scroll(0.0, pixels as f64).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            // ── Wait ────────────────────────────────────────────────
            "wait_for" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.wait_for(sel, timeout_ms).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "wait" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let cond = match params["wait_condition"].as_str().unwrap_or("network_idle") {
                    "dom_content_loaded" => super::engine::BrowseWaitCondition::DomContentLoaded,
                    "load" => super::engine::BrowseWaitCondition::Load,
                    _ => super::engine::BrowseWaitCondition::NetworkIdle,
                };
                tab.wait_for_condition(&cond, timeout_ms)
                    .await
                    .map_err(browser_err)?;
                Ok(json_ok())
            }

            // ── Observe ─────────────────────────────────────────────
            "observe" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let obs = tab.observe().await.map_err(browser_err)?;
                Ok(AgentToolResult::success(
                    serde_json::to_string_pretty(&obs).unwrap_or_default(),
                ))
            }

            // ── Read ────────────────────────────────────────────────
            "content" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let page = tab.content().await.map_err(browser_err)?;

                let content = match format {
                    "html" => {
                        if let Some(sel) = selector {
                            tab.query_all(sel).await.map_err(browser_err)?.join("\n\n")
                        } else {
                            page.html.clone()
                        }
                    }
                    "links" => {
                        let links = if let Some(sel) = selector {
                            let js = helpers::js_links_within(sel);
                            let value = tab.evaluate(&js).await.map_err(browser_err)?;
                            helpers::parse_link_values(value)
                        } else {
                            helpers::extract_links(tab)
                                .await
                                .map_err(|e: ToolError| e)?
                        };
                        helpers::format_links(&links)
                    }
                    "text" => {
                        if let Some(sel) = selector {
                            tab.query_all(sel).await.map_err(browser_err)?.join("\n")
                        } else {
                            page.markdown.clone()
                        }
                    }
                    _ => {
                        if let Some(sel) = selector {
                            tab.query_all(sel).await.map_err(browser_err)?.join("\n\n")
                        } else {
                            page.markdown.clone()
                        }
                    }
                };

                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "url": page.url,
                    "title": page.title,
                    "content": content,
                }))))
            }

            "query_all" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let results = tab.query_all(sel).await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "results": results,
                }))))
            }

            "extract_links" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;

                let links = if let Some(sel) = selector {
                    let js = helpers::js_links_within(sel);
                    let value = tab.evaluate(&js).await.map_err(browser_err)?;
                    helpers::parse_link_values(value)
                } else {
                    helpers::extract_links(tab)
                        .await
                        .map_err(|e: ToolError| e)?
                };

                let json_links: Vec<Value> = links
                    .iter()
                    .map(|(text, href)| json!({ "text": text, "href": href }))
                    .collect();

                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "links": json_links,
                }))))
            }

            // ── Evaluate ────────────────────────────────────────────
            "evaluate" => {
                self.check_idle_timeout().await?;
                let js = javascript
                    .ok_or_else(|| "Missing required parameter: javascript".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let result_val = tab.evaluate(js).await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "result": result_val,
                }))))
            }

            "evaluate_await" => {
                self.check_idle_timeout().await?;
                let js = javascript
                    .ok_or_else(|| "Missing required parameter: javascript".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let result_val = tab.evaluate_await(js).await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "result": result_val,
                }))))
            }

            // ── Screenshot ──────────────────────────────────────────
            "screenshot" => {
                self.check_idle_timeout().await?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let png = tab.screenshot(width).await.map_err(browser_err)?;
                let size_bytes = png.len();
                let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
                let img = oxi_ai::ContentBlock::Image(oxi_ai::ImageContent::new(b64, "image/png"));

                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "size_bytes": size_bytes,
                })))
                .with_content_blocks(vec![img]))
            }

            // ── Extended DOM actions ──────────────────────────────
            "scroll_into_view" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.scroll_into_view(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "hover" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.hover(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "double_click" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.double_click(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "right_click" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.right_click(sel).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "drag" => {
                self.check_idle_timeout().await?;
                let from = from_selector
                    .ok_or_else(|| "Missing required parameter: from_selector".to_string())?;
                let to = to_selector
                    .ok_or_else(|| "Missing required parameter: to_selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.drag(from, to).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "upload_file" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let path =
                    file_path.ok_or_else(|| "Missing required parameter: file_path".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                tab.upload_file(sel, path).await.map_err(browser_err)?;
                Ok(json_ok())
            }

            "get_value" => {
                self.check_idle_timeout().await?;
                let sel =
                    selector.ok_or_else(|| "Missing required parameter: selector".to_string())?;
                let slot = self.tab.lock().await;
                let tab = require_tab(&slot)?;
                let result_val = tab.get_value(sel).await.map_err(browser_err)?;
                Ok(AgentToolResult::success(json_str(&json!({
                    "status": "ok",
                    "value": result_val,
                }))))
            }

            _ => Err(format!(
                "Unknown action: '{}'. Valid actions: open, goto, back, forward, reload, \
                     click, fill, type, clear, press, select, check, uncheck, scroll, \
                     scroll_into_view, hover, double_click, right_click, drag, upload_file, \
                     wait_for, wait, content, observe, query_all, extract_links, evaluate, \
                     evaluate_await, get_value, screenshot, close",
                action
            )),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Get a reference to the tab from the locked slot, or return an error.
fn require_tab(slot: &Option<TabGuard>) -> Result<&dyn super::engine::BrowserTab, ToolError> {
    match slot {
        Some(guard) => Ok(guard.tab()),
        None => Err(BrowserError::NoActiveSession.into()),
    }
}

/// Serialize a JSON value to a pretty string.
fn json_str(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Create a JSON success result.
fn json_ok() -> AgentToolResult {
    AgentToolResult::success(json_str(&json!({ "status": "ok" })))
}

/// Create a JSON error result (still `success: true` — error is in the payload).
fn json_error(msg: &str) -> AgentToolResult {
    AgentToolResult::success(json_str(&json!({
        "status": "error",
        "error": msg,
    })))
}

/// Convert a `BrowserError` into a `ToolError`.
fn browser_err(e: BrowserError) -> ToolError {
    e.to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browse::engine::{BrowserError, PageContent};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    // ── Mock tab for unit tests ─────────────────────────────────

    struct MockTab {
        closed: Arc<AtomicBool>,
    }

    impl MockTab {
        fn new() -> (Self, Arc<AtomicBool>) {
            let closed = Arc::new(AtomicBool::new(false));
            (
                Self {
                    closed: closed.clone(),
                },
                closed,
            )
        }
    }

    #[async_trait]
    impl super::super::engine::BrowserTab for MockTab {
        async fn goto(&self, _url: &str) -> Result<PageContent, BrowserError> {
            Ok(PageContent {
                url: "https://example.com".into(),
                title: "Example".into(),
                status: 200,
                markdown: "# Example\nHello".into(),
                html: "<h1>Example</h1>".into(),
            })
        }
        async fn click(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn type_(&self, _selector: &str, _text: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn fill(&self, _selector: &str, _value: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn press(&self, _combo: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn wait_for(&self, _selector: &str, _timeout_ms: u64) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn content(&self) -> Result<PageContent, BrowserError> {
            Ok(PageContent {
                url: "https://example.com".into(),
                title: "Example".into(),
                status: 200,
                markdown: "# Example\nHello".into(),
                html: "<h1>Example</h1>".into(),
            })
        }
        async fn query_all(&self, _selector: &str) -> Result<Vec<String>, BrowserError> {
            Ok(vec!["item1".into(), "item2".into()])
        }
        async fn evaluate(&self, _js: &str) -> Result<Value, BrowserError> {
            Ok(Value::String("ok".into()))
        }
        async fn screenshot(&self, _width: u32) -> Result<Vec<u8>, BrowserError> {
            Ok(vec![0x89, 0x50, 0x4E, 0x47]) // PNG magic bytes
        }
        async fn close(&self) -> Result<(), BrowserError> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn back(&self) -> Result<PageContent, BrowserError> {
            Ok(PageContent::empty())
        }
        async fn forward(&self) -> Result<PageContent, BrowserError> {
            Ok(PageContent::empty())
        }
        async fn reload(&self) -> Result<PageContent, BrowserError> {
            Ok(PageContent::empty())
        }
        async fn select_option(&self, _selector: &str, _value: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn check(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn uncheck(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn hover(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn double_click(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn right_click(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn scroll_into_view(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn drag(&self, _from_selector: &str, _to_selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn upload_file(&self, _selector: &str, _path: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn get_value(&self, _selector: &str) -> Result<String, BrowserError> {
            Ok("mock_value".into())
        }
        async fn evaluate_await(&self, _js: &str) -> Result<Value, BrowserError> {
            Ok(Value::String("ok".into()))
        }
    }

    // ── Mock engine ─────────────────────────────────────────────

    struct MockEngine;

    #[async_trait]
    impl super::super::engine::BrowserEngine for MockEngine {
        async fn new_tab(&self) -> Result<Box<dyn super::super::engine::BrowserTab>, BrowserError> {
            let (tab, _) = MockTab::new();
            Ok(Box::new(tab) as Box<dyn super::super::engine::BrowserTab>)
        }
        async fn close(&self) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn is_alive(&self) -> bool {
            true
        }
    }

    /// Create a tool with a mock engine for testing.
    fn make_tool() -> BrowseSessionTool {
        let engine: Arc<dyn BrowserEngine> = Arc::new(MockEngine);
        BrowseSessionTool::new(engine)
    }

    // ── Tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_open_close_lifecycle() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        let result = tool
            .execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("ok"));

        let result = tool
            .execute("c2", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_goto_requires_open_session() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        let result = tool
            .execute(
                "c1",
                json!({"action": "goto", "url": "https://example.com"}),
                None,
                &ctx,
            )
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no active session")
        );
    }

    #[tokio::test]
    async fn test_open_goto_close() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        let result = tool
            .execute(
                "c2",
                json!({"action": "goto", "url": "https://example.com"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("example.com"));
        assert!(result.output.contains("200"));

        let result = tool
            .execute("c3", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_content_action() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();
        tool.execute(
            "c2",
            json!({"action": "goto", "url": "https://example.com"}),
            None,
            &ctx,
        )
        .await
        .unwrap();

        let result = tool
            .execute(
                "c3",
                json!({"action": "content", "format": "markdown"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Example"));
        assert!(result.output.contains("Hello"));

        tool.execute("c4", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_query_all_action() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        let result = tool
            .execute(
                "c2",
                json!({"action": "query_all", "selector": ".item"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("item1"));
        assert!(result.output.contains("item2"));

        tool.execute("c3", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_evaluate_action() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        let result = tool
            .execute(
                "c2",
                json!({"action": "evaluate", "javascript": "document.title"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("ok"));

        tool.execute("c3", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_screenshot_action() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        let result = tool
            .execute("c2", json!({"action": "screenshot"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("size_bytes"));
        assert!(result.content_blocks.is_some());

        tool.execute("c3", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_dom_actions() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        let actions: Vec<(&str, Value)> = vec![
            ("click", json!({"action": "click", "selector": "#btn"})),
            (
                "fill",
                json!({"action": "fill", "selector": "#input", "value": "hello"}),
            ),
            (
                "type",
                json!({"action": "type", "selector": "#input", "value": "world"}),
            ),
            ("clear", json!({"action": "clear", "selector": "#input"})),
            ("press", json!({"action": "press", "combo": "Enter"})),
            ("check", json!({"action": "check", "selector": "#agree"})),
            (
                "uncheck",
                json!({"action": "uncheck", "selector": "#newsletter"}),
            ),
            ("scroll", json!({"action": "scroll", "pixels": 500})),
            (
                "wait_for",
                json!({"action": "wait_for", "selector": ".loaded"}),
            ),
            (
                "scroll_into_view",
                json!({"action": "scroll_into_view", "selector": "#section"}),
            ),
            ("hover", json!({"action": "hover", "selector": "#menu"})),
            (
                "double_click",
                json!({"action": "double_click", "selector": "#item"}),
            ),
            (
                "right_click",
                json!({"action": "right_click", "selector": "#item"}),
            ),
            (
                "get_value",
                json!({"action": "get_value", "selector": "#input"}),
            ),
        ];

        for (name, params) in &actions {
            let result = tool.execute("cx", params.clone(), None, &ctx).await;
            assert!(result.is_ok(), "Action '{}' failed: {:?}", name, result);
        }

        tool.execute("c99", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_navigation_actions() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        for nav_action in &["back", "forward", "reload"] {
            let result = tool
                .execute("cx", json!({"action": *nav_action}), None, &ctx)
                .await;
            assert!(result.is_ok(), "Navigation action '{}' failed", nav_action);
        }

        tool.execute("c99", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        let result = tool
            .execute("c1", json!({"action": "nonexistent"}), None, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_close_without_open() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        let result = tool
            .execute("c1", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("error"));
    }

    #[tokio::test]
    async fn test_re_open_closes_previous() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        let result = tool
            .execute("c2", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();
        assert!(result.success);

        let result = tool
            .execute(
                "c3",
                json!({"action": "goto", "url": "https://example.com"}),
                None,
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_missing_required_params() {
        let tool = make_tool();
        let ctx = ToolContext::default();

        tool.execute("c1", json!({"action": "open"}), None, &ctx)
            .await
            .unwrap();

        // goto without url
        assert!(
            tool.execute("c2", json!({"action": "goto"}), None, &ctx)
                .await
                .is_err()
        );

        // click without selector
        assert!(
            tool.execute("c3", json!({"action": "click"}), None, &ctx)
                .await
                .is_err()
        );

        // fill without value
        assert!(
            tool.execute(
                "c4",
                json!({"action": "fill", "selector": "#x"}),
                None,
                &ctx
            )
            .await
            .is_err()
        );

        // press without combo
        assert!(
            tool.execute("c5", json!({"action": "press"}), None, &ctx)
                .await
                .is_err()
        );

        // evaluate without javascript
        assert!(
            tool.execute("c6", json!({"action": "evaluate"}), None, &ctx)
                .await
                .is_err()
        );

        tool.execute("c7", json!({"action": "close"}), None, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_name_label_description() {
        let tool = make_tool();
        assert_eq!(tool.name(), "browse_session");
        assert_eq!(tool.label(), "Browser Session");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_schema_has_all_actions() {
        let tool = make_tool();
        let schema = tool.parameters_schema();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        assert_eq!(actions.len(), 31);
    }
}
