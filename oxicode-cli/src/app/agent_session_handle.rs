//! `SessionSwapper` — cheap, thread-safe handle to the "current"
//! `AgentSession` for the TUI. Replaces the raw `AgentSessionHandle`
//! everywhere the TUI / worker need to observe a session that may be
//! swapped mid-run (e.g. `/sessions <id>` resume).
//!
//! Construction is one-time at TUI startup (wraps the initial handle);
//! `swap` is called by the resume worker; readers clone via `current()`.
//! `parking_lot::Mutex` is the synchronization primitive — `current()`
//! is a hot read (every frame and every agent dispatch) and a Mutex
//! read is < 10 ns. We use an `Arc<Mutex<AgentSessionHandle>>` (not
//! `ArcSwap`) to avoid pulling a new dep for one feature.

use std::sync::Arc;

use crate::app::agent_session::AgentSessionHandle;

/// Cheap, thread-safe wrapper around the live `AgentSessionHandle`.
///
/// `current()` returns a cheap clone (the inner `Arc<AgentSession>`
/// is shared). `swap(new)` atomically replaces the inner handle;
/// the next `current()` call observes the new session.
#[derive(Clone)]
pub struct SessionSwapper {
    current: Arc<parking_lot::Mutex<AgentSessionHandle>>,
}

impl std::fmt::Debug for SessionSwapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSwapper")
            .field("session_id", &"<redacted: contains Arc<AgentSession>>")
            .finish_non_exhaustive()
    }
}

impl SessionSwapper {
    /// Wrap an initial handle. The TUI does this once at startup.
    pub fn new(initial: AgentSessionHandle) -> Self {
        Self {
            current: Arc::new(parking_lot::Mutex::new(initial)),
        }
    }

    /// Get a cheap clone of the current handle. Hot read.
    pub fn current(&self) -> AgentSessionHandle {
        self.current.lock().clone()
    }

    /// Atomically replace the current handle. The next `current()`
    /// call observes the new session.
    pub fn swap(&self, new: AgentSessionHandle) {
        *self.current.lock() = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_session::AgentSession;
    use crate::store::session::SessionManager;
    use crate::store::settings::Settings;
    use oxicode_agent::Agent;
    use oxicode_sdk::{Model, Provider, ProviderError, ProviderEvent};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context as TaskContext, Poll};

    // ── Mock Provider (hermetic: no file I/O, no auth, no network) ──

    /// Minimal mock provider that produces an empty stream.
    struct MockProvider;

    struct EmptyStream;

    impl futures::Stream for EmptyStream {
        type Item = ProviderEvent;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(None)
        }
    }

    impl Provider for MockProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a oxicode_sdk::Context,
            _options: Option<oxicode_sdk::StreamOptions>,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
                            ProviderError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                Ok::<_, ProviderError>(Box::pin(EmptyStream)
                    as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
            })
        }
    }

    fn dummy_handle() -> AgentSessionHandle {
        let provider = Arc::new(MockProvider);
        let config = oxicode_agent::AgentConfig::new("test/dummy-model");
        let agent = Arc::new(Agent::new(
            provider,
            config,
            Arc::new(oxicode_agent::ToolRegistry::new()),
        ));
        let sm = SessionManager::in_memory("/tmp");
        let session = AgentSession::new(
            agent,
            Settings::default(),
            sm,
            "/tmp".to_string(),
            crate::SessionState::default(),
        );
        session.clone_handle()
    }

    #[test]
    fn new_initial_handle_is_returned_by_current() {
        let h = dummy_handle();
        let id_before = h.session_id();
        let swapper = SessionSwapper::new(h);
        assert_eq!(swapper.current().session_id(), id_before);
    }

    #[test]
    fn swap_replaces_visible_handle() {
        let h1 = dummy_handle();
        let h2 = dummy_handle();
        let id1 = h1.session_id();
        let id2 = h2.session_id();
        assert_ne!(id1, id2);
        let swapper = SessionSwapper::new(h1);
        assert_eq!(swapper.current().session_id(), id1);
        swapper.swap(h2);
        assert_eq!(swapper.current().session_id(), id2);
    }

    #[test]
    fn swap_is_visible_across_threads() {
        // Two threads: one calls current() in a loop, one swaps. The
        // current() thread must never see a half-swapped handle. We
        // verify by checking that every observed session_id is one of
        // the two real ids (no torn reads).
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let h1 = dummy_handle();
        let h2 = dummy_handle();
        let id1 = h1.session_id();
        let id2 = h2.session_id();
        let h1_writer = h1.clone();
        let h2_writer = h2.clone();

        let swapper = StdArc::new(SessionSwapper::new(h1));
        let stop = StdArc::new(AtomicUsize::new(0));
        let swapper_reader = swapper.clone();
        let swapper_writer = swapper.clone();
        let stop_reader = stop.clone();
        let stop_writer = stop.clone();

        let reader = std::thread::spawn(move || {
            let mut seen = Vec::with_capacity(10_000);
            while stop_reader.load(Ordering::Relaxed) == 0 {
                let id = swapper_reader.current().session_id();
                assert!(id == id1 || id == id2, "torn read: unexpected id");
                seen.push(id);
            }
            seen
        });

        let writer = std::thread::spawn(move || {
            for i in 0..10_000 {
                swapper_writer.swap(if i & 1 == 0 {
                    h1_writer.clone()
                } else {
                    h2_writer.clone()
                });
            }
            stop_writer.store(1, Ordering::Relaxed);
        });
        writer.join().unwrap();
        let _seen = reader.join().unwrap();
        // We don't assert on the contents of `seen` (timing-dependent);
        // the per-iteration assert is the actual contract.
    }
}
