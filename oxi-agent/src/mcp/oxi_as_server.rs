//! oxi-as-server — expose oxi tools over MCP (PR-B3).
//!
//! Lets oxi act as an MCP server so external agents can call oxi's tools.
//! Also defines the hooks system (PR-B4) for server→client notifications.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;

use crate::tools::ToolRegistry;

/// Context passed to a tool call handler.
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    /// Identifier of the originating session.
    pub session_id: String,
}

/// Hook event sent from server to client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum HookEvent {
    /// Cancel an in-flight tool call by id.
    Cancel {
        /// The id of the call to cancel.
        call_id: String,
        /// Human-readable reason for cancellation.
        reason: String,
    },
    /// Pause a session's tool dispatch loop.
    Pause {
        /// The session to pause.
        session_id: String,
    },
    /// Resume a paused session's tool dispatch loop.
    Resume {
        /// The session to resume.
        session_id: String,
    },
}

/// Handler for a single tool exposed by the server.
///
/// Implementations may be backed by a registry tool, a closure, or any
/// other source. The server routes inbound tool calls and hook events to
/// the registered handler whose [`tool_id`](Self::tool_id) matches.
#[async_trait]
pub trait ToolServerHandler: Send + Sync + 'static {
    /// Stable identifier used to route calls and hooks.
    fn tool_id(&self) -> &str;
    /// Short human-readable description shown in tool listings.
    fn description(&self) -> String;
    /// Optional JSON Schema describing the call arguments.
    fn input_schema(&self) -> Option<Value> {
        None
    }
    /// Execute a tool call and return its serialized result.
    async fn handle_call(&self, ctx: ToolCallContext, args: Value) -> Result<Value, String>;
    /// React to a hook event targeting this handler. Default no-op.
    async fn handle_hook(&self, _session_id: &str, _event: HookEvent) {}
}

/// MCP server exposing oxi tools.
pub struct ToolServer {
    registry: Arc<ToolRegistry>,
    handlers: RwLock<HashMap<String, Arc<dyn ToolServerHandler>>>,
}

impl ToolServer {
    /// Build a new server backed by the given tool registry.
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            handlers: RwLock::new(HashMap::new()),
        }
    }

    /// Register an additional handler that overrides registry tools by id.
    pub fn register_handler(&self, handler: Arc<dyn ToolServerHandler>) {
        self.handlers
            .write()
            .insert(handler.tool_id().to_string(), handler);
    }

    /// Snapshot of every tool (registry + custom handlers) the server exposes.
    pub fn list_tools(&self) -> Vec<ToolEntry> {
        let mut tools: Vec<ToolEntry> = self
            .registry
            .names()
            .into_iter()
            .map(|name| ToolEntry {
                name: name.clone(),
                description: self
                    .registry
                    .get(&name)
                    .map(|t| t.description().to_string())
                    .unwrap_or_default(),
                input_schema: self.registry.get(&name).map(|t| t.parameters_schema()),
            })
            .collect();
        for (name, handler) in self.handlers.read().iter() {
            tools.push(ToolEntry {
                name: name.clone(),
                description: handler.description(),
                input_schema: handler.input_schema(),
            });
        }
        tools
    }

    /// Dispatch a tool call, preferring a custom handler then the registry.
    ///
    /// The handler lookup is bounded to a synchronous scope so the
    /// `RwLockReadGuard` is dropped before the handler's `.await` runs.
    pub async fn call_tool(
        &self,
        name: &str,
        ctx: ToolCallContext,
        args: Value,
    ) -> Result<Value, String> {
        let handler = self.handlers.read().get(name).cloned();
        if let Some(handler) = handler {
            return handler.handle_call(ctx, args).await;
        }
        let tool = self
            .registry
            .get(name)
            .ok_or_else(|| format!("Tool '{}' not found", name))?;
        let result = tool
            .execute("oxi-server", args, None, &Default::default())
            .await
            .map_err(|e| format!("Tool execution failed: {}", e))?;
        Ok(serde_json::json!({ "success": result.success, "output": result.output }))
    }

    /// Forward a hook event to the matching handler, if any.
    ///
    /// Handler lookup is bounded to a synchronous scope so the
    /// `RwLockReadGuard` is dropped before `handle_hook`'s `.await` runs.
    pub async fn dispatch_hook(&self, tool_id: &str, session_id: &str, event: HookEvent) {
        let handler = self.handlers.read().get(tool_id).cloned();
        if let Some(handler) = handler {
            handler.handle_hook(session_id, event).await;
        }
    }
}

/// Entry in a server's tool listing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolEntry {
    /// Tool identifier used in `call_tool`.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Optional JSON Schema for the call arguments.
    pub input_schema: Option<Value>,
}

/// Adapter wrapping a single registry tool as a [`ToolServerHandler`].
pub struct RegistryAdapter {
    registry: Arc<ToolRegistry>,
    tool_name: String,
}

impl RegistryAdapter {
    /// Wrap the registry tool named `tool_name` as a handler.
    pub fn new(registry: Arc<ToolRegistry>, tool_name: impl Into<String>) -> Self {
        Self {
            registry,
            tool_name: tool_name.into(),
        }
    }
}

#[async_trait]
impl ToolServerHandler for RegistryAdapter {
    fn tool_id(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> String {
        self.registry
            .get(&self.tool_name)
            .map(|t| t.description().to_string())
            .unwrap_or_default()
    }
    fn input_schema(&self) -> Option<Value> {
        self.registry
            .get(&self.tool_name)
            .map(|t| t.parameters_schema())
    }
    async fn handle_call(&self, _ctx: ToolCallContext, args: Value) -> Result<Value, String> {
        let tool = self
            .registry
            .get(&self.tool_name)
            .ok_or_else(|| format!("Tool '{}' not found", self.tool_name))?;
        let result = tool
            .execute("oxi-server", args, None, &Default::default())
            .await
            .map_err(|e| format!("Tool execution failed: {}", e))?;
        Ok(serde_json::json!({ "success": result.success, "output": result.output }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_server_new() {
        let registry = Arc::new(ToolRegistry::new());
        let server = ToolServer::new(registry);
        assert!(server.list_tools().is_empty());
    }

    #[test]
    fn test_hook_event_serialization() {
        let event = HookEvent::Cancel {
            call_id: "abc".into(),
            reason: "timeout".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "Cancel");
        assert_eq!(json["call_id"], "abc");
    }

    #[tokio::test]
    async fn test_call_unknown_tool_errors() {
        let registry = Arc::new(ToolRegistry::new());
        let server = ToolServer::new(registry);
        let result = server
            .call_tool(
                "nonexistent",
                ToolCallContext {
                    session_id: "test".into(),
                },
                serde_json::json!({}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
