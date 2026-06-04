//! oxibrowser-core backend for the browser engine.
//!
//! Implements `BrowserEngine` and `BrowserTab` using the pure-Rust
//! `oxibrowser-core` headless browser. Only compiled with
//! `#[cfg(feature = "native-browser")]`.

use super::config::BrowseConfig;
use super::engine::{
    BrowserError, BrowserTab as BrowserTabTrait, PageContent, ProgressForwarder,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

// ── OxiBrowserEngine ──────────────────────────────────────────────────────────

/// Browser engine powered by `oxibrowser-core`.
///
/// Spins a background task in its constructor that drains the browser's
/// event stream and invokes whatever callback is currently installed in
/// `progress_forwarder()`. The task exits gracefully when the browser
/// is dropped (the broadcast sender is dropped → `RecvError::Closed`).
///
/// Single-tenant — see `BrowseTool::execution_mode`.
pub struct OxiBrowserEngine {
    browser: oxibrowser_core::Browser,
    config: BrowseConfig,
    /// Shared slot for the tool's `on_progress` callback.
    progress: Arc<ProgressForwarder>,
    /// Background task that drains browser events into the forwarder.
    /// Held so we can `await` it on `close()` for clean shutdown.
    event_task: Mutex<Option<JoinHandle<()>>>,
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

        // Spawn the event-drain task. It lives for the lifetime of the engine:
        // when the browser (and thus its event_tx) is dropped, the task's
        // receiver returns `RecvError::Closed` and the task exits cleanly.
        let progress = Arc::new(ProgressForwarder::new());
        let mut events_rx = browser.subscribe_events();
        let progress_clone = Arc::clone(&progress);
        let event_task = tokio::spawn(async move {
            loop {
                match events_rx.recv().await {
                    Ok(event) => {
                        progress_clone.invoke(event.short_label());
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // Observer fell behind (broadcast buffer overflowed).
                        // We don't surface this to the agent loop — it's an
                        // observability concern only — but log it.
                        tracing::debug!(
                            skipped = skipped,
                            "oxibrowser event subscriber lagged; some events were dropped"
                        );
                        // Continue the loop — we may still be on time for the
                        // next batch of events.
                    }
                    Err(RecvError::Closed) => {
                        // Browser was dropped; the channel is gone.
                        // Exit gracefully.
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

    fn progress_forwarder(&self) -> Arc<ProgressForwarder> {
        Arc::clone(&self.progress)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browse::engine::BrowserEngine;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    /// End-to-end: the engine's background task should drain browser events
    /// and invoke the callback installed in `progress_forwarder()`.
    ///
    /// We use a `data:` URL so the test does not require network access.
    #[tokio::test]
    async fn engine_forwards_browser_events_to_progress_callback() {
        let engine = OxiBrowserEngine::new().await.unwrap();
        let forwarder = engine.progress_forwarder();
        let received: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        forwarder.set(oxi_ai::progress_callback(move |msg: String| {
            received_clone.lock().unwrap().push(msg);
        }));

        // Open a tab and navigate to a data: URL.
        let tab = engine.new_tab().await.unwrap();
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
        assert!(
            got.iter()
                .any(|s| s.contains("Hi") && s.contains("scripts")),
            "expected DocumentReady label to include title 'Hi' and script count, got {got:?}"
        );

        let _ = tab.close().await;
        let _ = engine.close().await;
    }

    /// Replacing the callback should drop the old one. Two callbacks should
    /// not both fire for the same event.
    #[tokio::test]
    async fn engine_replaces_progress_callback_cleanly() {
        let engine = OxiBrowserEngine::new().await.unwrap();
        let forwarder = engine.progress_forwarder();
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let ca = Arc::clone(&count_a);
        forwarder.set(oxi_ai::progress_callback(move |_| {
            ca.fetch_add(1, Ordering::SeqCst);
        }));

        let tab = engine.new_tab().await.unwrap();
        let _ = tab
            .goto("data:text/html,<title>A</title>")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let a_after_first = count_a.load(Ordering::SeqCst);
        assert!(a_after_first > 0, "callback A should have fired");

        // Replace with B.
        let cb_clone = Arc::clone(&count_b);
        forwarder.set(oxi_ai::progress_callback(move |_| {
            cb_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let _ = tab
            .goto("data:text/html,<title>B</title>")
            .await
            .unwrap();
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
}
