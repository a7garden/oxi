//! Loader with cancellation support.
//!
//! Extends the base [`Loader`] component with an abort flag that can be
//! checked by background operations. When the user presses Escape or
//! Ctrl+C, the loader is marked as aborted and an optional callback fires.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::components::Loader;
use crate::{Color, Component, Event, KeyCode, Rect, Size, Surface};

/// Callback when the loader is aborted by the user.
pub type OnAbortFn = Box<dyn Fn() + Send>;

/// A loader that can be cancelled with Escape or Ctrl+C.
///
/// Wraps a [`Loader`] with an atomic abort flag and an optional callback.
/// Background tasks can check [`CancellableLoader::is_aborted`] to know
/// when to stop.
///
/// # Example
///
/// ```ignore
/// let mut loader = CancellableLoader::new("Working...");
/// let abort_flag = loader.abort_flag();
///
/// // In event loop:
/// loader.handle_event(&event);
///
/// // In background task:
/// if abort_flag.load(Ordering::Relaxed) { return; }
/// ```
pub struct CancellableLoader {
    /// Inner loader for rendering.
    inner: Loader,
    /// Shared abort flag — set to `true` when the user cancels.
    aborted: Arc<AtomicBool>,
    /// Optional callback fired on abort.
    on_abort: Option<OnAbortFn>,
}

impl CancellableLoader {
    /// Create a new cancellable loader with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            inner: Loader::new().with_message(message),
            aborted: Arc::new(AtomicBool::new(false)),
            on_abort: None,
        }
    }

    /// Create a cancellable loader with a color.
    pub fn with_color(message: impl Into<String>, color: Color) -> Self {
        Self {
            inner: Loader::new().with_message(message).with_color(color),
            aborted: Arc::new(AtomicBool::new(false)),
            on_abort: None,
        }
    }

    /// Set the abort callback.
    pub fn on_abort(mut self, f: impl Fn() + Send + 'static) -> Self {
        self.on_abort = Some(Box::new(f));
        self
    }

    /// Get a clone of the shared abort flag.
    ///
    /// Background tasks can poll this to detect cancellation.
    pub fn abort_flag(&self) -> Arc<AtomicBool> {
        self.aborted.clone()
    }

    /// Check whether the loader was aborted.
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed)
    }

    /// Abort the loader programmatically.
    pub fn abort(&mut self) {
        self.aborted.store(true, Ordering::Relaxed);
        self.inner.cancel();
        if let Some(ref cb) = self.on_abort {
            cb();
        }
    }

    /// Reset the loader to a non-aborted, running state.
    pub fn reset(&mut self) {
        self.aborted.store(false, Ordering::Relaxed);
        self.inner.reset();
    }

    /// Set the status message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.inner.set_message(message);
    }

    /// Advance the spinner frame. Call on tick.
    pub fn tick(&mut self) {
        if !self.is_aborted() {
            self.inner.tick();
        }
    }

    /// Mark as done with a message.
    pub fn set_done(&mut self, msg: impl Into<String>) {
        self.inner.set_done(msg);
    }
}

impl Default for CancellableLoader {
    fn default() -> Self {
        Self::new("")
    }
}

impl Component for CancellableLoader {
    fn name(&self) -> &str {
        "CancellableLoader"
    }

    fn request_render(&mut self) {
        self.inner.request_render();
    }

    fn is_dirty(&self) -> bool {
        self.inner.is_dirty()
    }

    fn clear_dirty(&mut self) {
        self.inner.clear_dirty();
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if self.is_aborted() {
            return false;
        }

        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Escape => {
                    self.abort();
                    return true;
                }
                KeyCode::Char('c') if key.modifiers.ctrl => {
                    self.abort();
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn render(&mut self, surface: &mut Surface, area: Rect) {
        self.inner.render(surface, area);
    }

    fn min_size(&self) -> Size {
        self.inner.min_size()
    }

    fn on_focus(&mut self) {
        self.inner.on_focus();
    }

    fn on_unfocus(&mut self) {
        self.inner.on_unfocus();
    }

    fn is_focused(&self) -> bool {
        self.inner.is_focused()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_loader() {
        let loader = CancellableLoader::new("Working...");
        assert!(!loader.is_aborted());
    }

    #[test]
    fn test_abort() {
        let mut loader = CancellableLoader::new("Working...");
        assert!(!loader.is_aborted());

        loader.abort();
        assert!(loader.is_aborted());
    }

    #[test]
    fn test_abort_flag() {
        let loader = CancellableLoader::new("Working...");
        let flag = loader.abort_flag();
        assert!(!flag.load(Ordering::Relaxed));

        // Simulate background task checking
        drop(loader);
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn test_reset() {
        let mut loader = CancellableLoader::new("Working...");
        loader.abort();
        assert!(loader.is_aborted());

        loader.reset();
        assert!(!loader.is_aborted());
    }

    #[test]
    fn test_handle_escape() {
        let mut loader = CancellableLoader::new("Working...");
        let event = Event::Key(crate::KeyEvent::new(KeyCode::Escape));
        assert!(loader.handle_event(&event));
        assert!(loader.is_aborted());
    }

    #[test]
    fn test_handle_ctrl_c() {
        let mut loader = CancellableLoader::new("Working...");
        let event = Event::Key(crate::KeyEvent::with_modifiers(
            KeyCode::Char('c'),
            crate::event::KeyModifiers::new().with_ctrl(),
        ));
        assert!(loader.handle_event(&event));
        assert!(loader.is_aborted());
    }

    #[test]
    fn test_ignore_events_after_abort() {
        let mut loader = CancellableLoader::new("Working...");
        loader.abort();

        let event = Event::Key(crate::KeyEvent::new(KeyCode::Escape));
        assert!(!loader.handle_event(&event));
    }

    #[test]
    fn test_abort_callback() {
        use std::sync::atomic::AtomicUsize;

        static CALLED: AtomicUsize = AtomicUsize::new(0);

        let mut loader = CancellableLoader::new("Working...")
            .on_abort(|| {
                CALLED.fetch_add(1, Ordering::SeqCst);
            });

        loader.abort();
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_set_message() {
        let mut loader = CancellableLoader::new("Working...");
        loader.set_message("New message");
        // Just verify it doesn't panic
    }

    #[test]
    fn test_tick() {
        let mut loader = CancellableLoader::new("Working...");
        loader.tick();
        // Should not panic, spinner advances
    }

    #[test]
    fn test_tick_aborted() {
        let mut loader = CancellableLoader::new("Working...");
        loader.abort();
        loader.tick();
        // Should not panic even when aborted
    }
}
