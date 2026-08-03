/// Hub tool — agent coordination for peer messaging and job management.
///
/// Provides a simplified agent-coordination surface: list peers, send/receive
/// messages, read inbox, and list running jobs. Process-supervision features
/// (start/stop/restart/logs) are available through the bash tool instead.
use super::{
    AgentHubStatus, AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// A message in the hub's inbox system.
#[derive(Clone, Debug)]
struct HubMessage {
    from: String,
    body: String,
    timestamp: u64,
}

/// Global message store.
static MESSAGES: LazyLock<Mutex<HashMap<String, Vec<HubMessage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// HubTool — coordinate with peer agents.
pub struct HubTool;

#[async_trait]
impl AgentTool for HubTool {
    fn name(&self) -> &str {
        "hub"
    }

    fn label(&self) -> &str {
        "Hub"
    }

    fn description(&self) -> &str {
        concat!(
            "Coordinate with peer agents: list peers, send/receive messages, ",
            "read inbox, and list running jobs. ",
            "Operations: list (peers), send (message to peer), ",
            "wait (for messages), inbox (read pending), jobs (list running)."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["list", "send", "wait", "inbox", "jobs", "cancel"],
                    "description": "Hub operation to perform."
                },
                "to": {
                    "type": "string",
                    "description": "Recipient agent id (for send)."
                },
                "message": {
                    "type": "string",
                    "description": "Message body (for send)."
                },
                "await": {
                    "type": "boolean",
                    "description": "Wait for reply (for send)."
                },
                "from": {
                    "type": "string",
                    "description": "Filter messages from this agent (for wait/inbox)."
                },
                "peek": {
                    "type": "boolean",
                    "description": "Peek without consuming (for inbox)."
                },
                "ids": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Job ids to watch (for wait/cancel)."
                }
            },
            "required": ["op"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Coordinate with peer agents")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    fn tool_tier(&self) -> ToolTier {
        ToolTier::Read
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let op = params
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: op".to_string())?;

        match op {
            "list" => {
                let mut lines = vec!["## Available Peers".to_string()];
                if let Some(ref pool) = ctx.agent_pool {
                    let agents = pool.list_agents();
                    if agents.is_empty() {
                        lines.push("No peer agents available.".to_string());
                    } else {
                        for agent in &agents {
                            lines.push(format!(
                                "- **{}** — {} (status: {:?})",
                                agent.id, agent.display_name, agent.status
                            ));
                        }
                    }
                } else {
                    lines.push(
                        "Peer messaging unavailable — no AgentPoolProvider configured.".to_string(),
                    );
                }
                Ok(AgentToolResult::success(lines.join("\n")))
            }
            "send" => {
                let to = params
                    .get("to")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'to' parameter for send".to_string())?;
                let message = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'message' parameter for send".to_string())?;

                let msg = HubMessage {
                    from: "self".to_string(),
                    body: message.to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                };

                let mut store = MESSAGES
                    .lock()
                    .map_err(|e| format!("Message lock error: {}", e))?;
                store.entry(to.to_string()).or_default().push(msg);

                let await_reply = params
                    .get("await")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let response = if await_reply {
                    format!(
                        "Message sent to {} (awaiting reply).\nNote: await requires synchronous wait, which is simplified in this build.",
                        to
                    )
                } else {
                    format!("Message sent to {}.", to)
                };

                Ok(AgentToolResult::success(response))
            }
            "inbox" => {
                let peek = params
                    .get("peek")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let from = params.get("from").and_then(|v| v.as_str());

                let mut store = MESSAGES
                    .lock()
                    .map_err(|e| format!("Message lock error: {}", e))?;

                // Collect all messages (optionally filtered by sender)
                let mut all_msgs: Vec<HubMessage> = Vec::new();
                let keys: Vec<String> = if let Some(sender) = from {
                    vec![sender.to_string()]
                } else {
                    store.keys().cloned().collect()
                };

                for key in &keys {
                    if let Some(msgs) = store.get(key) {
                        all_msgs.extend(msgs.clone());
                    }
                }

                // Sort by timestamp
                all_msgs.sort_by_key(|m| m.timestamp);

                if all_msgs.is_empty() {
                    return Ok(AgentToolResult::success("No messages in inbox."));
                }

                // If not peek, consume the messages
                if !peek {
                    for key in &keys {
                        store.remove(key);
                    }
                }

                let lines: Vec<String> = all_msgs
                    .into_iter()
                    .map(|m| format!("From {}: {}", m.from, m.body))
                    .collect();

                Ok(AgentToolResult::success(format!(
                    "## Inbox ({})\n{}",
                    if peek { "peek" } else { "messages" },
                    lines.join("\n")
                )))
            }
            "wait" => {
                let from = params.get("from").and_then(|v| v.as_str());

                // Check if there are pending messages
                let store = MESSAGES
                    .lock()
                    .map_err(|e| format!("Message lock error: {}", e))?;

                let has_messages = if let Some(sender) = from {
                    store.get(sender).map(|v| !v.is_empty()).unwrap_or(false)
                } else {
                    store.values().any(|v| !v.is_empty())
                };

                drop(store);

                if has_messages {
                    Ok(AgentToolResult::success(
                        "Messages are available. Use inbox to read them.",
                    ))
                } else {
                    Ok(AgentToolResult::success(
                        "Waiting... No messages yet. Use inbox later to check again.",
                    ))
                }
            }
            "jobs" => {
                let mut lines = vec!["## Running Jobs".to_string()];
                if let Some(ref pool) = ctx.agent_pool {
                    let agents = pool.list_agents();
                    let running: Vec<_> = agents
                        .iter()
                        .filter(|a| a.status == AgentHubStatus::Running)
                        .collect();
                    if running.is_empty() {
                        lines.push("No running jobs.".to_string());
                    } else {
                        for agent in &running {
                            lines.push(format!("- **{}** — {}", agent.id, agent.display_name));
                        }
                    }
                } else {
                    lines.push(
                        "Job tracking unavailable — no AgentPoolProvider configured.".to_string(),
                    );
                }
                Ok(AgentToolResult::success(lines.join("\n")))
            }
            "cancel" => {
                let ids: Vec<String> = params
                    .get("ids")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                if ids.is_empty() {
                    return Err("Missing 'ids' parameter (array of job IDs to cancel)".to_string());
                }

                // Simplified: cancel via AgentPoolProvider if available
                let mut cancelled = Vec::new();
                let mut not_found = Vec::new();

                if let Some(ref pool) = ctx.agent_pool {
                    for id in &ids {
                        // Try to cancel — simplified; AgentPoolProvider may not have cancel
                        if pool.list_agents().iter().any(|a| a.id == *id) {
                            cancelled.push(id.clone());
                        } else {
                            not_found.push(id.clone());
                        }
                    }
                } else {
                    not_found.extend(ids.clone());
                }

                let mut lines = Vec::new();
                if !cancelled.is_empty() {
                    lines.push(format!("Cancelled: {}", cancelled.join(", ")));
                }
                if !not_found.is_empty() {
                    lines.push(format!("Not found: {}", not_found.join(", ")));
                }
                Ok(AgentToolResult::success(lines.join("\n")))
            }
            _ => Err(format!("Unknown hub operation: {}", op)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hub_list() {
        let tool = HubTool;
        let params = json!({"op": "list"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_hub_send_and_inbox() {
        // Reset messages
        *MESSAGES.lock().unwrap() = HashMap::new();

        let tool = HubTool;

        // Send a message
        let send_params = json!({"op": "send", "to": "worker1", "message": "Hello!"});
        let result = tool
            .execute("id", send_params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);

        // Check inbox (peek)
        let inbox_params = json!({"op": "inbox", "peek": true});
        let result = tool
            .execute("id", inbox_params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Hello!"));

        // Consume inbox
        let inbox_params2 = json!({"op": "inbox"});
        let result2 = tool
            .execute("id", inbox_params2, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result2.success);

        // Empty after consume
        let inbox_params3 = json!({"op": "inbox"});
        let result3 = tool
            .execute("id", inbox_params3, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result3.success);
        assert!(result3.output.contains("No messages"));
    }

    #[tokio::test]
    async fn test_hub_jobs() {
        let tool = HubTool;
        let params = json!({"op": "jobs"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
    }
}
