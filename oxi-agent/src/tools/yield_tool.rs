/// Yield tool — subagent result submission.
///
/// Subagents call yield to submit their output with type labels and optional
/// structured data. Supports final results, incremental findings, and progress
/// updates. In omp this is a HIDDEN_TOOL — available but not listed in the
/// BUILTIN_TOOLS map.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// A yielded result.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct YieldedResult {
    /// Submission type: "result" (final), "findings" (incremental), "progress".
    result_type: String,
    /// Structured output data.
    data: Option<Value>,
    /// Error message (if failed).
    error: Option<String>,
    /// Summary text.
    summary: Option<String>,
    /// Timestamp.
    timestamp: u64,
}

/// Global yield store.
static YIELD_STORE: LazyLock<Mutex<Vec<YieldedResult>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// YieldTool — submit subagent output.
pub struct YieldTool;

#[async_trait]
impl AgentTool for YieldTool {
    fn name(&self) -> &str {
        "yield"
    }

    fn label(&self) -> &str {
        "Yield"
    }

    fn description(&self) -> &str {
        concat!(
            "Submit subagent output with type labels and optional structured data. ",
            "Use 'result' for final output, 'findings' for incremental code review ",
            "findings, or 'progress' for interim updates. ",
            "Provide 'data' for structured results or 'error' for failures."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["result", "findings", "progress"],
                    "description": "Submission type: 'result' (final), 'findings' (incremental), 'progress' (interim)."
                },
                "data": {
                    "type": "object",
                    "description": "Structured output data (arbitrary JSON)."
                },
                "error": {
                    "type": "string",
                    "description": "Error message if the subagent failed."
                },
                "summary": {
                    "type": "string",
                    "description": "One-line summary of the result."
                }
            }
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Submit subagent output")
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
        let result_type = params
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("result")
            .to_string();

        let data = params.get("data").cloned();
        let error = params
            .get("error")
            .and_then(|v| v.as_str())
            .map(String::from);
        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .map(String::from);

        let yielded = YieldedResult {
            result_type: result_type.clone(),
            data,
            error: error.clone(),
            summary: summary.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };

        let mut store = YIELD_STORE
            .lock()
            .map_err(|e| format!("Yield store lock error: {}", e))?;
        store.push(yielded);

        let mut lines = vec![format!("Yielded result (type: {})", result_type)];
        if let Some(ref s) = summary {
            lines.push(format!("Summary: {}", s));
        }
        if let Some(ref e) = error {
            lines.push(format!("Error: {}", e));
        }

        Ok(AgentToolResult::success(format!(
            "{}\nResult stored {} entry.",
            lines.join("\n"),
            store.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_yield_result() {
        // Reset
        *YIELD_STORE.lock().unwrap() = Vec::new();

        let tool = YieldTool;
        let params = json!({
            "type": "result",
            "data": {"output": "completed analysis"},
            "summary": "Analysis complete"
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Yielded result"));
    }

    #[tokio::test]
    async fn test_yield_findings() {
        let tool = YieldTool;
        let params = json!({
            "type": "findings",
            "data": {
                "findings": [
                    {"title": "Security issue", "priority": "P0", "file": "src/auth.rs"}
                ]
            }
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_yield_error() {
        let tool = YieldTool;
        let params = json!({
            "type": "result",
            "error": "Analysis failed: timeout",
            "summary": "Failed to complete"
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Error:"));
    }
}
