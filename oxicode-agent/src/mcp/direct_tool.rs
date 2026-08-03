//! Direct MCP tool → `AgentTool` bridge (Phase 3).
//!
//! Wraps a single MCP tool as a standalone `AgentTool` so the LLM can call
//! it directly (without going through the `mcp` proxy gateway). The
//! prefixed name is precomputed at construction time and stored in a
//! `String` field, because `AgentTool::name()` returns `&str` whose lifetime
//! is tied to `&self`.

use super::McpManager;
use super::content;
use super::types::{ConsentState, DirectToolDef};
use crate::tools::{AgentTool, AgentToolResult, ToolContext};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::oneshot;

/// An individual MCP tool registered as a first-class `AgentTool`.
pub struct McpDirectTool {
    /// Prefixed tool name (e.g. `"chrome_take_screenshot"`).
    prefixed_name: String,
    /// Original (unprefixed) MCP tool name.
    original_name: String,
    /// Server that provides this tool.
    server_name: String,
    /// Tool description.
    description: String,
    /// JSON Schema for parameters.
    schema: Value,
    /// Shared `McpManager` for execute + lifecycle.
    manager: Arc<McpManager>,
}

impl std::fmt::Debug for McpDirectTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpDirectTool")
            .field("name", &self.prefixed_name)
            .field("server", &self.server_name)
            .finish()
    }
}

impl McpDirectTool {
    /// Create a new direct tool from a `DirectToolDef` and a shared manager.
    pub fn new(manager: Arc<McpManager>, def: DirectToolDef) -> Self {
        Self {
            prefixed_name: def.prefixed_name,
            original_name: def.original_name,
            server_name: def.server_name,
            description: def.description,
            schema: def
                .input_schema
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
            manager,
        }
    }
}

#[async_trait]
impl AgentTool for McpDirectTool {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn label(&self) -> &str {
        &self.original_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    fn essential(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, String> {
        // 1. Consent gate (Phase 3)
        if self.manager.consent().check(&self.original_name) == ConsentState::Deny {
            return Ok(AgentToolResult::error(format!(
                "Tool '{}' is denied by consent policy",
                self.original_name
            )));
        }

        // 2. Ensure the server is connected, then call the tool.
        match self
            .manager
            .call_tool(&self.original_name, params, Some(&self.server_name))
            .await
        {
            Ok(result) => {
                // 3. Reset idle timer so the server doesn't disconnect
                //    immediately after a direct-tool call.
                self.manager.reset_idle_timer(&self.server_name);

                if result.is_error {
                    let text = content::transform_mcp_content(&result.content);
                    Ok(AgentToolResult::error(format!("Error: {}", text)))
                } else {
                    let text = content::transform_mcp_content(&result.content);
                    Ok(AgentToolResult::success(text))
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_prefixed_name_reference() {
        // The critical compile-time guarantee: `name()` returns `&str`
        // borrowed from a `String` field owned by `self`.
        let manager: Arc<McpManager> = Arc::new(McpManager::new_no_spawn());
        let def = DirectToolDef {
            prefixed_name: "chrome_take_screenshot".to_string(),
            original_name: "take_screenshot".to_string(),
            server_name: "chrome".to_string(),
            description: "Take a screenshot".to_string(),
            input_schema: Some(serde_json::json!({"type": "object"})),
        };
        let tool = McpDirectTool::new(manager, def);
        assert_eq!(tool.name(), "chrome_take_screenshot");
        assert_eq!(tool.label(), "take_screenshot");
    }
}
