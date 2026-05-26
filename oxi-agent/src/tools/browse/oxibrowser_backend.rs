//! oxibrowser-core backend for the browser engine.
//!
//! Implements `BrowserEngine` and `BrowserTab` using the pure-Rust
//! `oxibrowser-core` headless browser. Only compiled with
//! `#[cfg(feature = "native-browser")]`.

use super::config::BrowseConfig;
use super::engine::{BrowserError, BrowserTab as BrowserTabTrait, PageContent};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

// ── OxiBrowserEngine ──────────────────────────────────────────────────────────

/// Browser engine powered by `oxibrowser-core`.
pub struct OxiBrowserEngine {
    browser: oxibrowser_core::Browser,
    config: BrowseConfig,
}

impl OxiBrowserEngine {
    /// Create a new engine with default config.
    pub async fn new() -> Result<Self, BrowserError> {
        Self::with_config(BrowseConfig::default()).await
    }

    /// Create a new engine with custom config.
    ///
    /// Propagates `BrowseConfig` fields (user_agent, obey_robots, js_timeout_ms)
    /// to the underlying `oxibrowser-core` `BrowserConfig`.
    pub async fn with_config(config: BrowseConfig) -> Result<Self, BrowserError> {
        let mut browser_config = oxibrowser_core::BrowserConfig::headless();

        // Propagate SDK-level settings to the browser engine
        if let Some(ref ua) = config.user_agent {
            browser_config.user_agent = ua.clone();
        }
        browser_config.obey_robots = config.obey_robots;
        browser_config.js_timeout_ms = config.js_timeout_ms;

        let browser = oxibrowser_core::Browser::new(browser_config)
            .await
            .map_err(|e| BrowserError::Backend(format!("Failed to create browser: {}", e)))?;
        Ok(Self { browser, config })
    }
}

impl Default for OxiBrowserEngine {
    fn default() -> Self {
        // Default cannot be async, so use blocking runtime.
        // Prefer `OxiBrowserEngine::new().await` in async contexts.
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(Self::new())
            .expect("Failed to create default OxiBrowserEngine")
    }
}

#[async_trait]
impl super::engine::BrowserEngine for OxiBrowserEngine {
    async fn new_tab(&self) -> Result<Box<dyn BrowserTabTrait>, BrowserError> {
        let tab = self
            .browser
            .new_tab()
            .await
            .map_err(|e| BrowserError::Backend(format!("Failed to create tab: {}", e)))?;
        Ok(Box::new(OxiTab {
            inner: tab,
            config: self.config.clone(),
        }))
    }

    async fn close(&self) -> Result<(), BrowserError> {
        self.browser
            .close()
            .await
            .map_err(|e| BrowserError::Backend(format!("Browser close failed: {}", e)))
    }

    async fn is_alive(&self) -> bool {
        self.browser.is_open()
    }
}

// ── OxiTab ────────────────────────────────────────────────────────────────────

/// A single browser tab backed by `oxibrowser-core`.
pub struct OxiTab {
    inner: oxibrowser_core::Tab,
    config: BrowseConfig,
}

#[async_trait]
impl BrowserTabTrait for OxiTab {
    async fn goto(&self, url: &str) -> Result<PageContent, BrowserError> {
        let page = self
            .inner
            .goto(url)
            .await
            .map_err(|e| BrowserError::Navigation(e.to_string()))?;
        Ok(browse_result_to_page_content(page))
    }

    async fn click(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .click(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn type_(&self, selector: &str, text: &str) -> Result<(), BrowserError> {
        self.inner
            .type_text(selector, text)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn fill(&self, selector: &str, value: &str) -> Result<(), BrowserError> {
        self.inner
            .fill(selector, value)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn press(&self, combo: &str) -> Result<(), BrowserError> {
        self.inner
            .press(combo)
            .await
            .map_err(|e| BrowserError::Evaluation(e.to_string()))
    }

    async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<(), BrowserError> {
        self.inner
            .wait_for(selector, timeout_ms)
            .await
            .map_err(|e| BrowserError::Timeout(e.to_string()))
    }

    async fn content(&self) -> Result<PageContent, BrowserError> {
        let page = self
            .inner
            .content()
            .await
            .map_err(|e| BrowserError::Backend(e.to_string()))?;
        Ok(browse_result_to_page_content(page))
    }

    async fn query_all(&self, selector: &str) -> Result<Vec<String>, BrowserError> {
        self.inner
            .query_all(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn evaluate(&self, js: &str) -> Result<Value, BrowserError> {
        self.inner
            .evaluate(js)
            .await
            .map_err(|e| BrowserError::Evaluation(e.to_string()))
    }

    async fn screenshot(&self, width: u32) -> Result<Vec<u8>, BrowserError> {
        self.inner
            .screenshot(width)
            .await
            .map_err(|e| BrowserError::Screenshot(e.to_string()))
    }

    async fn close(&self) -> Result<(), BrowserError> {
        self.inner
            .close()
            .await
            .map_err(|e| BrowserError::TabClosed(e.to_string()))
    }

    // ── Navigation — oxibrowser native history management ──────────────

    async fn back(&self) -> Result<PageContent, BrowserError> {
        let page = self
            .inner
            .back()
            .await
            .map_err(|e| BrowserError::Navigation(e.to_string()))?;
        Ok(browse_result_to_page_content(page))
    }

    async fn forward(&self) -> Result<PageContent, BrowserError> {
        let page = self
            .inner
            .forward()
            .await
            .map_err(|e| BrowserError::Navigation(e.to_string()))?;
        Ok(browse_result_to_page_content(page))
    }

    async fn reload(&self) -> Result<PageContent, BrowserError> {
        let page = self
            .inner
            .reload()
            .await
            .map_err(|e| BrowserError::Navigation(e.to_string()))?;
        Ok(browse_result_to_page_content(page))
    }

    // ── Form interaction — oxibrowser native implementations ──────────

    async fn select_option(&self, selector: &str, value: &str) -> Result<(), BrowserError> {
        self.inner
            .select_option(selector, value)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn check(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .check(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn uncheck(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .uncheck(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    // ── Advanced interaction — oxibrowser native ──────────────────────

    async fn clear(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .clear_input(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn hover(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .hover(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn double_click(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .double_click(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn right_click(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .right_click(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), BrowserError> {
        self.inner
            .scroll(delta_x, delta_y)
            .await
            .map_err(|e| BrowserError::Evaluation(e.to_string()))
    }

    async fn scroll_into_view(&self, selector: &str) -> Result<(), BrowserError> {
        self.inner
            .scroll_into_view(selector, true)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn drag(&self, from_selector: &str, to_selector: &str) -> Result<(), BrowserError> {
        self.inner
            .drag(from_selector, to_selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn upload_file(&self, selector: &str, path: &str) -> Result<(), BrowserError> {
        self.inner
            .upload_file(selector, path)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn get_value(&self, selector: &str) -> Result<String, BrowserError> {
        self.inner
            .get_value(selector)
            .await
            .map_err(|e| BrowserError::ElementNotFound(e.to_string()))
    }

    async fn evaluate_await(&self, js: &str) -> Result<Value, BrowserError> {
        self.inner
            .evaluate_await(js)
            .await
            .map_err(|e| BrowserError::Evaluation(e.to_string()))
    }

    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert an `oxibrowser_core::BrowseResult` into our portable `PageContent`.
fn browse_result_to_page_content(page: oxibrowser_core::BrowseResult) -> PageContent {
    PageContent {
        url: page.url.clone(),
        title: page.title.clone(),
        status: page.status,
        markdown: page.markdown.clone(),
        html: page.html.clone(),
    }
}
