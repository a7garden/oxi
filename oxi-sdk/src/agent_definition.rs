//! Agent definition file parsing and validation.
//!
//! Loads agent definitions from markdown files with YAML frontmatter.
//! Discovery searches ~/.oxi/agents/ and .oxi/agents/ directories.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Agent definition parsed from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Agent name (a-z, 0-9, hyphens, max 64 chars)
    pub name: String,
    /// Human-readable description (max 1024 chars)
    pub description: String,
    /// Optional model override
    #[serde(default)]
    pub model: Option<String>,
    /// Tool names to make available
    #[serde(default)]
    pub tools: Vec<String>,
    /// System prompt (from frontmatter or body)
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Agent scope (where it's discovered)
    #[serde(default)]
    pub scope: AgentScope,
    /// Extensions to load
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Maximum subagent nesting depth (max 10)
    #[serde(default = "default_max_depth")]
    pub max_subagent_depth: u8,
    /// Default context mode
    #[serde(default)]
    pub default_context: DefaultContext,
}

fn default_max_depth() -> u8 {
    3
}

/// Agent visibility scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentScope {
    #[default]
    User,
    Project,
    Both,
}

/// Default context for agent sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultContext {
    #[default]
    Fresh,
    Fork,
}

impl AgentDefinition {
    /// Load an agent definition from a markdown file.
    pub fn from_markdown(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let (frontmatter, body) = extract_frontmatter(&content);

        let mut def: AgentDefinition = if frontmatter.is_empty() {
            // No frontmatter — use filename as name
            let name = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            AgentDefinition {
                name,
                description: String::new(),
                model: None,
                tools: vec![],
                system_prompt: None,
                scope: AgentScope::default(),
                extensions: vec![],
                max_subagent_depth: 3,
                default_context: DefaultContext::default(),
            }
        } else {
            serde_yaml::from_str(&frontmatter)
                .with_context(|| format!("Failed to parse YAML frontmatter in {}", path.display()))?
        };

        // Use body as system_prompt if not set in frontmatter
        if !body.is_empty() && def.system_prompt.is_none() {
            def.system_prompt = Some(body);
        }

        // If description is still empty, use the first line of the body
        if def.description.is_empty() {
            if let Some(first_line) = def.system_prompt.as_ref().and_then(|s| s.lines().next()) {
                def.description = first_line.trim_start_matches('#').trim().to_string();
            }
        }

        def.validate()?;
        Ok(def)
    }

    /// Validate the agent definition.
    fn validate(&self) -> Result<()> {
        validate_agent_name(&self.name)?;

        if self.description.len() > 1024 {
            anyhow::bail!(
                "Description too long ({} chars, max 1024)",
                self.description.len()
            );
        }

        if self.max_subagent_depth > 10 {
            anyhow::bail!(
                "max_subagent_depth too high ({} > 10)",
                self.max_subagent_depth
            );
        }

        Ok(())
    }
}

/// Validate an agent name.
pub fn validate_agent_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Agent name must not be empty");
    }
    if name.len() > 64 {
        anyhow::bail!("Agent name too long ({} > 64)", name.len());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        anyhow::bail!(
            "Agent name must contain only a-z, 0-9, and hyphens: got '{}'",
            name
        );
    }
    Ok(())
}

/// Extract YAML frontmatter and body from markdown content.
fn extract_frontmatter(content: &str) -> (String, String) {
    if !content.starts_with("---") {
        return (String::new(), content.to_string());
    }

    let rest = &content[3..];
    if let Some(end) = rest.find("\n---") {
        let yaml_str = rest[..end].to_string();
        let body = rest[end + 4..].trim().to_string();
        (yaml_str, body)
    } else {
        (String::new(), content.to_string())
    }
}

/// Agent discovery from filesystem directories.
pub struct AgentDiscovery;

impl AgentDiscovery {
    /// Discover agent definitions from global and project directories.
    ///
    /// Search order (later overrides earlier):
    /// 1. Global: ~/.oxi/agents/
    /// 2. Project: .oxi/agents/ (relative to `cwd`)
    pub fn discover(cwd: &Path) -> Result<Vec<(String, AgentDefinition)>> {
        let mut agents = HashMap::new();

        // 1. Global: ~/.oxi/agents/
        if let Some(home) = dirs::home_dir() {
            let global_dir = home.join(".oxi/agents");
            Self::discover_from_dir(&global_dir, &mut agents)?;
        }

        // 2. Project: .oxi/agents/
        let project_dir = cwd.join(".oxi/agents");
        Self::discover_from_dir(&project_dir, &mut agents)?;

        Ok(agents.into_iter().collect())
    }

