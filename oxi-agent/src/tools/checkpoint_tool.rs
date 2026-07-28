/// Checkpoint and Rewind tools — save/restore investigation state.
///
/// CheckpointTool saves a snapshot of the current investigation goal.
/// RewindTool restores to a previous checkpoint by accepting a findings report.
/// State is stored in-memory (process-local, not persisted).
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Internal checkpoint state.
#[derive(Clone)]
struct CheckpointState {
    goal: String,
}

/// Global checkpoint store (process-local).
static CHECKPOINT: LazyLock<Mutex<Option<CheckpointState>>> = LazyLock::new(|| Mutex::new(None));

/// CheckpointTool — create a checkpoint with an investigation goal.
pub struct CheckpointTool;

#[async_trait]
impl AgentTool for CheckpointTool {
    fn name(&self) -> &str {
        "checkpoint"
    }

    fn label(&self) -> &str {
        "Checkpoint"
    }

    fn description(&self) -> &str {
        concat!(
            "Create an investigation checkpoint to save and restore session state. ",
            "Call with a goal describing what you are investigating. ",
            "After completing the investigation, use the rewind tool with a report of findings."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "Investigation goal — what you plan to find out."
                }
            },
            "required": ["goal"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Save investigation checkpoint")
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
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let goal = params
            .get("goal")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Missing required parameter: goal".to_string())?;

        let mut store = CHECKPOINT
            .lock()
            .map_err(|e| format!("Checkpoint lock error: {}", e))?;

        if store.is_some() {
            return Err(
                "Checkpoint already active. Use rewind to close the current checkpoint first."
                    .to_string(),
            );
        }

        *store = Some(CheckpointState { goal: goal.clone() });

        let output = format!(
            "Checkpoint created.\nGoal: {}\nRun your investigation, then call rewind with a concise report.",
            goal
        );
        Ok(AgentToolResult::success(output))
    }
}

/// RewindTool — close a checkpoint and submit investigation findings.
pub struct RewindTool;

#[async_trait]
impl AgentTool for RewindTool {
    fn name(&self) -> &str {
        "rewind"
    }

    fn label(&self) -> &str {
        "Rewind"
    }

    fn description(&self) -> &str {
        concat!(
            "Close an active investigation checkpoint and submit findings. ",
            "Provide a concise report of what you discovered. ",
            "Must be called after a checkpoint is active."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "report": {
                    "type": "string",
                    "description": "Investigation findings report — concise summary of what was discovered."
                }
            },
            "required": ["report"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Close investigation checkpoint with findings")
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
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let report = params
            .get("report")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Missing required parameter: report".to_string())?;

        let mut store = CHECKPOINT
            .lock()
            .map_err(|e| format!("Checkpoint lock error: {}", e))?;

        let state = store
            .take()
            .ok_or_else(|| "No active checkpoint to rewind.".to_string())?;

        Ok(AgentToolResult::success(format!(
            "Checkpoint closed.\nGoal: {}\nReport: {}\n\nCheckpoint rewound successfully.",
            state.goal, report
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_checkpoint_create_and_rewind() {
        // Reset global state
        *CHECKPOINT.lock().unwrap() = None;

        let tool = CheckpointTool;
        let params = json!({"goal": "Investigate authentication flow"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Checkpoint created"));

        // Second checkpoint should fail
        let params2 = json!({"goal": "Another goal"});
        let result2 = tool
            .execute("id", params2, None, &ToolContext::default())
            .await;
        assert!(result2.is_err());

        // Rewind
        let rewind = RewindTool;
        let params3 = json!({"report": "Found auth token rotation issue"});
        let result3 = rewind
            .execute("id", params3.clone(), None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result3.success);
        assert!(result3.output.contains("Checkpoint closed"));

        // Rewind without checkpoint should fail
        let result4 = rewind
            .execute("id", params3, None, &ToolContext::default())
            .await;
        assert!(result4.is_err());
    }
}
