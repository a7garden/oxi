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

    /// Return this tab's unique ID, if the backend supports it.
    /// Defaults to `Uuid::nil()` for backends that don't track tab identity.
    fn tab_id(&self) -> uuid::Uuid {
        uuid::Uuid::nil()
    }

    /// Support downcasting for backend-specific access.
    fn as_any(&self) -> &dyn std::any::Any {
        // Default: no concrete type info.
        &std::marker::PhantomData::<()>
    }

    /// Clear any registered progress callback for this tab.
    /// Defaults to no-op — only backends with callback registries override.
    fn clear_progress_callback(&self) {}

    /// Register a structured browse progress callback for this tab.
    /// Defaults to no-op — only backends with browse callback support override.
    fn set_browse_progress_callback(&self, _cb: BrowseProgressCallback) {}
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

    /// Access the engine's per-tab callback registry.
    ///
    /// Tools (e.g. `BrowseTool`) register per-tab callbacks keyed by
    /// `tab_id`. The backend's background event-drain task extracts
    /// `tab_id` from each `BrowserEvent` and routes it to the correct
    /// callback. Backends without event streaming return an empty
    /// registry — `set`/`invoke` become no-ops.
    ///
    /// Default implementation returns a fresh empty registry.
    fn callback_registry(&self) -> Arc<TabCallbackRegistry> {
        Arc::new(TabCallbackRegistry::new())
    }
}

// ── BrowseProgress ──────────────────────────────────────────────────────

/// Structured progress event for browser tool execution.
///
/// Converted from `oxibrowser_core::BrowserEvent` in the backend's drain
/// task. Carries structured data that would be lost if flattened to a string
/// via `short_label()`. The agent loop's browse callback receives these and
/// enriches `ToolCallContext` with the result fields.
///
/// Defined here (not in `oxibrowser_backend.rs`) so the type is always
/// available — no feature gate needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrowseProgress {
    /// A navigation has begun.
    NavigationStarted {
        /// URL being navigated to (pre-redirect).
        url: String,
    },

    /// Waiting for a CSS selector to appear.
    WaitingForSelector {
        /// CSS selector being awaited.
        selector: String,
        /// Maximum wait time in milliseconds.
        timeout_ms: u64,
    },

    /// Page has finished loading and JS has executed.
    /// This is the key event — carries rich structured data.
    DocumentReady {
        /// Final URL after redirects.
        url: String,
        /// Page `<title>`.
        title: String,
        /// HTTP status code.
        status: u16,
        /// Size of the HTML body in bytes.
        bytes: u64,
        /// Wall-clock duration of the page load, in milliseconds.
        duration_ms: u64,
    },

    /// A screenshot has been captured.
    ScreenshotCaptured {
        /// Size of the PNG payload in bytes.
        bytes: usize,
        /// Viewport width the screenshot was rendered at.
        width: u32,
        /// Render duration in milliseconds.
        duration_ms: u64,
    },

    /// Navigation failed.
    NavigationFailed {
        /// URL that failed.
        url: String,
        /// Error description.
        error: String,
    },
}

// ── BrowseProgressCallback ──────────────────────────────────────────────

/// Callback type for structured browse progress events.
pub type BrowseProgressCallback = Arc<dyn Fn(BrowseProgress) + Send + Sync>;

// ── TabCallbackRegistry ──────────────────────────────────────────────────

/// Per-`tab_id` callback entry. Groups the string progress callback
/// and the structured browse callback for a single tab. Both share
/// the same lifecycle — `clear` removes both at once.
#[derive(Default)]
struct TabCallbacks {
    /// String progress callback (`partial_result` text).
    progress: Option<crate::tools::ProgressCallback>,
    /// Structured browse progress callback (context enrichment).
    browse: Option<BrowseProgressCallback>,
}

/// Per-`tab_id` callback registry for browser event routing.
///
/// Each `BrowseTool` invocation opens its own tab and registers a callback
/// keyed by the tab's `tab_id`. The engine's background event-drain task
/// extracts `tab_id` from each `BrowserEvent` and routes it to the correct
/// callback. Multiple tabs can be active concurrently — each receives only
/// its own events.
///
/// Tabs that have no registered callback (e.g. opened outside of a tool
/// call) are silently ignored — `invoke` is a no-op for unknown tab IDs.
pub struct TabCallbackRegistry {
    entries: Mutex<HashMap<uuid::Uuid, TabCallbacks>>,
}

