//! oxibrowser-core backend for the browser engine.
//!
//! Implements `BrowserEngine` and `BrowserTab` using the pure-Rust
//! `oxibrowser-core` headless browser. Only compiled with
//! `#[cfg(feature = "native-browser")]`.

use super::config::BrowseConfig;
use super::engine::{
    BrowseProgress, BrowserEngine, BrowserError, BrowserTab as BrowserTabTrait, PageContent,
    TabCallbackRegistry,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

/// Extract the `tab_id` from any `BrowserEvent` variant.
fn extract_event_tab_id(event: &oxibrowser_core::BrowserEvent) -> uuid::Uuid {
    match event {
        oxibrowser_core::BrowserEvent::NavigationStarted { tab_id, .. }
        | oxibrowser_core::BrowserEvent::WaitingForSelector { tab_id, .. }
        | oxibrowser_core::BrowserEvent::DocumentReady { tab_id, .. }
        | oxibrowser_core::BrowserEvent::ScreenshotCaptured { tab_id, .. }
        | oxibrowser_core::BrowserEvent::PdfExported { tab_id, .. } => *tab_id,
        // `BrowserEvent` is `#[non_exhaustive]`; fall through for forward-compat.
        _ => uuid::Uuid::nil(),
    }
}

/// Convert an `oxibrowser_core::BrowserEvent` into a `BrowseProgress`.
///
/// Returns `None` for unknown variants (forward-compatible with
/// future `BrowserEvent` additions).
fn browse_progress_from_event(event: &oxibrowser_core::BrowserEvent) -> Option<BrowseProgress> {
    use oxibrowser_core::BrowserEvent::*;
    match event {
        NavigationStarted { url, .. } => {
            Some(BrowseProgress::NavigationStarted { url: url.clone() })
        }
        WaitingForSelector {
            selector,
            timeout_ms,
            ..
        } => Some(BrowseProgress::WaitingForSelector {
            selector: selector.clone(),
            timeout_ms: *timeout_ms,
        }),
        DocumentReady {
            final_url,
            title,
            status,
            total_bytes,
            total_duration,
            ..
        } => Some(BrowseProgress::DocumentReady {
            url: final_url.clone(),
            title: title.clone(),
            status: *status,
            bytes: *total_bytes,
            duration_ms: total_duration.as_millis() as u64,
        }),
        ScreenshotCaptured {
            bytes,
            viewport_width,
            duration,
            ..
        } => Some(BrowseProgress::ScreenshotCaptured {
            bytes: *bytes,
            width: *viewport_width,
            duration_ms: duration.as_millis() as u64,
        }),
        PdfExported {
            bytes,
            viewport_width,
            duration,
            ..
        } => Some(BrowseProgress::PdfExported {
            bytes: *bytes,
            width: *viewport_width,
            duration_ms: duration.as_millis() as u64,
        }),
        _ => None,
    }
}

// ── OxicodeBrowserEngine ──────────────────────────────────────────────────────────

/// Browser engine powered by `oxibrowser-core`.
///
/// Spins a background task in its constructor that drains the browser's
/// event stream and invokes whatever callback is currently installed in
/// `progress_forwarder()`. The task exits gracefully when the browser
/// is dropped (the broadcast sender is dropped → `RecvError::Closed`).
///
/// Single-tenant — see `BrowseTool::execution_mode`.
pub struct OxicodeBrowserEngine {
    browser: oxibrowser_core::Browser,
    config: BrowseConfig,
    /// Shared per-tab callback registry.
    progress: Arc<TabCallbackRegistry>,
    /// Background task that drains browser events into the forwarder.
    /// Held so we can `await` it on `close()` for clean shutdown.
    event_task: Mutex<Option<JoinHandle<()>>>,
}

impl OxicodeBrowserEngine {
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

        // Spawn the event-drain task. It lives for the lifetime of the engine:
        // when the browser (and thus its event_tx) is dropped, the task's
        // receiver returns `RecvError::Closed` and the task exits cleanly.
        let progress = Arc::new(TabCallbackRegistry::new());
        let mut events_rx = browser.subscribe_events();
        let progress_clone = Arc::clone(&progress);
        let event_task = tokio::spawn(async move {
            loop {
                match events_rx.recv().await {
                    Ok(event) => {
                        let tab_id = extract_event_tab_id(&event);
                        // Enrich context FIRST so the String callback
                        // below reads the enriched context_cell.
                        if let Some(bp) = browse_progress_from_event(&event) {
                            progress_clone.invoke_browse(&tab_id, bp);
                        }
                        progress_clone.invoke(&tab_id, event.short_label());
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        tracing::debug!(
                            skipped = skipped,
                            "oxibrowser event subscriber lagged; some events were dropped"
                        );
                    }
                    Err(RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            browser,
            config,
            progress,
            event_task: Mutex::new(Some(event_task)),
        })
    }
}

impl Default for OxicodeBrowserEngine {
    fn default() -> Self {
        // Default cannot be async, so use blocking runtime.
        // Prefer `OxicodeBrowserEngine::new().await` in async contexts.
        // SAFETY: `Runtime::new()` cannot fail with default config; and
        // `block_on(Self::new())` panics rather than returning a half-built
        // engine because `Default` has no Result channel. A failing browser
        // init is an environment error (no Chrome/backend) that the caller
        // should handle via `OxicodeBrowserEngine::new().await` instead.
        #[allow(clippy::expect_used)]
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        #[allow(clippy::expect_used)]
        rt.block_on(Self::new())
            .expect("Failed to create default OxicodeBrowserEngine")
    }
}

#[async_trait]
impl BrowserEngine for OxicodeBrowserEngine {
    async fn new_tab(&self) -> Result<Box<dyn BrowserTabTrait>, BrowserError> {
        let tab = self
            .browser
            .new_tab()
            .await
            .map_err(|e| BrowserError::Backend(format!("Failed to create tab: {}", e)))?;
        let tab_id = tab.tab_id();
        Ok(Box::new(OxicodeTab {
            inner: tab,
            config: self.config.clone(),
            tab_id,
            registry: Arc::clone(&self.progress),
        }))
    }

    async fn close(&self) -> Result<(), BrowserError> {
        // Close the browser first. After this returns, the browser's internal
        // event_tx is dropped — but the broadcast channel itself stays alive
        // because the spawned event task holds its own sender clone. We need
        // to cancel the task explicitly to make `close()` mean "fully shut
        // down". The task will then exit with no further events forwarded.
        self.browser
            .close()
            .await
            .map_err(|e| BrowserError::Backend(format!("Browser close failed: {}", e)))?;

        if let Some(handle) = self.event_task.lock().await.take() {
            handle.abort();
            let _ = handle.await; // ignore JoinError from abort
        }
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        self.browser.is_open()
    }

    fn callback_registry(&self) -> Arc<TabCallbackRegistry> {
        Arc::clone(&self.progress)
    }
}

// ── OxicodeTab ────────────────────────────────────────────────────────────────────

/// A single browser tab backed by `oxibrowser-core`.
#[allow(dead_code)] // config kept for future per-tab settings
pub struct OxicodeTab {
    inner: oxibrowser_core::Tab,
    config: BrowseConfig,
    /// Stable tab identity from `oxibrowser_core::Tab::tab_id()`.
    tab_id: uuid::Uuid,
    /// Shared per-tab callback registry.
    registry: Arc<TabCallbackRegistry>,
}

impl OxicodeTab {
    /// Register a progress callback for this tab.
    pub fn set_progress_callback(&self, cb: crate::tools::ProgressCallback) {
        self.registry.set(self.tab_id, cb);
    }

    /// Remove the progress callback for this tab.
    pub fn clear_progress_callback(&self) {
        self.registry.clear(&self.tab_id);
    }

    /// Register a structured browse progress callback for this tab.
    pub fn set_browse_progress_callback_impl(&self, cb: super::engine::BrowseProgressCallback) {
        self.registry.set_browse(self.tab_id, cb);
    }

    /// Return this tab's stable ID.
    pub fn tab_id(&self) -> uuid::Uuid {
        self.tab_id
    }
}

#[async_trait]
impl BrowserTabTrait for OxicodeTab {
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
            .r#type(selector, text)
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
    /// Native structured-wait override — maps our portable
    /// [`super::engine::BrowseWaitCondition`] to `oxibrowser_core::tab::WaitCondition` so
    /// `NetworkIdle` / `DomContentLoaded` / `Load` honour real in-flight
    /// traffic semantics (Playwright/Puppeteer "networkidle" parity).
    async fn wait_for_condition(
        &self,
        cond: &super::engine::BrowseWaitCondition,
        timeout_ms: u64,
    ) -> Result<(), BrowserError> {
        use super::engine::BrowseWaitCondition as Bwc;
        let mapped = match cond {
            Bwc::Visible(s) => oxibrowser_core::tab::WaitCondition::Visible(s.clone()),
            Bwc::NetworkIdle => oxibrowser_core::tab::WaitCondition::NetworkIdle,
            Bwc::DomContentLoaded => oxibrowser_core::tab::WaitCondition::DomContentLoaded,
            Bwc::Load => oxibrowser_core::tab::WaitCondition::Load,
        };
        self.inner
            .wait_for_condition(mapped, timeout_ms)
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
    /// omp `observe()` parity — runs the JS accessibility-surface synthesis
    /// via `evaluate()` and parses the result into a [`super::engine::Observation`].
    /// Returns the page's visible, interactive elements with stable
    /// `data-oxicode-ref` selectors (no coordinates — boa only approximates
    /// layout geometry).
    async fn observe(&self) -> Result<super::engine::Observation, BrowserError> {
        let page = self
            .inner
            .content()
            .await
            .map_err(|e| BrowserError::Backend(e.to_string()))?;
        let value = self
            .inner
            .evaluate(super::helpers::JS_OBSERVE)
            .await
            .map_err(|e| BrowserError::Evaluation(e.to_string()))?;
        Ok(super::engine::Observation {
            url: page.url,
            title: page.title,
            elements: super::helpers::parse_observed_elements(value),
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

    async fn print_to_pdf(&self, width: u32) -> Result<Vec<u8>, BrowserError> {
        self.inner
            .print_to_pdf(width)
            .await
            .map_err(|e| BrowserError::Pdf(e.to_string()))
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

    fn tab_id(&self) -> uuid::Uuid {
        self.tab_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clear_progress_callback(&self) {
        self.registry.clear(&self.tab_id);
    }

    fn set_browse_progress_callback(&self, cb: super::engine::BrowseProgressCallback) {
        self.set_browse_progress_callback_impl(cb);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// End-to-end: the engine's background task should drain browser events
    /// and invoke the callback installed in `progress_forwarder()`.
    ///
    /// We use a `data:` URL so the test does not require network access.
    #[tokio::test]
    async fn engine_forwards_browser_events_to_progress_callback() {
        let engine = OxicodeBrowserEngine::new().await.unwrap();
        let registry = engine.callback_registry();
        let received: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        // Open a tab first to get its tab_id
        let tab = engine.new_tab().await.unwrap();
        let tab_id = tab
            .as_any()
            .downcast_ref::<OxicodeTab>()
            .map(|t| t.tab_id())
            .unwrap_or_default();

        registry.set(
            tab_id,
            oxicode_ai::progress_callback(move |msg: String| {
                received_clone.lock().unwrap().push(msg);
            }),
        );

        // Navigate to a data: URL.
        let _ = tab
            .goto("data:text/html,<title>Hi</title><p>Hello</p>")
            .await
            .unwrap();

        // Give the background task a moment to drain the broadcast channel.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let got = received.lock().unwrap().clone();
        assert!(
            got.iter().any(|s| s.starts_with("Opening")),
            "expected 'Opening …' event, got {got:?}"
        );
        assert!(
            got.iter().any(|s| s.contains("Loaded")),
            "expected 'Loaded …' event, got {got:?}"
        );

        let _ = tab.close().await;
        let _ = engine.close().await;
    }

    /// Replacing the callback should drop the old one. Two callbacks should
    /// not both fire for the same event.
    #[tokio::test]
    async fn engine_replaces_progress_callback_cleanly() {
        let engine = OxicodeBrowserEngine::new().await.unwrap();
        let registry = engine.callback_registry();
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        // Open tab to get its tab_id
        let tab = engine.new_tab().await.unwrap();
        let tab_id = tab
            .as_any()
            .downcast_ref::<OxicodeTab>()
            .map(|t| t.tab_id())
            .unwrap_or_default();

        let ca = Arc::clone(&count_a);
        registry.set(
            tab_id,
            oxicode_ai::progress_callback(move |_| {
                ca.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let _ = tab.goto("data:text/html,<title>A</title>").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let a_after_first = count_a.load(Ordering::SeqCst);
        assert!(a_after_first > 0, "callback A should have fired");

        // Replace with B.
        let cb_clone = Arc::clone(&count_b);
        registry.set(
            tab_id,
            oxicode_ai::progress_callback(move |_| {
                cb_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let _ = tab.goto("data:text/html,<title>B</title>").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let a_final = count_a.load(Ordering::SeqCst);
        let b_final = count_b.load(Ordering::SeqCst);
        assert_eq!(
            a_final, a_after_first,
            "callback A should not fire after being replaced"
        );
        assert!(b_final > 0, "callback B should have fired");

        let _ = tab.close().await;
        let _ = engine.close().await;
    }

    /// End-to-end: `invoke_browse` should fire the structured
    /// `BrowseProgressCallback` with `DocumentReady` carrying the page title
    /// and HTTP status. This is the key T2 integration test for
    /// `BrowseProgress` propagation.
    #[tokio::test]
    async fn engine_forwards_browse_progress_to_callback() {
        use crate::tools::browse::BrowseProgress;

        let engine = OxicodeBrowserEngine::new().await.unwrap();
        let registry = engine.callback_registry();
        let received: Arc<StdMutex<Vec<BrowseProgress>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        let tab = engine.new_tab().await.unwrap();
        let tab_id = tab.tab_id();

        registry.set_browse(
            tab_id,
            Arc::new(move |bp: BrowseProgress| {
                received_clone.lock().unwrap().push(bp);
            }),
        );

        // Navigate to a data: URL — must produce DocumentReady.
        let _ = tab
            .goto("data:text/html,<title>Hi</title><p>Hello</p>")
            .await
            .unwrap();

        // Allow drain task to process events.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let events = received.lock().unwrap().clone();
        assert!(
            events
                .iter()
                .any(|bp| matches!(bp, BrowseProgress::DocumentReady { status: 200, .. })),
            "expected DocumentReady with status 200, got {events:?}"
        );
        let doc_ready = events.iter().find_map(|bp| match bp {
            BrowseProgress::DocumentReady {
                title,
                bytes,
                duration_ms,
                ..
            } => Some((title.clone(), *bytes, *duration_ms)),
            _ => None,
        });
        let (title, bytes, duration_ms) = doc_ready.expect("DocumentReady present");
        assert_eq!(title, "Hi");
        assert!(
            bytes > 0,
            "bytes should be > 0 for non-empty page, got {bytes}"
        );
        assert!(
            duration_ms < 30_000,
            "duration_ms should be reasonable, got {duration_ms}"
        );

        let _ = tab.close().await;
        let _ = engine.close().await;
    }

    /// Open two tabs, register per-tab browse callbacks, and verify each
    /// callback receives only its own tab's `BrowseProgress` events.
    #[tokio::test]
    async fn engine_routes_browse_progress_by_tab_id() {
        use crate::tools::browse::BrowseProgress;

        let engine = OxicodeBrowserEngine::new().await.unwrap();
        let registry = engine.callback_registry();

        let received_a: Arc<StdMutex<Vec<BrowseProgress>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_b: Arc<StdMutex<Vec<BrowseProgress>>> = Arc::new(StdMutex::new(Vec::new()));
        let ra = Arc::clone(&received_a);
        let rb = Arc::clone(&received_b);

        let tab_a = engine.new_tab().await.unwrap();
        let tab_b = engine.new_tab().await.unwrap();
        let tid_a = tab_a.tab_id();
        let tid_b = tab_b.tab_id();

        registry.set_browse(
            tid_a,
            Arc::new(move |bp: BrowseProgress| {
                ra.lock().unwrap().push(bp);
            }),
        );
        registry.set_browse(
            tid_b,
            Arc::new(move |bp: BrowseProgress| {
                rb.lock().unwrap().push(bp);
            }),
        );

        let _ = tab_a
            .goto("data:text/html,<title>OnlyA</title>")
            .await
            .unwrap();
        let _ = tab_b
            .goto("data:text/html,<title>OnlyB</title>")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let got_a = received_a.lock().unwrap().clone();
        let got_b = received_b.lock().unwrap().clone();

        let a_titles: Vec<&str> = got_a
            .iter()
            .filter_map(|bp| match bp {
                BrowseProgress::DocumentReady { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        let b_titles: Vec<&str> = got_b
            .iter()
            .filter_map(|bp| match bp {
                BrowseProgress::DocumentReady { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            a_titles.contains(&"OnlyA"),
            "A should have OnlyA, got {a_titles:?}"
        );
        assert!(!a_titles.contains(&"OnlyB"), "A should NOT have OnlyB");
        assert!(
            b_titles.contains(&"OnlyB"),
            "B should have OnlyB, got {b_titles:?}"
        );
        assert!(!b_titles.contains(&"OnlyA"), "B should NOT have OnlyA");

        let _ = tab_a.close().await;
        let _ = tab_b.close().await;
        let _ = engine.close().await;
    }

    /// Open two tabs in one engine, register two callbacks, navigate each.
    /// Assert each callback fires only for its own tab's events.
    #[tokio::test]
    async fn engine_routes_events_by_tab_id_concurrent() {
        let engine = OxicodeBrowserEngine::new().await.unwrap();
        let registry = engine.callback_registry();

        let received_a: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_b: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_a_clone = Arc::clone(&received_a);
        let received_b_clone = Arc::clone(&received_b);

        // Open two tabs
        let tab_a = engine.new_tab().await.unwrap();
        let tab_b = engine.new_tab().await.unwrap();
        let tab_id_a = tab_a.tab_id();
        let tab_id_b = tab_b.tab_id();
        assert_ne!(tab_id_a, tab_id_b, "two tabs must have distinct IDs");

        // Register per-tab callbacks
        registry.set(
            tab_id_a,
            oxicode_ai::progress_callback(move |msg: String| {
                received_a_clone.lock().unwrap().push(msg);
            }),
        );
        registry.set(
            tab_id_b,
            oxicode_ai::progress_callback(move |msg: String| {
                received_b_clone.lock().unwrap().push(msg);
            }),
        );

        // Navigate tab A
        let _ = tab_a
            .goto("data:text/html,<title>TabA</title>")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Navigate tab B
        let _ = tab_b
            .goto("data:text/html,<title>TabB</title>")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let got_a = received_a.lock().unwrap().clone();
        let got_b = received_b.lock().unwrap().clone();

        // Each tab should have received its own events
        assert!(
            got_a.iter().any(|s| s.contains("TabA")),
            "tab A callback should have received TabA events, got {got_a:?}"
        );
        assert!(
            got_b.iter().any(|s| s.contains("TabB")),
            "tab B callback should have received TabB events, got {got_b:?}"
        );
        // Cross-contamination check: A's callback should NOT have B's events
        assert!(
            !got_a.iter().any(|s| s.contains("TabB")),
            "tab A callback should NOT have received TabB events, got {got_a:?}"
        );
        assert!(
            !got_b.iter().any(|s| s.contains("TabA")),
            "tab B callback should NOT have received TabA events, got {got_b:?}"
        );

        let _ = tab_a.close().await;
        let _ = tab_b.close().await;
        let _ = engine.close().await;
    }
}
