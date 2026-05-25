//! Browser engine abstraction layer.

#![allow(missing_docs)]
//!
//! Defines the core traits (`BrowserEngine`, `BrowserTab`) and shared
//! types that all browser tools depend on. These traits are always compiled
//! (no feature gates) so tools can use them regardless of the backend.
//!
//! Actual backend implementations (e.g. oxibrowser-core) are behind
//! `#[cfg(feature = "native-browser")]` in `oxibrowser_backend.rs`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Errors that can occur during browser operations.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("navigation failed: {0}")]
    Navigation(String),
    #[error("element not found: {0}")]
    ElementNotFound(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("evaluation error: {0}")]
    Evaluation(String),
    #[error("screenshot failed: {0}")]
    Screenshot(String),
    #[error("tab closed: {0}")]
    TabClosed(String),
    #[error("browser error: {0}")]
    Backend(String),
}

impl From<BrowserError> for crate::tools::ToolError {
    fn from(e: BrowserError) -> Self {
        e.to_string()
    }
}

/// Shared page content returned by `goto` and `content` methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageContent {
    /// Final URL after redirects.
    pub url: String,
    /// Page title.
    pub title: String,
    /// HTTP status code.
    pub status: u16,
    /// Rendered page content as markdown.
    pub markdown: String,
    /// Raw HTML body.
    #[serde(default)]
    pub html: String,
}

impl PageContent {
    /// Create an empty page (for mock / fallback).
    pub fn empty() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            status: 0,
            markdown: String::new(),
            html: String::new(),
        }
    }
}

/// A single link on a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkInfo {
    #[allow(missing_docs)]
    pub text: String,
    #[allow(missing_docs)]
    pub href: String,
}

/// A single element matched by a CSS selector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementInfo {
    #[allow(missing_docs)]
    pub tag: String,
    #[allow(missing_docs)]
    pub text: String,
    #[serde(default)]
    #[allow(missing_docs)]
    pub attributes: HashMap<String, String>,
}

// ── BrowserTab trait ──────────────────────────────────────────────────────────

/// Operations available on a single browser tab.
///
/// Implementors handle their own async runtime; this trait only
/// defines the interface contract.
#[async_trait]
pub trait BrowserTab: Send + Sync {
    /// Navigate to `url` and return page content.
    async fn goto(&self, url: &str) -> Result<PageContent, BrowserError>;

    /// Click an element matching `selector`.
    async fn click(&self, selector: &str) -> Result<(), BrowserError>;

    /// Type text into an element matching `selector`.
    async fn type_(&self, selector: &str, text: &str) -> Result<(), BrowserError>;

    /// Fill (set value of) an element matching `selector`.
    async fn fill(&self, selector: &str, value: &str) -> Result<(), BrowserError>;

    /// Press a keyboard combo (e.g. `"Enter"`, `"Control+c"`).
    async fn press(&self, combo: &str) -> Result<(), BrowserError>;

    /// Wait for an element matching `selector` to appear.
    async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<(), BrowserError>;

    /// Get the current page content (markdown + html).
    async fn content(&self) -> Result<PageContent, BrowserError>;

    /// Get text content of all elements matching `selector`.
    async fn query_all(&self, selector: &str) -> Result<Vec<String>, BrowserError>;

    /// Evaluate a JavaScript expression and return the JSON result.
    async fn evaluate(&self, js: &str) -> Result<Value, BrowserError>;

    /// Capture a screenshot and return PNG bytes.
    async fn screenshot(&self, width: u32) -> Result<Vec<u8>, BrowserError>;

    /// Close this tab.
    async fn close(&self) -> Result<(), BrowserError>;
}

// ── BrowserEngine trait ───────────────────────────────────────────────────────

/// Factory for opening and managing browser tabs.
///
/// This trait is implemented by backends (e.g. oxibrowser-core) and
/// consumed by the tool layer via `Arc<dyn BrowserEngine>`.
#[async_trait]
pub trait BrowserEngine: Send + Sync {
    /// Fetch a URL and return page content (no tab management).
    async fn fetch(&self, url: &str) -> Result<PageContent, BrowserError> {
        let tab = self.new_tab().await?;
        let content = tab.goto(url).await;
        let _ = tab.close().await;
        content
    }

    /// Open a new browser tab and return it.
    async fn new_tab(&self) -> Result<Box<dyn BrowserTab>, BrowserError>;

    /// Close all open tabs and shut down the browser instance.
    async fn close(&self) -> Result<(), BrowserError>;

    /// Returns `true` if the browser is still alive.
    async fn is_alive(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_content_empty() {
        let p = PageContent::empty();
        assert!(p.url.is_empty());
        assert_eq!(p.status, 0);
    }

    #[test]
    fn browser_error_display() {
        let e = BrowserError::Navigation("connection refused".into());
        assert!(e.to_string().contains("navigation failed"));
    }

    #[test]
    fn link_info_serde() {
        let link = LinkInfo {
            text: "Example".into(),
            href: "https://example.com".into(),
        };
        let json = serde_json::to_string(&link).unwrap();
        let restored: LinkInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.text, "Example");
        assert_eq!(restored.href, "https://example.com");
    }

    #[test]
    fn element_info_serde() {
        let elem = ElementInfo {
            tag: "DIV".into(),
            text: "Hello".into(),
            attributes: [("class".into(), "item".into())].into(),
        };
        let json = serde_json::to_string(&elem).unwrap();
        assert!(json.contains("DIV"));
        assert!(json.contains("Hello"));
    }
}