//! Agent tools system
//!
//! This module provides the tool abstraction layer and built-in tools.

use crate::types::ToolDefinition;
use async_trait::async_trait;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Result type for tool execution
pub type ToolError = String;

/// Result of tool execution
#[derive(Debug)]
pub struct AgentToolResult {
    pub success: bool,
    pub output: String,
    pub metadata: Option<serde_json::Value>,
}

impl AgentToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            metadata: None,
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            success: false,
            output: output.into(),
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

impl fmt::Display for AgentToolResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.output)
    }
}

/// Callback type for progress updates
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Core trait for all agent tools
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Tool name (used in function calls)
    fn name(&self) -> &str;

    /// Human-readable label
    fn label(&self) -> &str;

    /// Description for the model
    fn description(&self) -> &str;

    /// JSON Schema for parameters
    fn parameters_schema(&self) -> Value;

    /// Execute the tool
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError>;

    /// Called with progress updates during execution.
    /// Tools can override this to emit streaming updates.
    fn on_progress(&self, _callback: ProgressCallback) {
        // Default no-op
    }

    /// Convert to ToolDefinition
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: serde_json::from_value(self.parameters_schema()).unwrap_or_default(),
        }
    }
}

// Built-in tools
pub mod read;
pub mod write;
pub mod edit;
pub mod bash;
pub mod grep;
pub mod find;
pub mod ls;

// Re-export for convenience
pub use read::ReadTool;
pub use write::WriteTool;
pub use edit::EditTool;
pub use bash::BashTool;
pub use grep::GrepTool;
pub use find::FindTool;
pub use ls::LsTool;

/// Tool registry for managing available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<parking_lot::RwLock<std::collections::HashMap<String, Arc<dyn AgentTool>>>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Register a tool
    pub fn register(&self, tool: impl AgentTool + 'static) {
        let name = tool.name().to_string();
        self.tools.write().insert(name, Arc::new(tool));
    }

    /// Register a tool that is already wrapped in an `Arc`.
    /// This is the primary path for extensions that produce `Arc<dyn AgentTool>`.
    pub fn register_arc(&self, tool: Arc<dyn AgentTool>) {
        let name = tool.name().to_string();
        self.tools.write().insert(name, tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.read().get(name).cloned()
    }

    /// List all registered tool names
    pub fn names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

    /// Get all tool definitions
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.read().values().map(|t| t.to_definition()).collect()
    }

    /// Get all tools as a slice
    pub fn get_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.read().values().cloned().collect()
    }

    /// Create a registry with all built-in tools
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        registry.register(ReadTool::new());
        registry.register(WriteTool::new());
        registry.register(EditTool::new());
        registry.register(BashTool::new());
        registry.register(GrepTool::new());
        registry.register(FindTool::new());
        registry.register(LsTool::new());
        registry
    }
}