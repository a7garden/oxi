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
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

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
    #[error("no active session — call 'open' first")]
    NoActiveSession,
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

    /// Navigate back in history. Returns the rendered page content.
    async fn back(&self) -> Result<PageContent, BrowserError>;

    /// Navigate forward in history. Returns the rendered page content.
    async fn forward(&self) -> Result<PageContent, BrowserError>;

    /// Reload the current page. Returns the rendered page content.
    async fn reload(&self) -> Result<PageContent, BrowserError>;

    /// Select an option in a `<select>` element.
    async fn select_option(&self, selector: &str, value: &str) -> Result<(), BrowserError>;

    /// Check a checkbox or radio input.
    async fn check(&self, selector: &str) -> Result<(), BrowserError>;

    /// Uncheck a checkbox or radio input.
    async fn uncheck(&self, selector: &str) -> Result<(), BrowserError>;

    // ── Advanced interaction ───────────────────────────────────

    /// Clear the value of an input element.
    async fn clear(&self, selector: &str) -> Result<(), BrowserError> {
        self.fill(selector, "").await
    }

    /// Hover over an element.
    async fn hover(&self, selector: &str) -> Result<(), BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var el = document.querySelector({sel}); if (!el) return null; el.dispatchEvent(new MouseEvent('mouseover', {{bubbles:true}})); return el.tagName; }})()"#
        );
        self.evaluate(&js).await.map(|_| ())
    }

    /// Double-click an element.
    async fn double_click(&self, selector: &str) -> Result<(), BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var el = document.querySelector({sel}); if (!el) return null; el.dispatchEvent(new MouseEvent('dblclick', {{bubbles:true}})); return el.tagName; }})()"#
        );
        self.evaluate(&js).await.map(|_| ())
    }

    /// Right-click (context menu) an element.
    async fn right_click(&self, selector: &str) -> Result<(), BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var el = document.querySelector({sel}); if (!el) return null; el.dispatchEvent(new MouseEvent('contextmenu', {{bubbles:true, button:2}})); return el.tagName; }})()"#
        );
        self.evaluate(&js).await.map(|_| ())
    }

    /// Scroll the page by delta pixels.
    async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), BrowserError> {
        let js = format!("window.scrollBy({}, {})", delta_x, delta_y);
        self.evaluate(&js).await.map(|_| ())
    }

    /// Scroll an element into view.
    async fn scroll_into_view(&self, selector: &str) -> Result<(), BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var el = document.querySelector({sel}); if (!el) return null; el.scrollIntoView(); return el.tagName; }})()"#
        );
        self.evaluate(&js).await.map(|_| ())
    }

    /// Drag from one element to another.
    async fn drag(&self, from_selector: &str, to_selector: &str) -> Result<(), BrowserError> {
        let from_sel = serde_json::to_string(from_selector).unwrap_or_default();
        let to_sel = serde_json::to_string(to_selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var src = document.querySelector({from_sel}); var dst = document.querySelector({to_sel}); if (!src || !dst) return null; src.dispatchEvent(new DragEvent('dragstart', {{bubbles:true}})); dst.dispatchEvent(new DragEvent('drop', {{bubbles:true}})); src.dispatchEvent(new DragEvent('dragend', {{bubbles:true}})); return 'ok'; }})()"#
        );
        self.evaluate(&js).await.map(|_| ())
    }

    /// Upload a file to a file input element.
    async fn upload_file(&self, selector: &str, path: &str) -> Result<(), BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let p = serde_json::to_string(path).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var el = document.querySelector({sel}); if (!el || el.type !== 'file') return null; if (typeof DataTransfer === 'undefined') return null; var dt = new DataTransfer(); var f = new File([], {p}.split('/').pop()); dt.items.add(f); el.files = dt.files; el.dispatchEvent(new Event('change', {{bubbles:true}})); return el.tagName; }})()"#
        );
        self.evaluate(&js).await.map(|_| ())
    }

    /// Get the value or text content of an element.
    async fn get_value(&self, selector: &str) -> Result<String, BrowserError> {
        let sel = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{ var el = document.querySelector({sel}); if (!el) return null; return (el.value !== undefined ? el.value : el.textContent) || ''; }})()"#
        );
        let val = self.evaluate(&js).await?;
        Ok(val.as_str().unwrap_or("").to_string())
    }

    /// Evaluate JS that may return a promise; awaits by default.
    async fn evaluate_await(&self, js: &str) -> Result<Value, BrowserError> {
        self.evaluate(js).await
    }

    /// Returns `true` if this tab has been closed.
    fn is_closed(&self) -> bool {
        false
    }
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

    /// Access the engine's progress forwarder.
    ///
    /// Tools (e.g. `BrowseTool`) call `engine.progress_forwarder().set(cb)`
    /// from their `on_progress` implementation. Backends that stream
    /// lifecycle events (oxibrowser-core) spawn a background task in their
    /// constructor that drains the browser's event channel and invokes
    /// the currently-installed callback. Backends without event streaming
    /// return an empty forwarder — `set` becomes a no-op, no events fire.
    ///
    /// Default implementation returns a fresh empty forwarder.
    fn progress_forwarder(&self) -> Arc<ProgressForwarder> {
        Arc::new(ProgressForwarder::new())
    }
}

