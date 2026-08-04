//! [`HookMiddleware`](crate::middleware::HookMiddleware) — bridge [`HookRunner`](crate::ports::HookRunner) into the existing
//! [`MiddlewarePipeline`] so Pre/PostToolUse hooks fire through the
//! same path as audit/authorizer middlewares.
//!
//! SubagentStop is fired here as a side effect: when an `AfterTool` call
//! has `tool_name == "subagent"`, we additionally invoke
//! `runner.run(SubagentStop, ctx)` so users only need a single matcher
//! rule. SessionStart / SessionEnd / Stop / Notification are NOT
//! fired here — those are product-lifecycle events owned by the
//! composition root.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::middleware::{
    Middleware, MiddlewareAction, MiddlewareContext, MiddlewareData, MiddlewarePhase,
    MiddlewareResult,
};
use crate::ports::{HookContext, HookEvent, HookRunner};

const SUBAGENT_TOOL_NAME: &str = "subagent";

/// Middleware that routes `BeforeTool` / `AfterTool` phases through the
/// registered [`HookRunner`] as `PreToolUse` / `PostToolUse` events.
pub struct HookMiddleware {
    runner: Arc<dyn HookRunner>,
    session_id: Option<String>,
    session_cwd: Option<PathBuf>,
}

impl std::fmt::Debug for HookMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookMiddleware")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl HookMiddleware {
    /// Wrap a [`HookRunner`] as a middleware. The runner is usually the
    /// engine's registered `CommandHookRunner` (port #16).
    pub fn new(runner: Arc<dyn HookRunner>) -> Self {
        Self {
            runner,
            session_id: None,
            session_cwd: None,
        }
    }

    /// Tag every emitted [`HookContext`] with a session id + cwd so hook
    /// scripts can identify which session triggered them.
    pub fn with_session(mut self, id: String, cwd: PathBuf) -> Self {
        self.session_id = Some(id);
        self.session_cwd = Some(cwd);
        self
    }
}

impl Middleware for HookMiddleware {
    fn name(&self) -> &str {
        "HookMiddleware"
    }

    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::BeforeTool, MiddlewarePhase::AfterTool]
    }

    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        let (event, tool_name, args, result) = match &ctx.data {
            MiddlewareData::BeforeTool { tool_name, params } => (
                HookEvent::PreToolUse,
                tool_name.clone(),
                params.clone(),
                None,
            ),
            MiddlewareData::AfterTool {
                tool_name,
                params: _,
                result,
            } => (
                HookEvent::PostToolUse,
                tool_name.clone(),
                serde_json::Value::Null,
                Some(result.clone()),
            ),
            _ => return Box::pin(async { MiddlewareResult::pass() }),
        };

        let runner = Arc::clone(&self.runner);
        let session_id = self.session_id.clone();
        let session_cwd = self.session_cwd.clone();
        let is_after = matches!(ctx.phase, MiddlewarePhase::AfterTool);

        Box::pin(async move {
            let hook_ctx = HookContext {
                event,
                tool_name: Some(tool_name.clone()),
                tool_args: if args.is_null() { None } else { Some(args) },
                tool_result: result,
                is_error: None,
                session_id,
                session_cwd,
                extra: None,
            };
            let outcome = runner.run(event, &hook_ctx).await;
            if outcome.block {
                return MiddlewareResult {
                    action: MiddlewareAction::Block,
                    modified_data: None,
                    reason: outcome.reason.or(Some(format!(
                        "hook {:?} denied tool `{}`",
                        event, tool_name
                    ))),
                };
            }

            // SubagentStop is fired as a side effect of the `subagent`
            // tool completing. We don't block on it (SubagentStop is
            // notification-only by design); we just route through so
            // users can react to subagent completion.
            if is_after && tool_name == SUBAGENT_TOOL_NAME {
                let sub_ctx = HookContext {
                    event: HookEvent::SubagentStop,
                    ..hook_ctx
                };
                let _ = runner.run(HookEvent::SubagentStop, &sub_ctx).await;
            }

            MiddlewareResult::pass()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::HookOutcome;
    use crate::ports::inmem::InMemoryHookRunner;
    use serde_json::json;

    fn before_tool_ctx(tool_name: &str) -> MiddlewareContext {
        MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "agent-1",
            MiddlewareData::BeforeTool {
                tool_name: tool_name.into(),
                params: json!({"command": "ls"}),
            },
        )
    }

    fn after_tool_ctx(tool_name: &str, result: &str) -> MiddlewareContext {
        MiddlewareContext::new(
            MiddlewarePhase::AfterTool,
            "agent-1",
            MiddlewareData::AfterTool {
                tool_name: tool_name.into(),
                params: json!({}),
                result: result.into(),
            },
        )
    }

    #[tokio::test]
    async fn before_tool_pass_through_when_no_handlers() {
        let mw = HookMiddleware::new(Arc::new(InMemoryHookRunner::new()));
        let result = mw.handle(&before_tool_ctx("bash")).await;
        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn before_tool_block_short_circuits_tool() {
        let runner = InMemoryHookRunner::new();
        runner.on(|_, _| HookOutcome {
            block: true,
            reason: Some("deny".into()),
            ..Default::default()
        });
        let mw = HookMiddleware::new(Arc::new(runner));
        let result = mw.handle(&before_tool_ctx("bash")).await;
        assert!(matches!(result.action, MiddlewareAction::Block));
        assert_eq!(result.reason.as_deref(), Some("deny"));
    }

    #[tokio::test]
    async fn after_subagent_fires_subagent_stop() {
        let runner = InMemoryHookRunner::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        runner.on(move |event, _| {
            if event == HookEvent::SubagentStop {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            HookOutcome::default()
        });
        let mw = HookMiddleware::new(Arc::new(runner));
        mw.handle(&after_tool_ctx("subagent", "{}")).await;
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn after_non_subagent_does_not_fire_subagent_stop() {
        let runner = InMemoryHookRunner::new();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        runner.on(move |event, _| {
            if event == HookEvent::SubagentStop {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            HookOutcome::default()
        });
        let mw = HookMiddleware::new(Arc::new(runner));
        mw.handle(&after_tool_ctx("read", "ok")).await;
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
