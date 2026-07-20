// Main event loop — entry point for the pager.

use crate::state::{PagerState, SharedState};
use parking_lot::RwLock;
use std::future::Future;
use std::sync::Arc;

/// Run the pager main loop.
///
/// `_app` is the oxi-cli `App` (generic to avoid circular deps).
/// `inner` is an async callback that runs the real TUI (e.g. `run_tui_interactive_impl`).
/// In this pass-through phase the pager initializes its state then
/// delegates to the inner TUI. Future PRs will migrate logic from
/// `inner` into the pager's reducer + render path.
pub async fn run<A, F, Fut>(_app: A, inner: F) -> anyhow::Result<()>
where
    A: Send + 'static,
    F: FnOnce(A) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let _state: SharedState = Arc::new(RwLock::new(PagerState::default()));
    inner(_app).await
}
