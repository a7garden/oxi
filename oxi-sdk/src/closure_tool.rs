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
impl ClosureTool {
    /// Typed async closure helper.
    ///
    /// Generates the parameters schema via `schemars::schema_for!(Args)` and
    /// forwards deserialized args to `handler`. Reduces boilerplate for
    /// SDK consumers implementing custom tools.
    pub fn new_typed_async<Args, F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: F,
    ) -> Self
    where
        Args: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        F: Fn(Args, &ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<AgentToolResult, ToolError>> + Send + 'static,
    {
        use std::sync::Arc;
        let schema = serde_json::to_value(schemars::schema_for!(Args))
            .unwrap_or_else(|_| serde_json::json!({"type": "object"}));
        let name_owned: String = name.into();
        let handler: Arc<F> = Arc::new(handler);
        Self::new_async(name_owned, description, schema, move |params, ctx| {
            match serde_json::from_value::<Args>(params) {
                Ok(args) => {
                    let handler = handler.clone();
                    let fut = handler(args, ctx);
                    Box::pin(fut)
                }
                Err(e) => Box::pin(async move {
                    Err::<AgentToolResult, ToolError>(format!(
                        "invalid args for typed closure: {e}"
                    ))
                }),
            }
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, JsonSchema)]
    struct EchoArgs {
        message: String,
    }

    fn ctx() -> ToolContext {
        ToolContext::default()
    }

    #[tokio::test]
    async fn new_typed_async_generates_schema() {
        let tool = ClosureTool::new_typed_async::<EchoArgs, _, _>(
            "echo",
            "echo back",
            |args, _ctx| async move { Ok(AgentToolResult::success(args.message)) },
        );
        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.description(), "echo back");
        let schema = tool.parameters_schema();
        assert!(
            schema.get("properties").is_some(),
            "schema missing properties: {schema}"
        );
    }

    #[tokio::test]
    async fn new_typed_async_deserializes_args() {
        let tool = ClosureTool::new_typed_async::<EchoArgs, _, _>(
            "echo",
            "echo back",
            |args, _ctx| async move { Ok(AgentToolResult::success(args.message)) },
        );
        let result = tool
            .execute(
                "call_1",
                serde_json::json!({"message": "hello"}),
                None,
                &ctx(),
            )
            .await
            .expect("execute ok");
        assert!(result.success);
        assert_eq!(result.output, "hello");
    }

    #[tokio::test]
    async fn new_typed_async_rejects_invalid_args() {
        let tool = ClosureTool::new_typed_async::<EchoArgs, _, _>(
            "echo",
            "echo back",
            |args, _ctx| async move { Ok(AgentToolResult::success(args.message)) },
        );
        let err = tool
            .execute("call_1", serde_json::json!({}), None, &ctx())
            .await
            .expect_err("missing field should fail");
        assert!(
            err.contains("invalid args for typed closure"),
            "unexpected error: {err}"
        );
    }
}
