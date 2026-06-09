//! Agent definition file parsing and validation.
//!
//! Loads agent definitions from markdown files with YAML frontmatter.
//! Discovery searches `~/.oxi/agents/` (user) and `.oxi/agents/` (project).
//!
//! Supports two directory layouts (subdirectory takes priority on collision):
//! - Flat file: `~/.oxi/agents/scout.md`
//! - Subdirectory: `~/.oxi/agents/scout/agent.md`

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Agent definition parsed from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Agent name (a-z, 0-9, hyphens, max 64 chars)
    pub name: String,
    /// Human-readable description (max 1024 chars)
    #[serde(default)]
    pub description: String,
    /// Optional model override
    #[serde(default)]
    pub model: Option<String>,
    /// Tool names to make available. Accepts both YAML array and comma-separated string.
    #[serde(default, deserialize_with = "deserialize_tools")]
    pub tools: Vec<String>,
    /// System prompt (from frontmatter or body)
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Discovery scope: "user" or "project". Set by discovery, not by the file.
    #[serde(default)]
    pub source: String,
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

/// Agent visibility scope for discovery queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentScope {
    /// Only user-level agents (~/.oxi/agents/)
    #[default]
    User,
    /// Only project-level agents (.oxi/agents/)
    Project,
    /// Both user and project agents
    Both,
}

/// Default context for agent sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultContext {
    #[default]
    /// Start with an empty context.
    Fresh,
    /// Branch from the parent session's context.
    Fork,
}

