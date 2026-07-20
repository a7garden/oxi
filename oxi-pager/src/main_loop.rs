// Main event loop — entry point for the pager.

use crate::state::{PagerState, SharedState};
use parking_lot::RwLock;
use std::future::Future;
use std::sync::Arc;

/// Run the pager main loop.
///
/// Creates a `SharedState` and passes it to the inner TUI closure so the
/// old TUI can populate pager state from agent events and call the pager's
/// render function. Future PRs will migrate the event loop into this module.
pub async fn run<A, F, Fut>(app: A, inner: F) -> anyhow::Result<()>
where
    A: Send + 'static,
    F: FnOnce(SharedState, A) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let state: SharedState = Arc::new(RwLock::new(PagerState::default()));
    inner(state, app).await
}
