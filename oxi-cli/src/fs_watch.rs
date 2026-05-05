//! Filesystem watching utilities.
//!
//! Provides wrapper around notify for filesystem watching with error handling.

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// Delay between watcher creation retries on error (milliseconds)
pub const FS_WATCH_RETRY_DELAY_MS: u64 = 5000;

/// Errors that can occur during filesystem watching
#[derive(Debug, thiserror::Error)]
pub enum FsWatchError {
    #[error("Failed to create watcher: {0}")]
    CreateError(String),

    #[error("Failed to watch path: {0}")]
    WatchError(String),

    #[error("Watch event channel closed unexpectedly")]
    ChannelClosed,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Watcher handle that can be used to stop watching
pub struct FsWatcher {
    /// Keep: watcher must stay alive for the duration of observation; dropping it stops watching.
    #[allow(dead_code)]
    watcher: Option<RecommendedWatcher>,
    stop_tx: Option<Sender<()>>,
}

impl FsWatcher {
    /// Create a new filesystem watcher for the given path.
    ///
    /// Returns a watcher handle and a receiver for filesystem events.
    pub fn new(
        path: impl AsRef<Path>,
    ) -> Result<(Self, Receiver<notify::Result<notify::Event>>), FsWatchError> {
        let (event_tx, event_rx) = channel();
        let (stop_tx, _stop_rx) = channel();

        let path = path.as_ref().to_path_buf();

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                // Ignore send errors (channel closed)
                let _ = event_tx.send(res);
            },
            Config::default()
                .with_poll_interval(Duration::from_secs(1)),
        )
        .map_err(|e| FsWatchError::CreateError(e.to_string()))?;

        watcher
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|e| FsWatchError::WatchError(e.to_string()))?;

        // Spawn a background task to handle stop signal
        let _stop_tx = stop_tx.clone();
        let watcher = Some(watcher);

        Ok((
            Self {
                watcher,
                stop_tx: Some(stop_tx),
            },
            event_rx,
        ))
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        // Signal stop and let the watcher be dropped
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Close a watcher, ignoring any errors.
pub fn close_watcher(watcher: Option<FsWatcher>) {
    // Dropping the watcher will close it
    drop(watcher);
}

/// Watch a path with error handler callback.
///
/// Returns a watcher handle and event receiver, or None on error.
pub fn watch_with_error_handler(
    path: impl AsRef<Path>,
    on_error: impl FnOnce(FsWatchError) + Send + 'static,
) -> Option<(FsWatcher, Receiver<notify::Result<notify::Event>>)> {
    match FsWatcher::new(path) {
        Ok((watcher, rx)) => Some((watcher, rx)),
        Err(e) => {
            on_error(e);
            None
        }
    }
}

/// Simple event handler for filesystem changes
pub trait FsWatchHandler: Send {
    /// Handle a filesystem event
    fn handle(&mut self, event: notify::Event);

    /// Handle an error
    fn handle_error(&mut self, error: FsWatchError);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path();

        let result = FsWatcher::new(path);
        assert!(result.is_ok(), "Should create watcher without error");
    }

    #[test]
    fn test_close_watcher() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path();

        let result = FsWatcher::new(path);
        assert!(result.is_ok());

        let (watcher, _rx) = result.unwrap();
        // Should not panic when closing
        close_watcher(Some(watcher));
    }

    #[test]
    fn test_watch_with_error_handler() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path();

        let error_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let error_called_clone = error_called.clone();

        let result = watch_with_error_handler(path, move |e| {
            error_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            eprintln!("Watch error: {}", e);
        });

        assert!(result.is_some());
        assert!(!error_called.load(std::sync::atomic::Ordering::SeqCst));
    }
}