impl AgentDefinition {
    /// Load an agent definition from a markdown file.
    pub fn from_markdown(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let (frontmatter, body) = extract_frontmatter(&content);

        let mut def: AgentDefinition = if frontmatter.is_empty() {
            // No frontmatter — use filename stem as name
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            AgentDefinition {
                name,
                description: String::new(),
                model: None,
                tools: vec![],
                system_prompt: None,
                source: String::new(),
                extensions: vec![],
                max_subagent_depth: 3,
                default_context: DefaultContext::default(),
            }
        } else {
            serde_yaml::from_str(&frontmatter).with_context(|| {
                format!("Failed to parse YAML frontmatter in {}", path.display())
            })?
        };

        // Use body as system_prompt if not set in frontmatter
        if !body.is_empty() && def.system_prompt.is_none() {
            def.system_prompt = Some(body);
        }

        // If description is still empty, use the first line of the body
        if def.description.is_empty()
            && let Some(first_line) = def.system_prompt.as_ref().and_then(|s| s.lines().next())
        {
            def.description = first_line.trim_start_matches('#').trim().to_string();
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

use serde::de::Deserializer;

/// Custom deserializer for the `tools` field.
/// Accepts either a YAML array (`["read", "bash"]`) or a comma-separated string (`"read, bash"`).
fn deserialize_tools<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde_yaml::Value;
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Sequence(seq) => Ok(seq
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()),
        Value::String(s) => Ok(s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()),
        _ => Ok(vec![]),
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
    let Some(rest) = content.strip_prefix("---") else {
        return (String::new(), content.to_string());
    };

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
    /// Search order (project overrides user on collision):
    /// 1. Global: ~/.oxi/agents/
    /// 2. Project: .oxi/agents/ (walks up to .git boundary)
    ///
    /// Within each directory, subdirectory format (`<name>/agent.md`) takes
    /// priority over flat files (`<name>.md`) on name collision.
    pub fn discover(cwd: &Path, scope: AgentScope) -> Result<Vec<(String, AgentDefinition)>> {
        let mut agents = HashMap::new();

        // 1. Global: ~/.oxi/agents/
        if (scope == AgentScope::User || scope == AgentScope::Both)
            && let Some(home) = dirs::home_dir()
        {
            let global_dir = home.join(".oxi/agents");
            Self::discover_from_dir(&global_dir, "user", &mut agents)?;
        }

        // 2. Project: walk up to find .oxi/agents/
        if (scope == AgentScope::Project || scope == AgentScope::Both)
            && let Some(project_dir) = find_project_agents_dir(cwd)
        {
            Self::discover_from_dir(&project_dir, "project", &mut agents)?;
        }

        Ok(agents.into_iter().collect())
    }

    /// Discover agents from a single directory.
    /// Supports both subdirectory format (`<name>/agent.md`) and flat files (`<name>.md`).
    /// Subdirectory entries are loaded first so they take priority.
    fn discover_from_dir(
        dir: &Path,
        source: &str,
        agents: &mut HashMap<String, AgentDefinition>,
    ) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        // First pass: subdirectories (higher priority)
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let agent_file = path.join("agent.md");
                if agent_file.exists() {
                    let dir_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    match AgentDefinition::from_markdown(&agent_file) {
                        Ok(mut def) => {
                            def.source = source.to_string();
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

        // Second pass: flat .md files (lower priority — or_insert skips collisions)
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                match AgentDefinition::from_markdown(&path) {
                    Ok(mut def) => {
                        def.source = source.to_string();
                        agents.entry(name.to_lowercase()).or_insert(def);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load agent {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(())
    }
}

/// Walk up from `cwd` to find `.oxi/agents/`.
/// Stops at `.git` boundary (project root). Returns None if not found.
fn find_project_agents_dir(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let candidate = current.join(".oxi").join("agents");
        if candidate.is_dir() {
            return Some(candidate);
        }
        // .git marks project root — don't go higher
        if current.join(".git").exists() {
            return None;
        }
        current = current.parent()?;
    }
}

// ── Depth tracking ─────────────────────────────────────────────────────

/// Get the current subagent nesting depth from the environment.
/// Default is 0 (top-level process).
pub fn current_subagent_depth() -> u8 {
    std::env::var("OXI_SUBAGENT_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Get the maximum allowed subagent depth from the environment.
/// Default is 3.
pub fn max_subagent_depth() -> u8 {
    std::env::var("OXI_MAX_SUBAGENT_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
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
        let agent_file = dir.path().join("test-agent.md");
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
        assert_eq!(def.system_prompt, Some("You are a test agent.".to_string()));
    }

    #[test]
    fn test_from_markdown_flat_tools() {
        let dir = TempDir::new().unwrap();
        let agent_file = dir.path().join("scout.md");
        let mut f = fs::File::create(&agent_file).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: scout").unwrap();
        writeln!(f, "tools: read, grep, find").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "You are a scout.").unwrap();

        let def = AgentDefinition::from_markdown(&agent_file).unwrap();
        assert_eq!(def.tools, vec!["read", "grep", "find"]);
    }

    #[test]
    fn test_from_markdown_validation_fails() {
        let dir = TempDir::new().unwrap();
        let agent_file = dir.path().join("bad.md");
        let mut f = fs::File::create(&agent_file).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: BAD_NAME").unwrap(); // uppercase
        writeln!(f, "description: Invalid").unwrap();
        writeln!(f, "---").unwrap();

        let result = AgentDefinition::from_markdown(&agent_file);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_markdown_no_frontmatter() {
        let dir = TempDir::new().unwrap();
        let agent_file = dir.path().join("worker.md");
        fs::write(&agent_file, "You are a worker agent.").unwrap();

        let def = AgentDefinition::from_markdown(&agent_file).unwrap();
        assert_eq!(def.name, "worker");
        assert_eq!(
            def.system_prompt,
            Some("You are a worker agent.".to_string())
        );
    }

    #[test]
    fn test_discover_subdirectory() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".oxi").join("agents");
        let agent_dir = agents_dir.join("my-worker");
        fs::create_dir_all(&agent_dir).unwrap();
        let agent_file = agent_dir.join("agent.md");
        let mut f = fs::File::create(&agent_file).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: my-worker").unwrap();
        writeln!(f, "description: Worker agent").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "You are a worker.").unwrap();

        let agents = AgentDiscovery::discover(dir.path(), AgentScope::Project).unwrap();
        assert_eq!(agents.len(), 1);
        let (name, def) = &agents[0];
        assert_eq!(name, "my-worker");
        assert_eq!(def.name, "my-worker");
        assert_eq!(def.source, "project");
    }

    #[test]
    fn test_discover_flat_md() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".oxi").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(
            agents_dir.join("scout.md"),
            "---\nname: scout\ndescription: Recon\n---\nBe a scout.",
        )
        .unwrap();

        let agents = AgentDiscovery::discover(dir.path(), AgentScope::Project).unwrap();
        assert_eq!(agents.len(), 1);
        let (name, _) = &agents[0];
        assert_eq!(name, "scout");
    }

    #[test]
    fn test_discover_subdir_takes_priority() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".oxi").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        // Flat file
        fs::write(
            agents_dir.join("scout.md"),
            "---\nname: scout\ndescription: Flat\n---\nFlat scout.",
        )
        .unwrap();

        // Subdirectory (should win)
        let subdir = agents_dir.join("scout");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(
            subdir.join("agent.md"),
            "---\nname: scout\ndescription: Subdir\n---\nSubdir scout.",
        )
        .unwrap();

        let agents = AgentDiscovery::discover(dir.path(), AgentScope::Project).unwrap();
        assert_eq!(agents.len(), 1);
        let (_, def) = &agents[0];
        assert_eq!(def.description, "Subdir");
    }

    #[test]
    fn test_discover_scope_filtering() {
        let dir = TempDir::new().unwrap();

        // Create .git boundary so find_project_agents_dir stops
        fs::create_dir_all(dir.path().join(".git")).unwrap();

        // Project agent (under cwd/.oxi/agents)
        let agents_dir = dir.path().join(".oxi").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(
            agents_dir.join("project-agent.md"),
            "---\nname: project-agent\n---\nProject.",
        )
        .unwrap();

        // Project scope should find project agents
        let agents = AgentDiscovery::discover(dir.path(), AgentScope::Project).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].1.source, "project");
    }

    #[test]
    fn test_find_project_agents_dir() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".oxi").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_project_agents_dir(&sub), Some(agents_dir));
    }

    #[test]
    fn test_find_project_agents_dir_stops_at_git() {
        let dir = TempDir::new().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        assert_eq!(find_project_agents_dir(dir.path()), None);
    }

    #[test]
    fn test_depth_functions_default() {
        // Clear env vars to test defaults
        unsafe {
            std::env::remove_var("OXI_SUBAGENT_DEPTH");
            std::env::remove_var("OXI_MAX_SUBAGENT_DEPTH");
        }
        assert_eq!(current_subagent_depth(), 0);
        assert_eq!(max_subagent_depth(), 3);
    }
}
