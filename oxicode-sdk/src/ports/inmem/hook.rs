//! In-memory [`HookRunner`] for tests and headless products.

use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ports::{HookContext, HookEvent, HookOutcome, HookRunner};

type Handler = Arc<dyn Fn(HookEvent, &HookContext) -> HookOutcome + Send + Sync>;

/// Test hook runner — handlers are registered as plain closures.
/// All handlers fire on every event; the first one that returns
/// `block = true` short-circuits.
#[derive(Default)]
pub struct InMemoryHookRunner {
    handlers: Mutex<Vec<Handler>>,
}

impl std::fmt::Debug for InMemoryHookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryHookRunner")
            .field("handler_count", &self.handlers.lock().len())
            .finish()
    }
}

impl InMemoryHookRunner {
    /// Create a runner with no handlers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Handlers run in registration order.
    pub fn on<F>(&self, f: F)
    where
        F: Fn(HookEvent, &HookContext) -> HookOutcome + Send + Sync + 'static,
    {
        self.handlers.lock().push(Arc::new(f));
    }
}

impl HookRunner for InMemoryHookRunner {
    fn run<'a>(
        &'a self,
        event: HookEvent,
        ctx: &'a HookContext,
    ) -> Pin<Box<dyn Future<Output = HookOutcome> + Send + 'a>> {
        let handlers = self.handlers.lock().clone();
        let ctx = ctx.clone();
        Box::pin(async move {
            let mut out = HookOutcome::default();
            for h in &handlers {
                let step = h(event, &ctx);
                if step.block {
                    return HookOutcome {
                        block: true,
                        reason: step.reason.or(out.reason),
                        override_content: step.override_content.or(out.override_content),
                    };
                }
                if step.override_content.is_some() {
                    out.override_content = step.override_content;
                }
                if step.reason.is_some() {
                    out.reason = step.reason;
                }
            }
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handler_fires_and_blocks() {
        let runner = InMemoryHookRunner::new();
        runner.on(|_, _| HookOutcome {
            block: true,
            reason: Some("blocked".into()),
            ..Default::default()
        });
        let ctx = HookContext::default();
        let out = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(out.block);
        assert_eq!(out.reason.as_deref(), Some("blocked"));
    }

    #[tokio::test]
    async fn empty_runner_returns_default() {
        let runner = InMemoryHookRunner::new();
        let ctx = HookContext::default();
        let out = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(!out.block);
    }

    #[tokio::test]
    async fn block_short_circuits() {
        let runner = InMemoryHookRunner::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = Arc::clone(&counter);
        let c3 = Arc::clone(&counter);
        runner.on(move |_, _| {
            c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HookOutcome::default()
        });
        runner.on(|_, _| HookOutcome {
            block: true,
            ..Default::default()
        });
        runner.on(move |_, _| {
            // Should not run.
            c3.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
            HookOutcome::default()
        });
        let ctx = HookContext::default();
        let out = runner.run(HookEvent::PreToolUse, &ctx).await;
        assert!(out.block);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
