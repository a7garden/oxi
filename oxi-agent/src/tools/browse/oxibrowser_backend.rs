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
    pub fn new() -> Result<Self, BrowserError> {
        Self::with_config(BrowseConfig::default())
    }

    /// Create a new engine with custom config.
    pub fn with_config(config: BrowseConfig) -> Result<Self, BrowserError> {
        let browser = oxibrowser_core::Browser::new()
            .map_err(|e| BrowserError::Backend(format!("Failed to create browser: {}", e)))?;
        Ok(Self { browser, config })
    }
}

impl Default for OxiBrowserEngine {
    fn default() -> Self {
        Self::new().expect("Failed to create default OxiBrowserEngine")
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
        // oxibrowser-core browser cleanup happens on drop
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        true // If we can hold a reference, it's alive
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

        Ok(PageContent {
            url: page.url.clone(),
            title: page.title.clone().unwrap_or_default(),
            status: page.status.unwrap_or(200),
            markdown: page.content.clone().unwrap_or_default(),
            html: page.html.clone().unwrap_or_default(),
        })
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

        Ok(PageContent {
            url: page.url.clone(),
            title: page.title.clone().unwrap_or_default(),
            status: page.status.unwrap_or(200),
            markdown: page.content.clone().unwrap_or_default(),
            html: page.html.clone().unwrap_or_default(),
        })
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
}
