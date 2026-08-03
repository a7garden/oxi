//! RAII guard that ensures a browser tab is properly closed.
//!
//! Prevents tab leaks by tracking lifecycle and warning on implicit drops.
//! Use `TabGuard::close().await` for explicit async close, or `into_inner()`
//! to transfer ownership.

use super::engine::BrowserTab;

/// RAII wrapper around `Box<dyn BrowserTab>`.
///
/// # Leak Prevention
///
/// If dropped without calling [`close`](TabGuard::close) or
/// [`into_inner`](TabGuard::into_inner), a `tracing::warn` is emitted.
/// Since Rust's `Drop` cannot be async, the tab itself cannot be closed
/// synchronously — always prefer `guard.close().await` before the guard
/// goes out of scope.
///
/// # Example
///
/// ```ignore
/// let guard = TabGuard::new(engine.new_tab().await?);
/// let page = guard.tab().goto(url).await?;
/// // ... use tab ...
/// guard.close().await; // explicit async close
/// ```
pub struct TabGuard {
    tab: Option<Box<dyn BrowserTab>>,
    explicitly_consumed: bool,
}

impl TabGuard {
    /// Create a new guard wrapping an opened tab.
    pub fn new(tab: Box<dyn BrowserTab>) -> Self {
        Self {
            tab: Some(tab),
            explicitly_consumed: false,
        }
    }

    /// Access the underlying tab reference.
    ///
    /// # Panics
    ///
    /// Panics if the guard has already been consumed (via `close` or `into_inner`).
    pub fn tab(&self) -> &dyn BrowserTab {
        // SAFETY: `as_ref().map(...)` returns None only after this guard was
        // already consumed (a use-after-consume bug); the guard contract
        // forbids it.
        #[allow(clippy::expect_used)]
        self.tab
            .as_ref()
            .map(|t| t.as_ref() as &dyn BrowserTab)
            .expect("TabGuard: tab already consumed")
    }

    /// Explicitly close the tab and consume the guard.
    ///
    /// If `close()` fails on the underlying tab, a warning is logged but
    /// no error is propagated — the guard is still consumed.
    pub async fn close(mut self) {
        self.explicitly_consumed = true;
        if let Some(tab) = self.tab.take() {
            tab.clear_progress_callback();
            if let Err(e) = tab.close().await {
                tracing::warn!("TabGuard: tab close failed: {}", e);
            }
        }
    }

    /// Take ownership of the tab without closing it.
    ///
    /// Useful when transferring tab ownership to a longer-lived scope
    /// (e.g., multi-step script execution).
    pub fn into_inner(mut self) -> Box<dyn BrowserTab> {
        self.explicitly_consumed = true;
        // SAFETY: `take()` returns None only after this guard was already
        // consumed (a use-after-consume bug); the guard contract forbids it.
        #[allow(clippy::expect_used)]
        self.tab.take().expect("TabGuard: tab already consumed")
    }
}

impl Drop for TabGuard {
    fn drop(&mut self) {
        if !self.explicitly_consumed {
            tracing::warn!(
                "TabGuard dropped without explicit close — tab may leak. \
                 Call .close().await or .into_inner() to prevent this."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browse::engine::{BrowserError, PageContent};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
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
    impl BrowserTab for MockTab {
        async fn goto(&self, _url: &str) -> Result<PageContent, BrowserError> {
            Ok(PageContent::empty())
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
            Ok(PageContent::empty())
        }
        async fn query_all(&self, _selector: &str) -> Result<Vec<String>, BrowserError> {
            Ok(vec![])
        }
        async fn evaluate(&self, _js: &str) -> Result<Value, BrowserError> {
            Ok(Value::Null)
        }
        async fn screenshot(&self, _width: u32) -> Result<Vec<u8>, BrowserError> {
            Ok(vec![])
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
        async fn clear(&self, _selector: &str) -> Result<(), BrowserError> {
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
        async fn scroll(&self, _delta_x: f64, _delta_y: f64) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn scroll_into_view(&self, _selector: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn drag(&self, _from: &str, _to: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn upload_file(&self, _selector: &str, _path: &str) -> Result<(), BrowserError> {
            Ok(())
        }
        async fn get_value(&self, _selector: &str) -> Result<String, BrowserError> {
            Ok(String::new())
        }
        async fn evaluate_await(&self, _js: &str) -> Result<Value, BrowserError> {
            Ok(Value::Null)
        }
        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
    }

    // ── Tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_guard_close_success() {
        let (mock, closed_flag) = MockTab::new();
        let guard = TabGuard::new(Box::new(mock));
        assert!(!closed_flag.load(Ordering::SeqCst));
        guard.close().await;
        assert!(closed_flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_guard_into_inner() {
        let (mock, closed_flag) = MockTab::new();
        let guard = TabGuard::new(Box::new(mock));
        let _tab = guard.into_inner();
        // Tab not closed — ownership transferred
        assert!(!closed_flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_guard_drop_without_close_warns() {
        let (mock, _) = MockTab::new();
        let guard = TabGuard::new(Box::new(mock));
        // Drop without close — should log warning but not panic
        drop(guard);
    }

    #[tokio::test]
    async fn test_guard_tab_access() {
        let (mock, _) = MockTab::new();
        let guard = TabGuard::new(Box::new(mock));
        // Should be able to access the tab
        let result = guard.tab().goto("https://example.com").await;
        assert!(result.is_ok());
        guard.close().await;
    }
}
