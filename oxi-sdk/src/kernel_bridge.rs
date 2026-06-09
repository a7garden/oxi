//! Kernel tool bridge — allows oxios kernel tools to be plugged into the SDK.
//!
//! oxios-kernel implements `KernelToolProvider` to register its tools
//! (exec, memory, browser, persona, etc.) into the SDK's agent builder.

use oxi_agent::ToolRegistry;
use std::collections::HashMap;
use std::path::PathBuf;

/// Context provided to kernel tool providers during registration.
///
/// Contains the metadata that kernel tools need to operate correctly
/// within an oxios agent session.
///
/// The `metadata` map is an extension point for kernel-specific data
/// (e.g., `space_id`, `cspace_name`, `seed_id`) without requiring SDK
/// changes for every new field.
#[derive(Debug, Clone)]
pub struct KernelToolContext {
    /// Agent's workspace directory.
    pub workspace_dir: PathBuf,
    /// oxios agent identifier.
    pub agent_id: String,
    /// Session identifier, if available.
    pub session_id: Option<String>,
    /// CSpace-based permission list.
    pub permissions: Vec<String>,
    /// Extension metadata for kernel-specific data.
    ///
    /// Keys are conventionally lowercase snake_case (e.g., `"space_id"`,
    /// `"cspace_name"`, `"seed_id"`). Consumers should use
    /// [`Self::get_meta`] / [`Self::get_meta_str`] for typed access.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl KernelToolContext {
    /// Create a new context with the given workspace and agent ID.
    pub fn new(workspace_dir: impl Into<PathBuf>, agent_id: impl Into<String>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            agent_id: agent_id.into(),
            session_id: None,
            permissions: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Set the session ID.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the permissions list.
    pub fn with_permissions(mut self, permissions: Vec<String>) -> Self {
        self.permissions = permissions;
        self
    }

    /// Add an extension metadata entry.
    ///
    /// # Example
    ///
    /// ```rust
    /// use oxi_sdk::KernelToolContext;
    ///
    /// let ctx = KernelToolContext::new("/workspace", "agent-001")
    ///     .with_meta("space_id", serde_json::json!("test-space"))
    ///     .with_meta("cspace_name", serde_json::json!("full"));
    /// ```
    pub fn with_meta(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Get a metadata value by key.
    pub fn get_meta(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }

    /// Get a metadata value as a string.
    pub fn get_meta_str(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).and_then(|v| v.as_str())
    }
}

/// Trait for providing kernel-level tools to the SDK.
///
/// oxios-kernel implements this trait to bridge its native tools
/// (exec, memory, browser, persona, etc.) into the oxi agent tool registry.
///
/// # Example
///
/// ```ignore
/// use oxi_sdk::{KernelToolProvider, KernelToolContext};
/// use oxi_agent::ToolRegistry;
///
/// struct MyKernelBridge;
///
/// impl KernelToolProvider for MyKernelBridge {
///     fn tool_names(&self) -> Vec<&str> {
///         vec!["exec", "memory"]
///     }
///
///     fn register_tools(&self, registry: &ToolRegistry, ctx: &KernelToolContext) {
///         registry.register(ExecTool::new(ctx.agent_id.clone()));
///         registry.register(MemoryTool::new(ctx.agent_id.clone()));
///     }
/// }
/// ```
pub trait KernelToolProvider: Send + Sync {
    /// Return the names of tools this provider will register.
    fn tool_names(&self) -> Vec<&str>;

    /// Register tools into the given registry.
    ///
    /// The `context` provides agent-specific metadata (workspace, agent_id,
    /// session, permissions) that tools may need at initialization time.
    fn register_tools(&self, registry: &ToolRegistry, context: &KernelToolContext);
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oxi_agent::{AgentTool, AgentToolResult, ToolContext, ToolError};
    use serde_json::Value;

    struct MockKernelTool {
        name: String,
    }

    #[async_trait]
    impl AgentTool for MockKernelTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn label(&self) -> &str {
            "mock"
        }
        fn description(&self) -> &str {
            "A mock kernel tool"
        }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &ToolContext,
        ) -> Result<AgentToolResult, ToolError> {
            Ok(AgentToolResult::success("mock result"))
        }
    }

    struct MockKernelBridge;

    impl KernelToolProvider for MockKernelBridge {
        fn tool_names(&self) -> Vec<&str> {
            vec!["exec", "memory"]
        }

        fn register_tools(&self, registry: &ToolRegistry, ctx: &KernelToolContext) {
            registry.register(MockKernelTool {
                name: format!("exec_{}", ctx.agent_id),
            });
            registry.register(MockKernelTool {
                name: format!("memory_{}", ctx.agent_id),
            });
        }
    }

    #[test]
    fn test_kernel_tool_context_builder() {
        let ctx = KernelToolContext::new("/workspace", "agent-001")
            .with_session("sess-123")
            .with_permissions(vec!["read".into(), "write".into()]);

        assert_eq!(ctx.workspace_dir, PathBuf::from("/workspace"));
        assert_eq!(ctx.agent_id, "agent-001");
        assert_eq!(ctx.session_id, Some("sess-123".to_string()));
        assert_eq!(ctx.permissions, vec!["read", "write"]);
    }

    #[test]
    fn test_kernel_bridge_registers_tools() {
        let bridge = MockKernelBridge;
        let registry = ToolRegistry::new();
        let ctx = KernelToolContext::new("/workspace", "agent-001");

        bridge.register_tools(&registry, &ctx);

        let names = registry.names();
        assert!(names.contains(&"exec_agent-001".to_string()));
        assert!(names.contains(&"memory_agent-001".to_string()));
    }
}
