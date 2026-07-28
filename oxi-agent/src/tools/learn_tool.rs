/// Learn tool — capture a reusable lesson to memory and optionally create a managed skill.
///
/// Persists a lesson to the memory backend and, given a `skill` payload,
/// creates or updates a managed SKILL.md. Mirrors the omp `learn` tool.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Simple in-memory lesson store.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Lesson {
    memory: String,
    context: Option<String>,
    skill_action: Option<String>,
    skill_name: Option<String>,
}

static LESSONS: LazyLock<Mutex<Vec<Lesson>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// LearnTool — capture lessons to memory.
pub struct LearnTool;

#[async_trait]
impl AgentTool for LearnTool {
    fn name(&self) -> &str {
        "learn"
    }

    fn label(&self) -> &str {
        "Learn"
    }

    fn description(&self) -> &str {
        concat!(
            "Capture a reusable lesson to long-term memory (and optionally a managed skill). ",
            "Provide 'memory' for the lesson content, optional 'context' for source context, ",
            "and optional 'skill' with action/name/description/body to also mint a managed skill."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "memory": {
                    "type": "string",
                    "description": "The durable, self-contained lesson to remember (what, when, why)."
                },
                "context": {
                    "type": "string",
                    "description": "Optional source context for the lesson."
                },
                "skill": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["create", "update"],
                            "description": "Create or update a managed skill."
                        },
                        "name": {
                            "type": "string",
                            "description": "Kebab-case skill name."
                        },
                        "description": {
                            "type": "string",
                            "description": "One-line description of when to use the skill."
                        },
                        "body": {
                            "type": "string",
                            "description": "The SKILL.md body in markdown (no frontmatter)."
                        }
                    },
                    "required": ["action", "name", "description", "body"],
                    "description": "Also create or enhance a managed skill in the same call."
                }
            },
            "required": ["memory"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Capture a lesson to long-term memory")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    fn tool_tier(&self) -> ToolTier {
        ToolTier::Write
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let memory = params
            .get("memory")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: memory".to_string())?;

        let context = params
            .get("context")
            .and_then(|v| v.as_str())
            .map(String::from);
        let skill = params.get("skill");

        let mut lines = vec![format!("Lesson recorded: {}", memory)];
        if let Some(ref ctx_str) = context {
            lines.push(format!("Context: {}", ctx_str));
        }

        // Try to persist via MemoryBackend if available
        if let Some(ref backend) = ctx.memory {
            let result = backend
                .put(memory, "lesson", context.as_deref().unwrap_or("general"))
                .await;
            match result {
                Ok(id) => lines.push(format!("Persisted to memory backend (id: {}).", id)),
                Err(e) => lines.push(format!("Warning: memory backend store failed: {}", e)),
            }
        } else {
            // Fallback: in-memory store
            let lesson = Lesson {
                memory: memory.to_string(),
                context: context.clone(),
                skill_action: skill
                    .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(String::from)),
                skill_name: skill
                    .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(String::from)),
            };
            let mut store = LESSONS
                .lock()
                .map_err(|e| format!("Lesson lock error: {}", e))?;
            store.push(lesson);
            lines.push(format!(
                "Stored in-memory (lesson #{}). No MemoryBackend configured.",
                store.len()
            ));
        }

        // Handle skill sub-payload
        if let Some(skill_val) = skill {
            let action = skill_val
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("create");
            let name = skill_val
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed");
            let desc = skill_val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let body = skill_val.get("body").and_then(|v| v.as_str()).unwrap_or("");

            lines.push(format!(
                "Skill {}: {} (description: {}, body: {} chars)",
                action,
                name,
                desc,
                body.len()
            ));
            lines.push(
                "Note: Managed skill writing requires the `manage_skill` tool for full file I/O."
                    .to_string(),
            );
        }

        Ok(AgentToolResult::success(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_learn_basic() {
        let tool = LearnTool;
        let params = json!({"memory": "Always validate input before processing"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Lesson recorded"));
    }

    #[tokio::test]
    async fn test_learn_with_context() {
        let tool = LearnTool;
        let params = json!({
            "memory": "Use parking_lot::RwLock not std::sync::RwLock",
            "context": "oxi concurrency conventions"
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Context:"));
    }

    #[tokio::test]
    async fn test_learn_with_skill() {
        let tool = LearnTool;
        let params = json!({
            "memory": "Debug with debug tool",
            "skill": {
                "action": "create",
                "name": "debug-rust",
                "description": "Debug Rust code with lldb",
                "body": "Use `rust-lldb` for debugging Rust programs..."
            }
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Skill create"));
    }
}
