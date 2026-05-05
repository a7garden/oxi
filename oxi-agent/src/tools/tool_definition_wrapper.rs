/// Tool definition wrapping utilities
/// Provides adapters for converting between tool representations.

use crate::tools::{AgentTool, AgentToolResult, ToolError};
use crate::types::ToolDefinition;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::oneshot;

/// A dynamically-constructed tool from a definition and execution function.
///
/// This is useful for wrapping external tool definitions (e.g., from extensions
/// or plugins) into the `AgentTool` interface without requiring a dedicated struct.
pub struct DynamicTool {
    name: String,
    label: String,
    description: String,
    parameters: Value,
    execute_fn: Arc<
        dyn Fn(&str, Value, Option<oneshot::Receiver<()>>) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
        > + Send
        + Sync,
    >,
}

impl DynamicTool {
    /// Create a new dynamic tool from components.
    pub fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execute_fn: impl Fn(&str, Value, Option<oneshot::Receiver<()>>) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
            > + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: description.into(),
            parameters,
            execute_fn: Arc::new(execute_fn),
        }
    }

    /// Create a `DynamicTool` from a `ToolDefinition` with a simple execution function.
    pub fn from_definition(
        def: ToolDefinition,
        execute_fn: impl Fn(&str, Value, Option<oneshot::Receiver<()>>) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
            > + Send
            + Sync
            + 'static,
    ) -> Self {
        let name_for_label = def.name.clone();
        let schema = serde_json::to_value(&def.input_schema).unwrap_or(Value::Object(Default::default()));
        Self {
            name: def.name,
            label: name_for_label, // Use name as label fallback
            description: def.description,
            parameters: schema,
            execute_fn: Arc::new(execute_fn),
        }
    }
}

#[async_trait]
impl AgentTool for DynamicTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        (self.execute_fn)(tool_call_id, params, signal).await
    }
}

/// Trait for tool definitions that can be wrapped into `AgentTool`.
pub trait ToolDefinitionLike: Send + Sync {
/// TODO: document this function.
    fn tool_name(&self) -> &str;
/// TODO: document this function.
    fn tool_label(&self) -> &str;
/// TODO: document this function.
    fn tool_description(&self) -> &str;
/// TODO: document this function.
    fn tool_parameters(&self) -> Value;
/// TODO: document this function.
    fn tool_execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
    >;
}

/// Wrap a `ToolDefinitionLike` into an `Arc<dyn AgentTool>`.
pub fn wrap_tool_definition<T: ToolDefinitionLike + 'static>(def: T) -> Arc<dyn AgentTool> {
    Arc::new(DefinitionWrapper(def))
}

/// Internal wrapper that adapts `ToolDefinitionLike` to `AgentTool`.
struct DefinitionWrapper<T>(T);

#[async_trait]
impl<T: ToolDefinitionLike + 'static> AgentTool for DefinitionWrapper<T> {
    fn name(&self) -> &str {
        self.0.tool_name()
    }

    fn label(&self) -> &str {
        self.0.tool_label()
    }

    fn description(&self) -> &str {
        self.0.tool_description()
    }

    fn parameters_schema(&self) -> Value {
        self.0.tool_parameters()
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        self.0.tool_execute(tool_call_id, params, signal).await
    }
}

/// Create a minimal `ToolDefinition` from an `AgentTool`.
///
/// This is useful for internal registries that operate on definitions
/// even when tools are provided as trait objects.
pub fn create_tool_definition_from_agent_tool(tool: &dyn AgentTool) -> ToolDefinition {
    let schema_map: std::collections::HashMap<String, Value> = {
        let schema = tool.parameters_schema();
        if let Value::Object(map) = schema {
            map.into_iter().collect()
        } else {
            let mut map = std::collections::HashMap::new();
            map.insert("type".to_string(), Value::String("object".to_string()));
            map
        }
    };

    ToolDefinition::new(tool.name(), tool.description(), schema_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_tool_definition_from_agent_tool() {
        struct TestTool;
        
        #[async_trait]
        impl AgentTool for TestTool {
            fn name(&self) -> &str { "test_tool" }
            fn label(&self) -> &str { "Test Tool" }
            fn description(&self) -> &str { "A test tool" }
            fn parameters_schema(&self) -> Value {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    }
                })
            }
            async fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _signal: Option<oneshot::Receiver<()>>,
            ) -> Result<AgentToolResult, ToolError> {
                Ok(AgentToolResult::success("test"))
            }
        }

        let tool = TestTool;
        let def = create_tool_definition_from_agent_tool(&tool);
        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
    }

    #[test]
    fn test_dynamic_tool_creation() {
        let tool = DynamicTool::new(
            "dynamic_test",
            "Dynamic Test",
            "A dynamic test tool",
            serde_json::json!({"type": "object"}),
            |_id, _params, _signal| {
                Box::pin(async { Ok(AgentToolResult::success("dynamic result")) })
            },
        );

        assert_eq!(tool.name(), "dynamic_test");
        assert_eq!(tool.label(), "Dynamic Test");
        assert_eq!(tool.description(), "A dynamic test tool");
    }

    #[tokio::test]
    async fn test_dynamic_tool_execution() {
        let tool = DynamicTool::new(
            "exec_test",
            "Exec Test",
            "Exec test tool",
            serde_json::json!({"type": "object"}),
            |_id, params, _signal| {
                let result = format!("got: {}", params);
                Box::pin(async move { Ok(AgentToolResult::success(result)) })
            },
        );

        let result = tool.execute("call_1", serde_json::json!({"key": "value"}), None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().output, "got: {\"key\":\"value\"}");
    }

    struct MockDefLike;

    impl ToolDefinitionLike for MockDefLike {
        fn tool_name(&self) -> &str { "mock_def" }
        fn tool_label(&self) -> &str { "Mock Definition" }
        fn tool_description(&self) -> &str { "Mock description" }
        fn tool_parameters(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn tool_execute(
            &self,
            _tool_call_id: &str,
            _params: Value,
            _signal: Option<oneshot::Receiver<()>>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
        > {
            Box::pin(async { Ok(AgentToolResult::success("mock result")) })
        }
    }

    #[test]
    fn test_wrap_tool_definition() {
        let wrapped = wrap_tool_definition(MockDefLike);
        assert_eq!(wrapped.name(), "mock_def");
        assert_eq!(wrapped.label(), "Mock Definition");
    }

    #[tokio::test]
    async fn test_wrapped_tool_execution() {
        let wrapped = wrap_tool_definition(MockDefLike);
        let result = wrapped.execute("call_1", Value::Null, None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().output, "mock result");
    }
}