    fn discover_from_dir(
        dir: &Path,
        agents: &mut HashMap<String, AgentDefinition>,
    ) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Look for agent.md files in subdirectories
            if path.is_dir() {
                let agent_file = path.join("agent.md");
                if agent_file.exists() {
                    let dir_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();

                    match AgentDefinition::from_markdown(&agent_file) {
                        Ok(def) => {
                            agents.insert(dir_name.to_lowercase(), def);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to load agent from {}: {}",
                                agent_file.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_validate_agent_name_valid() {
        assert!(validate_agent_name("my-agent").is_ok());
        assert!(validate_agent_name("agent123").is_ok());
        assert!(validate_agent_name("a").is_ok());
    }

    #[test]
    fn test_validate_agent_name_invalid() {
        assert!(validate_agent_name("").is_err());
        assert!(validate_agent_name("Agent").is_err()); // uppercase
        assert!(validate_agent_name("my_agent").is_err()); // underscore
        assert!(validate_agent_name(&"a".repeat(65)).is_err()); // too long
    }

    #[test]
    fn test_extract_frontmatter() {
        let content = "---\nname: test-agent\ndescription: A test\n---\nBody content";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.contains("test-agent"));
        assert!(body.starts_with("Body content"));
    }

    #[test]
    fn test_extract_frontmatter_none() {
        let content = "# No frontmatter\nJust content";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_empty());
        assert!(body.contains("No frontmatter"));
    }

    #[test]
    fn test_from_markdown_with_frontmatter() {
        let dir = TempDir::new().unwrap();
        let agent_dir = dir.path().join("test-agent");
        fs::create_dir_all(&agent_dir).unwrap();
        let agent_file = agent_dir.join("agent.md");
        let mut f = fs::File::create(&agent_file).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: test-agent").unwrap();
        writeln!(f, "description: A test agent").unwrap();
        writeln!(f, "model: gpt-4o").unwrap();
        writeln!(f, "tools:").unwrap();
        writeln!(f, "  - read").unwrap();
        writeln!(f, "  - bash").unwrap();
        writeln!(f, "max_subagent_depth: 5").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "You are a test agent.").unwrap();

        let def = AgentDefinition::from_markdown(&agent_file).unwrap();
        assert_eq!(def.name, "test-agent");
        assert_eq!(def.description, "A test agent");
        assert_eq!(def.model, Some("gpt-4o".to_string()));
        assert_eq!(def.tools, vec!["read", "bash"]);
        assert_eq!(def.max_subagent_depth, 5);
        assert_eq!(
            def.system_prompt,
            Some("You are a test agent.".to_string())
        );
    }

    #[test]
    fn test_from_markdown_validation_fails() {
        let dir = TempDir::new().unwrap();
        let agent_dir = dir.path().join("bad-agent");
        fs::create_dir_all(&agent_dir).unwrap();
        let agent_file = agent_dir.join("agent.md");
        let mut f = fs::File::create(&agent_file).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: BAD_NAME").unwrap(); // uppercase
        writeln!(f, "description: Invalid").unwrap();
        writeln!(f, "---").unwrap();

        let result = AgentDefinition::from_markdown(&agent_file);
        assert!(result.is_err());
    }

    #[test]
    fn test_discover() {
        let dir = TempDir::new().unwrap();

        // Create .oxi/agents/my-worker/agent.md under the temp dir
        let agents_dir = dir.path().join(".oxi/agents");
        let agent_dir = agents_dir.join("my-worker");
        fs::create_dir_all(&agent_dir).unwrap();
        let agent_file = agent_dir.join("agent.md");
        let mut f = fs::File::create(&agent_file).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: my-worker").unwrap();
        writeln!(f, "description: Worker agent").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "You are a worker.").unwrap();

        let agents = AgentDiscovery::discover(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        let (name, def) = &agents[0];
        assert_eq!(name, "my-worker");
        assert_eq!(def.name, "my-worker");
    }
}
