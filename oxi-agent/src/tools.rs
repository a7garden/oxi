//! Agent tool trait and utilities
//!
//! This module defines the AgentTool trait that tools must implement
//! to be used within the agent runtime.

use serde_json::Value as JsonValue;
use std::pin::Pin;
use std::sync::Arc;
use crate::types::AgentToolResult;

/// Boxed future type for tool execution
pub type ToolFuture = Pin<Box<dyn std::future::Future<Output = Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

/// Trait for implementing agent tools
///
/// Tools are the primary way for agents to interact with the external world.
/// Each tool must provide:
/// - A unique name and description
/// - A JSON schema for its parameters
/// - An execute method returning a boxed future
///
/// # Example
///
/// ```ignore
/// use oxi_agent::{AgentTool, AgentToolResult};
/// use serde_json::json;
/// use std::pin::Pin;
///
/// struct Calculator;
///
/// impl AgentTool for Calculator {
///     fn name(&self) -> &str { "calculator" }
///     fn label(&self) -> &str { "Calculator" }
///     fn description(&self) -> &str { "Perform mathematical calculations" }
///     fn parameters_schema(&self) -> &JsonValue {
///         static SCHEMA: JsonValue = json!({
///             "type": "object",
///             "properties": {
///                 "expression": {
///                     "type": "string",
///                     "description": "Mathematical expression to evaluate"
///                 }
///             },
///             "required": ["expression"]
///         });
///         &SCHEMA
///     }
///
///     fn execute(
///         &self,
///         tool_call_id: &str,
///         params: JsonValue,
///         signal: Option<tokio::sync::oneshot::Receiver<()>>,
///     ) -> Pin<Box<dyn std::future::Future<Output = Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>> + Send>> {
///         Box::pin(async move {
///             // Implementation here
///             Ok(AgentToolResult::json(vec![ContentBlock::Text { 
///                 text: "42".to_string() 
///             }]))
///         })
///     }
/// }
/// ```
pub trait AgentTool: Send + Sync {
    /// Unique identifier for this tool
    fn name(&self) -> &str;

    /// Human-readable label for display in UIs
    fn label(&self) -> &str;

    /// Detailed description of what this tool does
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters
    fn parameters_schema(&self) -> &JsonValue;

    /// Execute the tool with the given parameters
    ///
    /// # Arguments
    /// * `tool_call_id` - Unique identifier for this tool call
    /// * `params` - JSON object containing the tool parameters
    /// * `signal` - Optional cancellation signal receiver
    ///
    /// # Returns
    /// * A boxed future that resolves to `Ok(AgentToolResult)` on success
    /// * A boxed error if execution fails
    fn execute(
        &self,
        tool_call_id: &str,
        params: JsonValue,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> ToolFuture;

    /// Validate the given parameters against the schema
    ///
    /// Default implementation always returns Ok.
    /// Override to provide custom validation logic.
    fn validate_params(&self, params: &JsonValue) -> Result<(), ToolValidationError> {
        // Basic JSON schema validation could be added here
        // For now, just check it's an object
        if !params.is_object() {
            return Err(ToolValidationError::InvalidParams(
                "Parameters must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }
}

/// Error types for tool validation
#[derive(Debug, Clone)]
pub enum ToolValidationError {
    /// Parameters are not valid JSON
    InvalidParams(String),
    /// Required parameter is missing
    MissingRequired(String),
    /// Parameter type mismatch
    TypeMismatch {
        param: String,
        expected: String,
        actual: String,
    },
    /// Parameter failed custom validation
    ValidationFailed(String),
}

impl std::fmt::Display for ToolValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidParams(msg) => write!(f, "Invalid parameters: {}", msg),
            Self::MissingRequired(param) => write!(f, "Missing required parameter: {}", param),
            Self::TypeMismatch {
                param,
                expected,
                actual,
            } => write!(
                f,
                "Type mismatch for '{}': expected {}, got {}",
                param, expected, actual
            ),
            Self::ValidationFailed(msg) => write!(f, "Validation failed: {}", msg),
        }
    }
}

impl std::error::Error for ToolValidationError {}

/// Box<dyn AgentTool> type alias for trait objects
pub type DynAgentTool = dyn AgentTool;

/// Create a boxed tool trait object
pub fn make_tool<T: AgentTool + 'static>(tool: T) -> Box<dyn AgentTool> {
    Box::new(tool)
}

/// Create an Arc-wrapped boxed tool
pub fn make_arc_tool<T: AgentTool + 'static>(tool: T) -> Arc<dyn AgentTool> {
    Arc::new(tool)
}

/// Extension trait for executing tools with async/await syntax
pub trait AgentToolExt: AgentTool {
    /// Execute the tool and await the result
    fn execute_boxed(
        &self,
        tool_call_id: &str,
        params: JsonValue,
        signal: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> ToolFuture {
        self.execute(tool_call_id, params, signal)
    }
}

impl<T: AgentTool + ?Sized> AgentToolExt for T {}