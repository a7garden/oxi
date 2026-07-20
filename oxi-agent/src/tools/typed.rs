//! Typed 도구 트레이트 + AgentTool 어댑터.
//!
//! PR-A1: grok-build `Tool<Args: Deserialize + JsonSchema, Output>` 패턴의
//! 개념 차용. `AgentTool` 트레이트의 `params: serde_json::Value` 시그니처는
//! 보존(`Arc<dyn AgentTool>` 호환성 유지)하고, 도구 *구현자* 측에
//! 타입 안전 진입점을 제공.
//!
//! ## 사용 패턴
//!
//! ```ignore
//! use oxi_agent::tools::typed::{TypedTool, wrap_typed};
//! use schemars::JsonSchema;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize, JsonSchema)]
//! struct EchoArgs { message: String }
//!
//! struct EchoTool;
//!
//! impl TypedTool for EchoTool {
//!     type Args = EchoArgs;
//!     fn name(&self) -> &str { "echo" }
//!     fn description(&self) -> &str { "echo back" }
//!     async fn execute_typed(&self, _id: &str, args: Self::Args, _sig: Option<...>, _ctx: &ToolContext)
//!         -> Result<AgentToolResult, ToolError>
//!     {
//!         Ok(AgentToolResult::success(args.message))
//!     }
//! }
//!
//! // `Arc<dyn AgentTool>` 로 소거 — 기존 ToolRegistry.register_arc 경로 그대로.
//! let dyn_tool: Arc<dyn AgentTool> = wrap_typed(EchoTool);
//! ```

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize as _;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tokio::sync::oneshot;

use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode};

/// 타입 안전 도구 트레이트.
///
/// Generic + 연관 타입 `Self::Args` → **`dyn TypedTool` 으로 못 쓴다**.
/// [`TypedToolAdapter`] 가 [`AgentTool`]을 구현해 `Arc<dyn AgentTool>` 로
#[async_trait]
pub trait TypedTool: Send + Sync + 'static {
    /// LLM 에서 넘어오는 JSON 인자의 타입.
    type Args: DeserializeOwned + JsonSchema + Send + 'static;

    /// Typed 실행 진입점. 인자는 이미 deserialize 되어 전달됨.
    async fn execute_typed(
        &self,
        tool_call_id: &str,
        args: Self::Args,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError>;
}

/// [`TypedTool`] 을 [`AgentTool`] 로 소거하는 어댑터.
///
/// [`super::tool_definition_wrapper::DefinitionWrapper`] 와 같은 모양이지만
/// 인자 deserialize 지점이 `Value → <T as TypedTool>::Args` 로 강타입화.
pub struct TypedToolAdapter<T: TypedTool + ?Sized>(Arc<T>);

impl<T: TypedTool + AgentTool + ?Sized> std::fmt::Debug for TypedToolAdapter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedToolAdapter")
            .field("name", &self.0.name())
            .finish()
    }
}
#[async_trait]
impl<T: TypedTool + AgentTool + ?Sized> AgentTool for TypedToolAdapter<T> {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn label(&self) -> &str {
        self.0.label()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn essential(&self) -> bool {
        self.0.essential()
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        self.0.execution_mode()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.0.parameters_schema()
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let tool_name = self.0.name();
        let args = <T as TypedTool>::Args::deserialize(params)
            .map_err(|e| format!("invalid args for '{tool_name}': {e}"))?;
        self.0.execute_typed(tool_call_id, args, signal, ctx).await
    }
}

/// Wrap a [`TypedTool`] (which must also implement [`AgentTool`]) into an
/// `Arc<dyn AgentTool>` — the adapter delegates `name`/`description`/etc. to
/// the `AgentTool` impl and uses `TypedTool::Args` for typed deserialization.
///
/// The returned `Arc<dyn AgentTool>` is compatible with `ToolRegistry::register_arc`.
pub fn wrap_typed<T: TypedTool + AgentTool + ?Sized>(tool: Arc<T>) -> Arc<dyn AgentTool> {
    Arc::new(TypedToolAdapter(tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    struct EchoArgs {
        message: String,
    }

    struct EchoTool;

    #[async_trait]
    impl TypedTool for EchoTool {
        type Args = EchoArgs;
        async fn execute_typed(
            &self,
            _id: &str,
            args: Self::Args,
            _signal: Option<oneshot::Receiver<()>>,
            _ctx: &ToolContext,
        ) -> Result<AgentToolResult, ToolError> {
            Ok(AgentToolResult::success(args.message))
        }
    }

    #[async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn label(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echo back the message"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]})
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &ToolContext,
        ) -> Result<AgentToolResult, ToolError> {
            let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
            Ok(AgentToolResult::success(msg.to_string()))
        }
    }

    #[tokio::test]
    async fn wrap_typed_preserves_name_and_label() {
        let arc = wrap_typed::<EchoTool>(Arc::new(EchoTool));
        assert_eq!(arc.name(), "echo");
        assert_eq!(arc.label(), "echo");
        assert_eq!(arc.description(), "echo back the message");
        assert!(!arc.essential());
    }

    #[tokio::test]
    async fn parameters_schema_contains_args_field() {
        let arc = wrap_typed::<EchoTool>(Arc::new(EchoTool));
        let schema = arc.parameters_schema();
        // schemars 가 생성한 스키마는 object type + properties.message 형태.
        let obj = schema.as_object().expect("schema should be an object");
        assert!(
            obj.get("properties").is_some(),
            "schema missing properties: {schema}"
        );
    }

    #[tokio::test]
    async fn execute_deserializes_value_into_args() {
        let arc = wrap_typed::<EchoTool>(Arc::new(EchoTool));
        let ctx = ToolContext {
            workspace_dir: std::path::PathBuf::from("."),
            ..Default::default()
        };
        let result = arc
            .execute(
                "call_1",
                serde_json::json!({"message": "hello"}),
                None,
                &ctx,
            )
            .await
            .expect("execute should succeed");
        assert!(result.success);
        assert_eq!(result.output, "hello");
    }

    #[tokio::test]
    async fn execute_returns_error_on_invalid_args() {
        let arc = wrap_typed::<EchoTool>(Arc::new(EchoTool));
        let ctx = ToolContext {
            workspace_dir: std::path::PathBuf::from("."),
            ..Default::default()
        };
        // `message` 누락 → deserialize 실패.
        let result = arc
            .execute("call_1", serde_json::json!({}), None, &ctx)
            .await;
        let err = result.expect_err("should fail with missing field");
        assert!(
            err.contains("invalid args for 'echo'"),
            "unexpected error: {err}"
        );
    }
}
