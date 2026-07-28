/// Manage Skill tool — create, update, or delete isolated managed SKILL.md files.
///
/// Writes to `~/.omp/agent/managed-skills/<name>/SKILL.md`. Mirrors the omp
/// `manage_skill` tool. Requires filesystem access to the managed skills directory.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::oneshot;

/// Default managed skills directory.
fn managed_skills_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".omp/agent/managed-skills")
}

/// Validate a kebab-case skill name.
fn validate_skill_name(name: &str) -> Result<String, ToolError> {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    if sanitized.is_empty() || sanitized != name {
        return Err(format!(
            "Invalid skill name '{}'. Use kebab-case (lowercase letters, digits, hyphens).",
            name
        ));
    }
    Ok(sanitized)
}

/// ManageSkillTool — create/update/delete managed skills.
pub struct ManageSkillTool;

#[async_trait]
impl AgentTool for ManageSkillTool {
    fn name(&self) -> &str {
        "manage_skill"
    }

    fn label(&self) -> &str {
        "Manage Skill"
    }

    fn description(&self) -> &str {
        concat!(
            "Create, update, or delete an isolated managed skill. ",
            "Managed skills are SKILL.md files in ~/.omp/agent/managed-skills/ ",
            "that are surfaced to future sessions like normal skills. ",
            "Actions: create (requires name, description, body), ",
            "update (requires name, description, body), delete (requires name)."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update", "delete"],
                    "description": "Action to perform on the managed skill."
                },
                "name": {
                    "type": "string",
                    "description": "Kebab-case skill name (lowercase letters, digits, hyphens)."
                },
                "description": {
                    "type": "string",
                    "description": "One-line description of when to use the skill (required for create/update)."
                },
                "body": {
                    "type": "string",
                    "description": "The SKILL.md body in markdown, no frontmatter (required for create/update)."
                }
            },
            "required": ["action", "name"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Manage a SKILL.md file")
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
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?;

        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: name".to_string())?;
        let name = validate_skill_name(name)?;

        match action {
            "create" | "update" => {
                let description = params
                    .get("description")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'description' for create/update".to_string())?;
                let body = params
                    .get("body")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing 'body' for create/update".to_string())?;

                let skill_dir = managed_skills_dir().join(&name);
                fs::create_dir_all(&skill_dir)
                    .await
                    .map_err(|e| format!("Failed to create skill directory: {}", e))?;

                let skill_path = skill_dir.join("SKILL.md");
                let content = format!(
                    "---\nname: {}\ndescription: {}\n---\n\n{}",
                    name, description, body
                );
                fs::write(&skill_path, &content)
                    .await
                    .map_err(|e| format!("Failed to write SKILL.md: {}", e))?;

                Ok(AgentToolResult::success(format!(
                    "Skill '{}' {}d.\nDescription: {}\nPath: {}",
                    name,
                    action,
                    description,
                    skill_path.display()
                )))
            }
            "delete" => {
                let skill_dir = managed_skills_dir().join(&name);
                let skill_path = skill_dir.join("SKILL.md");

                if !skill_path.exists() {
                    return Err(format!(
                        "Skill '{}' not found at {}",
                        name,
                        skill_path.display()
                    ));
                }

                fs::remove_file(&skill_path)
                    .await
                    .map_err(|e| format!("Failed to delete SKILL.md: {}", e))?;

                // Try to remove the directory (may fail if not empty)
                let _ = fs::remove_dir(&skill_dir).await;

                Ok(AgentToolResult::success(format!(
                    "Skill '{}' deleted.",
                    name
                )))
            }
            _ => Err(format!("Unknown action: {}", action)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_validate_skill_name() {
        assert!(validate_skill_name("debug-rust").is_ok());
        assert!(validate_skill_name("my-skill-123").is_ok());
        assert!(validate_skill_name("Bad Name").is_err());
        assert!(validate_skill_name("UPPERCASE").is_err());
    }
}
