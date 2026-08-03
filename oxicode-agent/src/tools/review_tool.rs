/// Review tool — request code review with focus areas and priorities.
///
/// Submits a structured code review request. Supports specifying files to
/// review, a focus area (security, performance, correctness, style, all),
/// and finding priority level. Outputs a formatted review request that can
/// be used with a subagent reviewer.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// ReviewTool — request code review.
pub struct ReviewTool;

#[async_trait]
impl AgentTool for ReviewTool {
    fn name(&self) -> &str {
        "review"
    }

    fn label(&self) -> &str {
        "Review"
    }

    fn description(&self) -> &str {
        concat!(
            "Request code review with focus areas and finding priorities. ",
            "Specify files to review, the focus area (security, performance, ",
            "correctness, style, or all), and the finding priority level. ",
            "Use the subagent tool to dispatch the actual review."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Files to review (paths relative to workspace root)."
                },
                "context": {
                    "type": "string",
                    "description": "Additional context for the review (e.g., recent changes, known issues)."
                },
                "focus": {
                    "type": "string",
                    "enum": ["security", "performance", "correctness", "style", "all"],
                    "description": "Focus area for the review."
                },
                "priority": {
                    "type": "string",
                    "enum": ["P0", "P1", "P2", "P3"],
                    "description": "Finding priority threshold. P0=critical, P3=informational."
                }
            }
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Request code review")
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
        let files: Vec<String> = params
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let context = params
            .get("context")
            .and_then(|v| v.as_str())
            .map(String::from);

        let focus = params
            .get("focus")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        let priority = params
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("P2");

        let mut lines = vec!["## Code Review Request".to_string()];
        lines.push(format!("- Focus: {}", focus));
        lines.push(format!("- Priority threshold: {}", priority));

        if files.is_empty() {
            lines.push("\nFiles: (no specific files — review recent changes)".to_string());
        } else {
            lines.push(format!("\nFiles ({}):", files.len()));
            for file in &files {
                lines.push(format!("  - {}", file));
            }
        }

        if let Some(ref ctx) = context {
            lines.push(format!("\nContext:\n{}", ctx));
        }

        lines.push("\n## How to proceed".to_string());
        lines.push("To perform the actual review, use the subagent tool:".to_string());
        lines.push(
            "1. Create a reviewer agent with `subagent` tool and agent: 'reviewer'".to_string(),
        );
        lines.push("2. Pass the review files and focus area in the task description".to_string());
        lines.push("3. The reviewer will return findings with P0-P3 priorities".to_string());

        Ok(AgentToolResult::success(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_review_basic() {
        let tool = ReviewTool;
        let params = json!({
            "files": ["src/auth.rs", "src/api.rs"],
            "focus": "security",
            "priority": "P0"
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("security"));
        assert!(result.output.contains("P0"));
        assert!(result.output.contains("src/auth.rs"));
    }

    #[tokio::test]
    async fn test_review_all_focus() {
        let tool = ReviewTool;
        let params = json!({
            "context": "Recent refactor of the database layer",
            "focus": "all"
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Review Request"));
        assert!(result.output.contains("subagent"));
    }
}
