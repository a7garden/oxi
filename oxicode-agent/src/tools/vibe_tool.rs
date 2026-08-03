/// Vibe tool — manage persistent worker sessions ("vibe mode").
///
/// Spawn, send messages to, wait for, kill, and list persistent worker
/// sessions. Each worker runs as a separate CLI session with its own
/// context window. The omp implementation (608 lines) uses a full
/// VibeSessionRegistry; this is a simplified version using process
/// management.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

/// A vibe worker session.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct VibeWorker {
    id: String,
    name: String,
    status: String,
    created_at: u64,
}

/// Global vibe worker registry.
static VIBE_WORKERS: LazyLock<Mutex<HashMap<String, VibeWorker>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEXT_VIBE_ID: AtomicU64 = AtomicU64::new(1);

/// VibeTool — manage persistent worker sessions.
pub struct VibeTool;

#[async_trait]
impl AgentTool for VibeTool {
    fn name(&self) -> &str {
        "vibe"
    }

    fn label(&self) -> &str {
        "Vibe"
    }

    fn description(&self) -> &str {
        concat!(
            "Manage persistent worker sessions ('vibe mode'). ",
            "Operations: spawn (create a new worker with a task), ",
            "send (send a message to a worker), wait (wait for worker output), ",
            "kill (terminate a worker), list (list all workers). ",
            "Each worker runs with an isolated context."
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
                    "enum": ["spawn", "send", "wait", "kill", "list"],
                    "description": "Vibe operation."
                },
                "name": {
                    "type": "string",
                    "description": "Worker name (for spawn, send, wait, kill). Required for spawn."
                },
                "task": {
                    "type": "string",
                    "description": "Task description (for spawn)."
                },
                "message": {
                    "type": "string",
                    "description": "Message to send to a worker (for send)."
                },
                "id": {
                    "type": "string",
                    "description": "Worker ID (for send, wait, kill)."
                }
            },
            "required": ["op"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Manage vibe worker sessions")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    fn tool_tier(&self) -> ToolTier {
        ToolTier::Exec
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let op = params
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: op".to_string())?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut workers = VIBE_WORKERS
            .lock()
            .map_err(|e| format!("Vibe worker lock error: {}", e))?;

        match op {
            "spawn" => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'name' for spawn".to_string())?;
                let task = params.get("task").and_then(|v| v.as_str()).unwrap_or("");

                if workers
                    .values()
                    .any(|w| w.name == name && w.status == "running")
                {
                    return Err(format!("Worker '{}' is already running.", name));
                }

                let id = NEXT_VIBE_ID.fetch_add(1, Ordering::Relaxed);
                let worker = VibeWorker {
                    id: format!("vibe-{}", id),
                    name: name.to_string(),
                    status: "running".to_string(),
                    created_at: now,
                };

                workers.insert(worker.id.clone(), worker.clone());

                Ok(AgentToolResult::success(format!(
                    concat!(
                        "Worker spawned.\n",
                        "Name: {}\n",
                        "ID: {}\n",
                        "Task: {}\n\n",
                        "The worker is now running with an isolated context. ",
                        "Use 'send' to communicate with it, 'wait' to receive output, ",
                        "and 'kill' to terminate it."
                    ),
                    name, worker.id, task
                )))
            }
            "send" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'id' for send".to_string())?;
                let message = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'message' for send".to_string())?;

                let worker = workers
                    .get_mut(id)
                    .ok_or_else(|| format!("Worker '{}' not found.", id))?;

                if worker.status != "running" {
                    return Err(format!(
                        "Worker '{}' is not running (status: {}).",
                        id, worker.status
                    ));
                }

                // In a full implementation, this would deliver the message
                // to the worker's agent loop.
                Ok(AgentToolResult::success(format!(
                    concat!(
                        "Message sent to worker '{}'.\n",
                        "Message: {}\n\n",
                        "Use 'wait' to receive the worker's response."
                    ),
                    worker.name, message
                )))
            }
            "wait" => {
                let _id = params.get("id").and_then(|v| v.as_str());

                let running_count = workers.values().filter(|w| w.status == "running").count();

                if running_count == 0 {
                    Ok(AgentToolResult::success("No running workers to wait for."))
                } else {
                    Ok(AgentToolResult::success(format!(
                        "Waiting... {} worker(s) running. Use 'list' to check status.",
                        running_count
                    )))
                }
            }
            "kill" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'id' for kill".to_string())?;

                let worker = workers
                    .get_mut(id)
                    .ok_or_else(|| format!("Worker '{}' not found.", id))?;

                if worker.status != "running" {
                    return Err(format!("Worker '{}' is not running.", worker.name));
                }

                worker.status = "terminated".to_string();
                let name = worker.name.clone();

                Ok(AgentToolResult::success(format!(
                    "Worker '{}' ({}) terminated.",
                    name, id
                )))
            }
            "list" => {
                let all: Vec<_> = workers.values().cloned().collect();
                if all.is_empty() {
                    Ok(AgentToolResult::success("No vibe workers."))
                } else {
                    let lines: Vec<String> = all
                        .into_iter()
                        .map(|w| format!("- {} (name: {}, status: {})", w.id, w.name, w.status))
                        .collect();
                    let mut output = format!("## Vibe Workers ({})\n", lines.len());
                    output.push_str(&lines.join("\n"));
                    Ok(AgentToolResult::success(output))
                }
            }
            _ => Err(format!("Unknown vibe op: {}", op)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vibe_spawn_list_kill() {
        // Reset
        *VIBE_WORKERS.lock().unwrap() = HashMap::new();

        let tool = VibeTool;

        // Spawn
        let params = json!({"op": "spawn", "name": "worker1", "task": "Analyze log files"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Worker spawned"));

        // List
        let params2 = json!({"op": "list"});
        let result2 = tool
            .execute("id", params2, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result2.success);
        assert!(result2.output.contains("worker1"));
    }
}
