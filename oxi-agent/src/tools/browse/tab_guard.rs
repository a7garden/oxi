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
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // ── Mock tab for unit tests ─────────────────────────────────

    struct MockTab {
        closed: Arc<AtomicBool>,
    }

    impl MockTab {
        fn new<'a>() -> (Self, Arc<AtomicBool>) {
            let closed = Arc::new(AtomicBool::new(false));
            (
                Self {
                    closed: closed.clone(),
                },
                closed,
            )
        }
    }

    impl BrowserTab for MockTab {
        fn goto<'a>(
            &'a self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(PageContent::empty()) })
        }
        fn click<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn type_<'a>(
            &'a self,
            _selector: &str,
            _text: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn fill<'a>(
            &'a self,
            _selector: &str,
            _value: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn press<'a>(
            &'a self,
            _combo: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn wait_for<'a>(
            &'a self,
            _selector: &str,
            _timeout_ms: u64,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn content<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(PageContent::empty()) })
        }
        fn query_all<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(vec![]) })
        }
        fn evaluate<'a>(
            &'a self,
            _js: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Value, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(Value::Null) })
        }
        fn screenshot<'a>(
            &'a self,
            _width: u32,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(vec![]) })
        }
        fn close<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move {
                self.closed.store(true, Ordering::SeqCst);
                Ok(())
            })
        }
        fn back<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(PageContent::empty()) })
        }
        fn forward<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(PageContent::empty()) })
        }
        fn reload<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<PageContent, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(PageContent::empty()) })
        }
        fn select_option<'a>(
            &'a self,
            _selector: &str,
            _value: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn check<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn uncheck<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn clear<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn hover<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn double_click<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn right_click<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn scroll<'a>(
            &'a self,
            _delta_x: f64,
            _delta_y: f64,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn scroll_into_view<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn drag<'a>(
            &'a self,
            _from: &str,
            _to: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn upload_file<'a>(
            &'a self,
            _selector: &str,
            _path: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn get_value<'a>(
            &'a self,
            _selector: &str,
        ) -> Pin<Box<dyn Future<Output = Result<String, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(String::new()) })
        }
        fn evaluate_await<'a>(
            &'a self,
            _js: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Value, BrowserError>> + Send + 'a>> {
            Box::pin(async move { Ok(Value::Null) })
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
