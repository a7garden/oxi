/// Goal tool — manage investigation goals with token budgets.
///
/// Supports create, get, complete, resume, and drop operations on a single
/// active goal. Goals track an objective, status, and optional token budget.
/// In omp this is a HIDDEN_TOOL — available but not in BUILTIN_TOOLS.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

/// Next goal ID counter.
static NEXT_GOAL_ID: AtomicU64 = AtomicU64::new(1);

/// A goal.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Goal {
    #[allow(dead_code)]
    id: String,
    objective: String,
    status: String,
    token_budget: Option<u64>,
    tokens_used: u64,
    created_at: u64,
    updated_at: u64,
}

/// Global active goal.
static ACTIVE_GOAL: LazyLock<Mutex<Option<Goal>>> = LazyLock::new(|| Mutex::new(None));

/// GoalTool — manage investigation goals.
pub struct GoalTool;

#[async_trait]
impl AgentTool for GoalTool {
    fn name(&self) -> &str {
        "goal"
    }

    fn label(&self) -> &str {
        "Goal"
    }

    fn description(&self) -> &str {
        concat!(
            "Manage investigation goals with token budgets. ",
            "Operations: create (new goal with objective), ",
            "get (current goal), complete (mark done), ",
            "resume (reactivate), drop (abandon)."
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
                    "enum": ["create", "get", "complete", "resume", "drop"],
                    "description": "Goal operation."
                },
                "objective": {
                    "type": "string",
                    "description": "Goal objective (required for create)."
                },
                "token_budget": {
                    "type": "integer",
                    "description": "Optional token budget limit."
                }
            },
            "required": ["op"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Manage investigation goals")
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
        let op = params
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: op".to_string())?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut store = ACTIVE_GOAL
            .lock()
            .map_err(|e| format!("Goal lock error: {}", e))?;

        match op {
            "create" => {
                let objective = params
                    .get("objective")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| "Missing 'objective' parameter for create".to_string())?;

                let token_budget = params.get("token_budget").and_then(|v| v.as_u64());

                if store.is_some() {
                    return Err("A goal is already active. Complete or drop it first.".to_string());
                }

                let id = NEXT_GOAL_ID.fetch_add(1, Ordering::Relaxed);
                let goal = Goal {
                    id: format!("goal-{}", id),
                    objective: objective.clone(),
                    status: "active".to_string(),
                    token_budget,
                    tokens_used: 0,
                    created_at: now,
                    updated_at: now,
                };

                *store = Some(goal.clone());

                let mut lines = vec![
                    format!("Goal created: {}", objective),
                    format!("Status: active"),
                ];
                if let Some(budget) = token_budget {
                    lines.push(format!("Token budget: {}", budget));
                }

                Ok(AgentToolResult::success(lines.join("\n")))
            }
            "get" => match store.as_ref() {
                Some(goal) => {
                    let mut lines = vec![
                        format!("Goal: {}", goal.objective),
                        format!("Status: {}", goal.status),
                        format!("Tokens used: {}", goal.tokens_used),
                    ];
                    if let Some(budget) = goal.token_budget {
                        lines.push(format!("Token budget: {}", budget));
                        let remaining = budget.saturating_sub(goal.tokens_used);
                        lines.push(format!("Remaining: {}", remaining));
                    }
                    Ok(AgentToolResult::success(lines.join("\n")))
                }
                None => Ok(AgentToolResult::success(
                    "No active goal. Use 'create' to set one.",
                )),
            },
            "complete" => match store.as_mut() {
                Some(goal) => {
                    goal.status = "complete".to_string();
                    goal.updated_at = now;
                    let goal_clone = goal.clone();
                    *store = None;

                    Ok(AgentToolResult::success(format!(
                        "Goal completed: {}\nTokens used: {}",
                        goal_clone.objective, goal_clone.tokens_used
                    )))
                }
                None => Err("No active goal to complete.".to_string()),
            },
            "resume" => match store.as_mut() {
                Some(goal)
                    if goal.status == "complete"
                        || goal.status == "paused"
                        || goal.status == "dropped" =>
                {
                    goal.status = "active".to_string();
                    goal.updated_at = now;
                    Ok(AgentToolResult::success(format!(
                        "Goal resumed: {}",
                        goal.objective
                    )))
                }
                Some(goal) => Err(format!(
                    "Goal is currently '{}' and cannot be resumed.",
                    goal.status
                )),
                None => Err("No goal to resume. Use 'create' first.".to_string()),
            },
            "drop" => match store.take() {
                Some(goal) => Ok(AgentToolResult::success(format!(
                    "Goal dropped: {}",
                    goal.objective
                ))),
                None => Err("No active goal to drop.".to_string()),
            },
            _ => Err(format!("Unknown goal operation: {}", op)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_goal_create_get_complete() {
        // Reset
        *ACTIVE_GOAL.lock().unwrap() = None;

        let tool = GoalTool;

        // Create
        let params = json!({"op": "create", "objective": "Refactor auth module"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Goal created"));

        // Get
        let params2 = json!({"op": "get"});
        let result2 = tool
            .execute("id", params2, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result2.success);
        assert!(result2.output.contains("Refactor auth module"));

        // Create another while active should fail
        let params3 = json!({"op": "create", "objective": "Second goal"});
        let result3 = tool
            .execute("id", params3.clone(), None, &ToolContext::default())
            .await;
        assert!(result3.is_err());

        // Complete
        let params4 = json!({"op": "complete"});
        let result4 = tool
            .execute("id", params4.clone(), None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result4.success);
        assert!(result4.output.contains("Goal completed"));

        // Complete again should fail
        let result5 = tool
            .execute("id", params4.clone(), None, &ToolContext::default())
            .await;
        assert!(result5.is_err());
    }

    #[tokio::test]
    async fn test_goal_create_with_budget() {
        *ACTIVE_GOAL.lock().unwrap() = None;

        let tool = GoalTool;
        let params = json!({"op": "create", "objective": "Fix bugs", "token_budget": 5000});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("5000"));
    }
}
