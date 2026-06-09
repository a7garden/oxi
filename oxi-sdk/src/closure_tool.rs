//! ClosureTool — ergonomic custom tool from a closure

use async_trait::async_trait;
use oxi_agent::{AgentTool, AgentToolResult, ToolContext, ToolError};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Handler function type for ClosureTool.
pub type ToolHandler =
    Arc<dyn Fn(Value, &ToolContext) -> Result<AgentToolResult, ToolError> + Send + Sync>;

/// Async handler function type for ClosureTool.
pub type AsyncToolHandler = Arc<
    dyn Fn(
            Value,
            &ToolContext,
        ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, ToolError>> + Send>>
        + Send
        + Sync,
>;

/// A tool defined by a closure function.
///
/// Created via [`ClosureTool::new_sync`] or [`ClosureTool::new_async`].
/// For ergonomic creation, use the macros or AgentBuilder's `custom_tool` method.
pub struct ClosureTool {
    name: String,
    description: String,
    schema: Value,
    handler: AsyncToolHandler,
}

impl ClosureTool {
    /// Create a new sync tool from a closure.
    ///
    /// The closure receives `(params: Value, ctx: &ToolContext)`.
    pub fn new_sync(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        handler: impl Fn(Value, &ToolContext) -> Result<AgentToolResult, ToolError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        #[allow(clippy::type_complexity)]
        let handler_arc: Arc<
            dyn Fn(Value, &ToolContext) -> Result<AgentToolResult, ToolError> + Send + Sync,
        > = Arc::new(handler);
        Self {
            name: name.into(),
            description: description.into(),
            schema,
            handler: Arc::new(move |params, ctx| {
                let result = handler_arc(params, ctx);
                Box::pin(async move { result })
            }),
        }
    }

    /// Create a new async tool from a closure.
    ///
    /// The closure receives `(params: Value, ctx: &ToolContext)` and returns a Future.
    pub fn new_async(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        handler: impl Fn(
            Value,
            &ToolContext,
        )
            -> Pin<Box<dyn Future<Output = Result<AgentToolResult, ToolError>> + Send>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
            handler: Arc::new(handler),
        }
    }
}

#[async_trait]
impl AgentTool for ClosureTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        (self.handler)(params, ctx).await
    }
}
