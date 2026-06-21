//! Middleware module — Hook chain management

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Bridge connecting the legacy hooks API to the middleware pipeline.
pub mod bridge;
pub mod builtins;
pub mod plugin;

pub use bridge::build_hooks;
pub use builtins::{
    ContentFilterMiddleware, LoggingMiddleware, RateLimitMiddleware, TokenBudgetMiddleware,
};
pub use plugin::{PluginLoader, PluginManifest};

/// Middleware execution phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewarePhase {
    /// Before the request is sent to the LLM.
    BeforeLlm,
    /// After the LLM response is received.
    AfterLlm,
    /// Before a tool is invoked.
    BeforeTool,
    /// After a tool invocation completes.
    AfterTool,
    /// Before the agent run begins.
    BeforeRun,
    /// After the agent run completes.
    AfterRun,
}

/// Middleware data — context passed to middlewares per phase.
#[derive(Clone)]
pub enum MiddlewareData {
    /// Payload for the [`MiddlewarePhase::BeforeLlm`] phase.
    BeforeLlm {
        /// Outgoing messages that will be sent to the model.
        messages: Vec<oxi_ai::Message>,
        /// Identifier of the model that will receive the request.
        model_id: String,
    },
    /// Payload for the [`MiddlewarePhase::AfterLlm`] phase.
    AfterLlm {
        /// Text returned by the model.
        response_text: String,
        /// Token usage from the LLM response, if available.
        tokens_used: Option<crate::observability::TokenUsage>,
    },
    /// Payload for the [`MiddlewarePhase::BeforeTool`] phase.
    BeforeTool {
        /// Name of the tool about to be invoked.
        tool_name: String,
        /// Parameters that will be passed to the tool.
        params: Value,
    },
    /// Payload for the [`MiddlewarePhase::AfterTool`] phase.
    AfterTool {
        /// Name of the tool that was invoked.
        tool_name: String,
        /// Parameters that were passed to the tool.
        params: Value,
        /// Serialized result returned by the tool.
        result: String,
    },
    /// Payload for the [`MiddlewarePhase::BeforeRun`] phase.
    BeforeRun {
        /// User prompt that initiated the run.
        prompt: String,
    },
    /// Payload for the [`MiddlewarePhase::AfterRun`] phase.
    AfterRun {
        /// Final response produced by the run.
        response: String,
        /// Whether the run completed successfully.
        success: bool,
        /// Wall-clock duration of the run, in milliseconds.
        duration_ms: u64,
    },
}

/// Context passed to middleware during execution.
pub struct MiddlewareContext {
    /// Phase that triggered this invocation.
    pub phase: MiddlewarePhase,
    /// Identifier of the agent whose pipeline is executing.
    pub agent_id: String,
    /// Distributed trace context, if tracing is enabled.
    pub trace_id: Option<crate::observability::TraceId>,
    /// Phase-specific payload for this invocation.
    pub data: MiddlewareData,
}

impl MiddlewareContext {
    /// Create a context with no trace ID.
    pub fn new(phase: MiddlewarePhase, agent_id: &str, data: MiddlewareData) -> Self {
        Self {
            phase,
            agent_id: agent_id.to_string(),
            trace_id: None,
            data,
        }
    }

    /// Create context with an explicit trace ID.
    pub fn with_trace(
        phase: MiddlewarePhase,
        agent_id: &str,
        trace_id: crate::observability::TraceId,
        data: MiddlewareData,
    ) -> Self {
        Self {
            phase,
            agent_id: agent_id.to_string(),
            trace_id: Some(trace_id),
            data,
        }
    }

    /// Returns the tool name when the phase is tool-related, else `None`.
    pub fn tool_name(&self) -> Option<&str> {
        match &self.data {
            MiddlewareData::BeforeTool { tool_name, .. } => Some(tool_name),
            MiddlewareData::AfterTool { tool_name, .. } => Some(tool_name),
            _ => None,
        }
    }
}

/// Middleware action — determines how the pipeline continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareAction {
    /// Allow the pipeline to proceed to the next middleware.
    Continue,
    /// Block the current action (see [`MiddlewareResult::block`]).
    Block,
    /// Terminate the entire agent loop.
    Terminate,
}

/// Result of middleware execution.
#[derive(Clone)]
pub struct MiddlewareResult {
    /// How the pipeline should proceed after this middleware runs.
    pub action: MiddlewareAction,
    /// If set, the pipeline replaces the current data with this before continuing.
    pub modified_data: Option<MiddlewareData>,
    /// Human-readable explanation, typically set on block or terminate.
    pub reason: Option<String>,
}

impl std::fmt::Debug for MiddlewareResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareResult")
            .field("action", &self.action)
            .field("has_modified_data", &self.modified_data.is_some())
            .field("reason", &self.reason)
            .finish()
    }
}