// ── ProgressForwarder ─────────────────────────────────────────────────────

/// Shared slot for a single progress callback.
///
/// Holds at most one `ProgressCallback` at a time. Tools set the callback
/// before `execute`; a backend's background task reads it and invokes it
/// with each event's `short_label()`. Replacing the callback (the next
/// tool call) drops the old one — there is no fan-out.
///
/// The default forwarder (returned by the trait's default `progress_forwarder`)
/// is a no-op: `invoke` silently drops messages. This is the right
/// behaviour for backends that don't stream events.
pub struct ProgressForwarder {
    callback: Mutex<Option<crate::tools::ProgressCallback>>,
}

impl Default for ProgressForwarder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressForwarder {
    /// Create an empty forwarder.
    pub fn new() -> Self {
        Self {
            callback: Mutex::new(None),
        }
    }

    /// Install (or replace) the callback. Called from `AgentTool::on_progress`.
    pub fn set(&self, cb: crate::tools::ProgressCallback) {
        *self.callback.lock() = Some(cb);
    }

    /// Remove the callback. Subsequent `invoke` calls become no-ops until
    /// the next `set`.
    pub fn clear(&self) {
        *self.callback.lock() = None;
    }

    /// Invoke the currently-installed callback, if any. Never panics; never
    /// blocks. If the callback itself panics, the panic propagates (we do
    /// not silently swallow — that would hide bugs in user code).
    pub fn invoke(&self, msg: String) {
        if let Some(cb) = self.callback.lock().as_ref() {
            cb(msg);
        }
    }

    /// Whether a callback is currently installed. Useful for tests.
    pub fn is_set(&self) -> bool {
        self.callback.lock().is_some()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn progress_forwarder_default_is_empty() {
        let pf = ProgressForwarder::new();
        assert!(!pf.is_set());
        // invoke on empty forwarder is a silent no-op
        pf.invoke("should be dropped".into());
    }

    #[test]
    fn progress_forwarder_set_and_invoke() {
        let pf = ProgressForwarder::new();
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        pf.set(oxi_ai::progress_callback(move |msg: String| {
            assert_eq!(msg, "hello");
            count_clone.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(pf.is_set());

        pf.invoke("hello".into());
        pf.invoke("hello".into());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn progress_forwarder_set_replaces_previous() {
        let pf = ProgressForwarder::new();
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let ca = Arc::clone(&count_a);
        pf.set(oxi_ai::progress_callback(move |_| {
            ca.fetch_add(1, Ordering::SeqCst);
        }));
        pf.invoke("first".into());
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 0);

        // Replace with a new callback — the old one is dropped.
        let cb_clone = Arc::clone(&count_b);
        pf.set(oxi_ai::progress_callback(move |_| {
            cb_clone.fetch_add(1, Ordering::SeqCst);
        }));
        pf.invoke("second".into());
        assert_eq!(count_a.load(Ordering::SeqCst), 1, "old callback should not fire");
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn progress_forwarder_clear() {
        let pf = ProgressForwarder::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        pf.set(oxi_ai::progress_callback(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        pf.invoke("x".into());
        assert_eq!(count.load(Ordering::SeqCst), 1);

        pf.clear();
        assert!(!pf.is_set());
        pf.invoke("y".into());
        assert_eq!(count.load(Ordering::SeqCst), 1, "invoke after clear is no-op");
    }

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

    #[test]
    fn browser_error_no_active_session() {
        let e = BrowserError::NoActiveSession;
        assert!(e.to_string().contains("no active session"));
    }
}
