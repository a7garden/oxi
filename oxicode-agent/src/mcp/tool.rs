//! MCP proxy tool.
//!
//! Implements [`AgentTool`] for the unified `mcp` tool that acts as a
//! gateway to all MCP servers.  The LLM calls this single tool with
//! different parameters to search, describe, connect, and call MCP tools.
//!
//! The proxy tool is the **fallback / search** path. Specific tools may
//! also be registered directly via [`crate::mcp::McpDirectTool`] (Phase 3); the two
//! paths coexist.

use crate::tools::{AgentTool, AgentToolResult, ToolContext};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::oneshot;

use super::McpManager;
use super::content;

/// The unified MCP gateway tool.
///
/// Parameters follow the pi-mcp-adapter convention:
///
/// ```text
/// Mode: tool (call) > connect > describe > search > server (list) > action > nothing (status)
/// ```
pub struct McpTool {
    manager: Arc<McpManager>,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool").finish()
    }
}

impl McpTool {
    /// Create a new MCP tool with the given manager.
    pub fn new(manager: Arc<McpManager>) -> Self {
        Self { manager }
    }

    /// Get a clone of the underlying manager (used by other code paths
    /// that need to read McpManager state, e.g. the TUI dashboard).
    pub fn manager(&self) -> Arc<McpManager> {
        Arc::clone(&self.manager)
    }
}

#[async_trait]
impl AgentTool for McpTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn label(&self) -> &str {
        "MCP"
    }

    fn description(&self) -> &str {
        "MCP gateway - connect to MCP servers and call their tools. Non-MCP Pi tools should be called directly, not through mcp.\n\nUsage:\n  mcp({ }) → status\n  mcp({ tool: \"name\", args: '{}' }) → call tool\n  mcp({ connect: \"server\" }) → connect\n  mcp({ search: \"query\" }) → search\n  mcp({ describe: \"tool\" }) → describe\n  mcp({ server: \"name\" }) → list tools\n\nMode: tool > connect > describe > search > server > status"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tool": {
                    "type": "string",
                    "description": "Tool name to call (e.g. 'xcodebuild_list_sims')"
                },
                "args": {
                    "type": "string",
                    "description": "Arguments as JSON string (e.g. '{\"key\": \"value\"}')"
                },
                "connect": {
                    "type": "string",
                    "description": "Server name to connect (lazy connect + metadata refresh)"
                },
                "describe": {
                    "type": "string",
                    "description": "Tool name to describe (shows parameters)"
                },
                "search": {
                    "type": "string",
                    "description": "Search tools by name/description"
                },
                "regex": {
                    "type": "boolean",
                    "description": "Treat search as regex (default: substring match)"
                },
                "server": {
                    "type": "string",
                    "description": "Filter to specific server (also disambiguates tool calls)"
                },
                "action": {
                    "type": "string",
                    "description": "Action: 'ui-messages' to retrieve prompts/intents from UI sessions"
                }
            },
            "additionalProperties": false
        })
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
        let obj = params
            .as_object()
            .ok_or("Parameters must be a JSON object")?;

        // Parse optional args
        let parsed_args = if let Some(args_val) = obj.get("args").and_then(|v| v.as_str()) {
            serde_json::from_str::<Value>(args_val)
                .map_err(|e| format!("Invalid args JSON: {}", e))?
        } else {
            Value::Object(serde_json::Map::new())
        };

        // ── Route by priority ─────────────────────────────────────
        if let Some(action) = obj.get("action").and_then(|v| v.as_str()) {
            return self.handle_action(action, obj).await;
        }

        if let Some(tool_name) = obj.get("tool").and_then(|v| v.as_str()) {
            let server = obj.get("server").and_then(|v| v.as_str());
            return self.handle_call(tool_name, parsed_args, server).await;
        }

        if let Some(server_name) = obj.get("connect").and_then(|v| v.as_str()) {
            return self.handle_connect(server_name).await;
        }

        if let Some(tool_name) = obj.get("describe").and_then(|v| v.as_str()) {
            return self.handle_describe(tool_name).await;
        }

        if let Some(query) = obj.get("search").and_then(|v| v.as_str()) {
            let regex = obj.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
            let server = obj.get("server").and_then(|v| v.as_str());
            return self.handle_search(query, regex, server).await;
        }

        if let Some(server_name) = obj.get("server").and_then(|v| v.as_str()) {
            return self.handle_list(server_name).await;
        }

        // Default: status
        self.handle_status().await
    }
}

// ── Action handlers ──────────────────────────────────────────────────

impl McpTool {
    async fn handle_status(&self) -> Result<AgentToolResult, String> {
        let status = self.manager.status().await;
        Ok(AgentToolResult::success(status))
    }

