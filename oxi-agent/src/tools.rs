//! Agent tools system

use crate::types::ToolDefinition;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use parking_lot::RwLock;

/// Tool handler function type
pub type ToolHandler = Arc<dyn Fn(String) -> Pin<Box<dyn std::future::Future<Output = String> + Send>> + Send + Sync>;

/// Agent tool registry and execution
#[derive(Default)]
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, ToolDefinition>>,
    handlers: RwLock<HashMap<String, ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool with its handler
    pub fn register<T>(&self, tool: ToolDefinition, handler: T)
    where
        T: Fn(String) -> Pin<Box<dyn std::future::Future<Output = String> + Send>> + Send + Sync + 'static,
    {
        let name = tool.name.clone();
        let handler = Arc::new(handler) as ToolHandler;
        self.tools.write().insert(name.clone(), tool);
        self.handlers.write().insert(name, handler);
    }

    /// Register a simple sync tool
    pub fn register_sync<F>(&self, name: String, description: String, handler: F)
    where
        F: Fn(String) -> String + Send + Sync + 'static,
    {
        let definition = ToolDefinition::new(name.clone(), description, HashMap::new());
        let handler = Arc::new(move |_input: String| {
            let result = handler(_input);
            Box::pin(async move { result }) as Pin<Box<dyn std::future::Future<Output = String> + Send>>
        }) as ToolHandler;
        self.tools.write().insert(name.clone(), definition);
        self.handlers.write().insert(name, handler);
    }

    /// Get all registered tools
    pub fn get_tools(&self) -> Vec<ToolDefinition> {
        self.tools.read().values().cloned().collect()
    }

    /// Check if a tool is registered
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.read().contains_key(name)
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: String) -> Option<String> {
        let handler = self.handlers.read().get(name)?.clone();
        Some(handler(input).await)
    }
}