impl MiddlewareResult {
    /// Continue without modification.
    pub fn pass() -> Self {
        Self {
            action: MiddlewareAction::Continue,
            modified_data: None,
            reason: None,
        }
    }
    /// Continue but replace the middleware data (e.g. modify params).
    pub fn modify(data: MiddlewareData) -> Self {
        Self {
            action: MiddlewareAction::Continue,
            modified_data: Some(data),
            reason: None,
        }
    }
    /// Block the current action with a reason.
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            action: MiddlewareAction::Block,
            modified_data: None,
            reason: Some(reason.into()),
        }
    }
    /// Terminate the agent loop with a reason.
    pub fn terminate(reason: impl Into<String>) -> Self {
        Self {
            action: MiddlewareAction::Terminate,
            modified_data: None,
            reason: Some(reason.into()),
        }
    }
    /// Returns `true` if the action is [`MiddlewareAction::Continue`].
    pub fn is_continue(&self) -> bool {
        self.action == MiddlewareAction::Continue
    }
    /// Returns `true` if the action is [`MiddlewareAction::Block`].
    pub fn is_block(&self) -> bool {
        self.action == MiddlewareAction::Block
    }
    /// Returns `true` if the action is [`MiddlewareAction::Terminate`].
    pub fn is_terminate(&self) -> bool {
        self.action == MiddlewareAction::Terminate
    }
}

/// Middleware trait — implement this to add behavior to the agent pipeline.
pub trait Middleware: Send + Sync {
    /// Human-readable name of this middleware, used for logging and lookup.
    fn name(&self) -> &str;
    /// Phases at which this middleware wishes to be invoked.
    fn phases(&self) -> Vec<MiddlewarePhase>;
    /// Inspect the context for the current phase and return a result.
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>>;
}

/// Ordered chain of middlewares executed phase by phase.
#[derive(Default)]
pub struct MiddlewarePipeline {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewarePipeline {
    /// Create an empty pipeline.
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }
    /// Append an owned middleware to the chain, returning the pipeline for chaining.
    pub fn push<M: Middleware + 'static>(mut self, mw: M) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }
    /// Append a shared ([`Arc`]) middleware to the chain.
    pub fn add_arc(mut self, mw: Arc<dyn Middleware>) -> Self {
        self.middlewares.push(mw);
        self
    }
    /// Run every middleware registered for the context's phase, in order.
    pub async fn execute(&self, ctx: &MiddlewareContext) -> MiddlewareResult {
        for mw in &self.middlewares {
            if !mw.phases().contains(&ctx.phase) {
                continue;
            }
            let result = mw.handle(ctx).await;
            if !result.is_continue() {
                return result;
            }
        }
        MiddlewareResult::pass()
    }
    /// Names of the registered middlewares, in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.middlewares.iter().map(|m| m.name()).collect()
    }
    /// Returns `true` if no middlewares are registered.
    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestMw;
    impl Middleware for TestMw {
        fn name(&self) -> &str {
            "test"
        }
        fn phases(&self) -> Vec<MiddlewarePhase> {
            vec![MiddlewarePhase::BeforeTool]
        }
        fn handle<'a>(
            &'a self,
            _ctx: &'a MiddlewareContext,
        ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
            Box::pin(async { MiddlewareResult::pass() })
        }
    }

    #[tokio::test]
    async fn test_pipeline() {
        let p = MiddlewarePipeline::new().push(TestMw);
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "a1",
            MiddlewareData::BeforeTool {
                tool_name: "read".into(),
                params: serde_json::json!({}),
            },
        );
        assert!(p.execute(&ctx).await.is_continue());
    }

    #[tokio::test]
    async fn test_pipeline_skips_unrelated_phases() {
        struct BeforeToolOnly;
        impl Middleware for BeforeToolOnly {
            fn name(&self) -> &str {
                "before_only"
            }
            fn phases(&self) -> Vec<MiddlewarePhase> {
                vec![MiddlewarePhase::BeforeTool]
            }
            fn handle<'a>(
                &'a self,
                _ctx: &'a MiddlewareContext,
            ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
                Box::pin(async { MiddlewareResult::block("should not run") })
            }
        }
        let p = MiddlewarePipeline::new().push(BeforeToolOnly);
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::AfterLlm,
            "a1",
            MiddlewareData::AfterLlm {
                response_text: "hello".into(),
                tokens_used: None,
            },
        );
        // Should pass because the middleware is not registered for AfterLlm
        assert!(p.execute(&ctx).await.is_continue());
    }

    #[test]
    fn test_middleware_result_modify() {
        let data = MiddlewareData::BeforeTool {
            tool_name: "read".into(),
            params: serde_json::json!({"path": "/tmp"}),
        };
        let result = MiddlewareResult::modify(data);
        assert!(result.is_continue());
        assert!(result.modified_data.is_some());
    }

    #[test]
    fn test_middleware_context_with_trace() {
        use crate::observability::TraceId;
        let trace_id = TraceId::new();
        let ctx = MiddlewareContext::with_trace(
            MiddlewarePhase::BeforeTool,
            "a1",
            trace_id,
            MiddlewareData::BeforeTool {
                tool_name: "read".into(),
                params: serde_json::json!({}),
            },
        );
        assert_eq!(ctx.trace_id, Some(trace_id));
        assert_eq!(ctx.agent_id, "a1");
    }
}
