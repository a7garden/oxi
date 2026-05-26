//! Middleware module — Hook chain management

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
    BeforeLlm,
    AfterLlm,
    BeforeTool,
    AfterTool,
    BeforeRun,
    AfterRun,
}

/// Middleware data — context passed to middlewares per phase.
#[derive(Clone)]
pub enum MiddlewareData {
    BeforeLlm {
        messages: Vec<oxi_ai::Message>,
        model_id: String,
    },
    AfterLlm {
        response_text: String,
        /// Token usage from the LLM response, if available.
        tokens_used: Option<crate::observability::TokenUsage>,
    },
    BeforeTool {
        tool_name: String,
        params: Value,
    },
    AfterTool {
        tool_name: String,
        params: Value,
        result: String,
    },
    BeforeRun {
        prompt: String,
    },
    AfterRun {
        response: String,
        success: bool,
        duration_ms: u64,
    },
}

/// Context passed to middleware during execution.
pub struct MiddlewareContext {
    pub phase: MiddlewarePhase,
    pub agent_id: String,
    /// Distributed trace context, if tracing is enabled.
    pub trace_id: Option<crate::observability::TraceId>,
    pub data: MiddlewareData,
}

impl MiddlewareContext {
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
    Continue,
    Block,
    Terminate,
}

/// Result of middleware execution.
#[derive(Clone)]
pub struct MiddlewareResult {
    pub action: MiddlewareAction,
    /// If set, the pipeline replaces the current data with this before continuing.
    pub modified_data: Option<MiddlewareData>,
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
    pub fn is_continue(&self) -> bool {
        self.action == MiddlewareAction::Continue
    }
    pub fn is_block(&self) -> bool {
        self.action == MiddlewareAction::Block
    }
    pub fn is_terminate(&self) -> bool {
        self.action == MiddlewareAction::Terminate
    }
}

/// Middleware trait — implement this to add behavior to the agent pipeline.
pub trait Middleware: Send + Sync {
    fn name(&self) -> &str;
    fn phases(&self) -> Vec<MiddlewarePhase>;
    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>>;
}

#[derive(Default)]
pub struct MiddlewarePipeline {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewarePipeline {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }
    pub fn push<M: Middleware + 'static>(mut self, mw: M) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }
    pub fn add_arc(mut self, mw: Arc<dyn Middleware>) -> Self {
        self.middlewares.push(mw);
        self
    }
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
    pub fn names(&self) -> Vec<&str> {
        self.middlewares.iter().map(|m| m.name()).collect()
    }
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