impl Default for TabCallbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TabCallbackRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Register a string progress callback for the given `tab_id`.
    pub fn set(&self, tab_id: uuid::Uuid, cb: crate::tools::ProgressCallback) {
        self.entries.lock().entry(tab_id).or_default().progress = Some(cb);
    }

    /// Register a structured browse progress callback for the given tab.
    pub fn set_browse(&self, tab_id: uuid::Uuid, cb: BrowseProgressCallback) {
        self.entries.lock().entry(tab_id).or_default().browse = Some(cb);
    }

    /// Remove **all** callbacks for `tab_id`. Called when the tab is closed.
    pub fn clear(&self, tab_id: &uuid::Uuid) {
        self.entries.lock().remove(tab_id);
    }

    /// Invoke the string progress callback for `tab_id`, if registered.
    pub fn invoke(&self, tab_id: &uuid::Uuid, msg: String) {
        if let Some(entry) = self.entries.lock().get(tab_id)
            && let Some(ref cb) = entry.progress
        {
            cb(msg);
        }
    }

    /// Invoke the browse progress callback for `tab_id`, if registered.
    pub fn invoke_browse(&self, tab_id: &uuid::Uuid, progress: BrowseProgress) {
        if let Some(entry) = self.entries.lock().get(tab_id)
            && let Some(ref cb) = entry.browse
        {
            cb(progress);
        }
    }

    /// Whether a string callback is registered for the given `tab_id`.
    pub fn is_set(&self, tab_id: &uuid::Uuid) -> bool {
        self.entries.lock().contains_key(tab_id)
    }

    /// Number of currently registered tabs.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Returns `true` if no tabs have registered callbacks.
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn tab_callback_registry_default_is_empty() {
        let reg = TabCallbackRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        // invoke on empty registry is a silent no-op
        let nil = uuid::Uuid::nil();
        reg.invoke(&nil, "should be dropped".into());
    }

    #[test]
    fn tab_callback_registry_set_and_invoke() {
        let reg = TabCallbackRegistry::new();
        let tab_a = uuid::Uuid::new_v4();
        let tab_b = uuid::Uuid::new_v4();
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        reg.set(
            tab_a,
            oxi_ai::progress_callback(move |msg: String| {
                assert_eq!(msg, "hello");
                count_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );
        assert!(reg.is_set(&tab_a));
        assert!(!reg.is_set(&tab_b));

        reg.invoke(&tab_a, "hello".into());
        reg.invoke(&tab_a, "hello".into());
        // invoke for unregistered tab_b is a no-op
        reg.invoke(&tab_b, "hello".into());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn tab_callback_registry_set_per_tab_isolation() {
        let reg = TabCallbackRegistry::new();
        let tab_a = uuid::Uuid::new_v4();
        let tab_b = uuid::Uuid::new_v4();
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let ca = Arc::clone(&count_a);
        reg.set(
            tab_a,
            oxi_ai::progress_callback(move |_| {
                ca.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let cb_clone = Arc::clone(&count_b);
        reg.set(
            tab_b,
            oxi_ai::progress_callback(move |_| {
                cb_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );

        reg.invoke(&tab_a, "event".into());
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 0);

        reg.invoke(&tab_b, "event".into());
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn tab_callback_registry_clear() {
        let reg = TabCallbackRegistry::new();
        let tab_a = uuid::Uuid::new_v4();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        reg.set(
            tab_a,
            oxi_ai::progress_callback(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );
        reg.invoke(&tab_a, "x".into());
        assert_eq!(count.load(Ordering::SeqCst), 1);

        reg.clear(&tab_a);
        assert!(!reg.is_set(&tab_a));
        reg.invoke(&tab_a, "y".into());
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "invoke after clear is no-op"
        );
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

    // ── Browse progress callback tests ──────────────────────────

    #[test]
    fn tab_callback_registry_browse_set_and_invoke() {
        let reg = TabCallbackRegistry::new();
        let tab = uuid::Uuid::new_v4();
        let received: Arc<std::sync::Mutex<Vec<BrowseProgress>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let r = Arc::clone(&received);
        reg.set_browse(
            tab,
            Arc::new(move |bp: BrowseProgress| {
                r.lock().unwrap().push(bp);
            }),
        );

        let progress = BrowseProgress::DocumentReady {
            url: "https://example.com".into(),
            title: "Example".into(),
            status: 200,
            bytes: 1024,
            duration_ms: 500,
        };
        reg.invoke_browse(&tab, progress.clone());

        let events = received.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            BrowseProgress::DocumentReady { status: 200, .. }
        ));
    }

    #[test]
    fn tab_callback_registry_browse_clear_removes_both() {
        let reg = TabCallbackRegistry::new();
        let tab = uuid::Uuid::new_v4();

        // Register both types
        reg.set(tab, oxi_ai::progress_callback(move |_| {}));
        reg.set_browse(tab, Arc::new(move |_: BrowseProgress| {}));
        assert!(reg.is_set(&tab));

        // clear removes both
        reg.clear(&tab);
        assert!(!reg.is_set(&tab));
        assert!(reg.is_empty());
    }

    #[test]
    fn tab_callback_registry_browse_isolation_per_tab() {
        let reg = TabCallbackRegistry::new();
        let tab_a = uuid::Uuid::new_v4();
        let tab_b = uuid::Uuid::new_v4();

        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let ca = Arc::clone(&count_a);
        reg.set_browse(
            tab_a,
            Arc::new(move |_: BrowseProgress| {
                ca.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let cb2 = Arc::clone(&count_b);
        reg.set_browse(
            tab_b,
            Arc::new(move |_: BrowseProgress| {
                cb2.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let doc_ready = BrowseProgress::DocumentReady {
            url: "https://example.com".into(),
            title: "Example".into(),
            status: 200,
            bytes: 1024,
            duration_ms: 100,
        };
        reg.invoke_browse(&tab_a, doc_ready.clone());
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 0);

        reg.invoke_browse(&tab_b, doc_ready);
        assert_eq!(count_a.load(Ordering::SeqCst), 1);
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn browse_progress_serde_roundtrip() {
        let variants = vec![
            BrowseProgress::NavigationStarted {
                url: "https://example.com".into(),
            },
            BrowseProgress::WaitingForSelector {
                selector: ".content".into(),
                timeout_ms: 5000,
            },
            BrowseProgress::DocumentReady {
                url: "https://example.com/page".into(),
                title: "Test Page".into(),
                status: 200,
                bytes: 4096,
                duration_ms: 1234,
            },
            BrowseProgress::ScreenshotCaptured {
                bytes: 8192,
                width: 1280,
                duration_ms: 200,
            },
            BrowseProgress::NavigationFailed {
                url: "https://fail.example.com".into(),
                error: "connection refused".into(),
            },
        ];

        for bp in &variants {
            let json = serde_json::to_string(bp).unwrap();
            let restored: BrowseProgress = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&restored).unwrap();
            assert_eq!(json, json2, "roundtrip failed for {:?}", bp);
        }
    }
}