    async fn handle_connect(&self, server_name: &str) -> Result<AgentToolResult, String> {
        let result = self
            .manager
            .connect(server_name)
            .await
            .map_err(|e| e.to_string())?;
        Ok(AgentToolResult::success(result))
    }

    async fn handle_describe(&self, tool_name: &str) -> Result<AgentToolResult, String> {
        let result = self
            .manager
            .describe(tool_name)
            .await
            .map_err(|e| e.to_string())?;
        Ok(AgentToolResult::success(result))
    }

    async fn handle_search(
        &self,
        query: &str,
        regex: bool,
        server: Option<&str>,
    ) -> Result<AgentToolResult, String> {
        let result = self
            .manager
            .search(query, regex, server)
            .await
            .map_err(|e| e.to_string())?;
        Ok(AgentToolResult::success(result))
    }

    async fn handle_list(&self, server_name: &str) -> Result<AgentToolResult, String> {
        let result = self
            .manager
            .list_tools(server_name)
            .await
            .map_err(|e| e.to_string())?;
        Ok(AgentToolResult::success(result))
    }

    async fn handle_call(
        &self,
        tool_name: &str,
        args: Value,
        server: Option<&str>,
    ) -> Result<AgentToolResult, String> {
        let result = self
            .manager
            .call_tool(tool_name, args, server)
            .await
            .map_err(|e| e.to_string())?;

        if result.is_error {
            let text = content::transform_mcp_content(&result.content);
            Ok(AgentToolResult::error(format!("Error: {}", text)))
        } else {
            let text = content::transform_mcp_content(&result.content);
            Ok(AgentToolResult::success(text))
        }
    }

    async fn handle_action(
        &self,
        action: &str,
        obj: &serde_json::Map<String, Value>,
    ) -> Result<AgentToolResult, String> {
        let server = obj.get("server").and_then(|v| v.as_str()).unwrap_or("");
        match action {
            "ui-messages" => Ok(AgentToolResult::success(
                "No UI session messages available.",
            )),
            "list-resources" => {
                if server.is_empty() {
                    return Ok(AgentToolResult::error(String::from(
                        "list-resources requires 'server'",
                    )));
                }
                match self.manager.list_resources(server).await {
                    Ok(resources) => Ok(AgentToolResult::success(
                        serde_json::to_string_pretty(&resources).unwrap_or_default(),
                    )),
                    Err(e) => Ok(AgentToolResult::error(format!(
                        "list_resources('{}') failed: {}",
                        server, e
                    ))),
                }
            }
            "read-resource" => {
                let uri = obj.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                if server.is_empty() || uri.is_empty() {
                    return Ok(AgentToolResult::error(String::from(
                        "read-resource requires 'server' and 'uri'",
                    )));
                }
                match self.manager.read_resource(server, uri).await {
                    Ok(content) => Ok(AgentToolResult::success(content::transform_mcp_content(
                        &content,
                    ))),
                    Err(e) => Ok(AgentToolResult::error(format!(
                        "read_resource('{}','{}') failed: {}",
                        server, uri, e
                    ))),
                }
            }
            "list-prompts" => {
                if server.is_empty() {
                    return Ok(AgentToolResult::error(String::from(
                        "list-prompts requires 'server'",
                    )));
                }
                match self.manager.list_prompts(server).await {
                    Ok(prompts) => {
                        let summary: Vec<String> = prompts
                            .iter()
                            .map(|p| {
                                format!(
                                    "- {}{}",
                                    p.name,
                                    p.description
                                        .as_deref()
                                        .map(|d| format!(" — {}", d))
                                        .unwrap_or_default()
                                )
                            })
                            .collect();
                        Ok(AgentToolResult::success(summary.join("\n")))
                    }
                    Err(e) => Ok(AgentToolResult::error(format!(
                        "list_prompts('{}') failed: {}",
                        server, e
                    ))),
                }
            }
            "get-prompt" => {
                let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = obj
                    .get("arguments")
                    .and_then(|v| v.as_object())
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect::<std::collections::HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                if server.is_empty() || name.is_empty() {
                    return Ok(AgentToolResult::error(String::from(
                        "get-prompt requires 'server' and 'name'",
                    )));
                }
                match self.manager.get_prompt(server, name, arguments).await {
                    Ok(messages) => Ok(AgentToolResult::success(
                        serde_json::to_string_pretty(&messages).unwrap_or_default(),
                    )),
                    Err(e) => Ok(AgentToolResult::error(format!(
                        "get_prompt('{}','{}') failed: {}",
                        server, name, e
                    ))),
                }
            }
            _ => Ok(AgentToolResult::error(format!(
                "Unknown action: '{}'. Supported: 'ui-messages', 'list-resources', 'read-resource', 'list-prompts', 'get-prompt'",
                action
            ))),
        }
    }
}
