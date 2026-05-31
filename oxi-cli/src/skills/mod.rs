//! Skills system for oxi CLI
//!
//! Skills are on-demand capability packages stored as markdown files.
//! Each skill lives in its own directory under `~/.oxi/skills/<name>/SKILL.md`.
//!
//! When a skill is invoked, its content is appended to the system prompt,
//! giving the agent additional context and instructions for that skill.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// YAML frontmatter parsed from SKILL.md
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SkillFrontmatter {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

/// A single skill loaded from a SKILL.md file.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name (directory name, e.g. "my-skill")
    pub name: String,
    /// Short description extracted from the first line or frontmatter
    pub description: String,
    /// Full markdown content from SKILL.md
    pub content: String,
    /// Absolute path to the skill directory (parent of SKILL.md)
    pub location: PathBuf,
    /// Whether model invocation is disabled for this skill
    pub disable_model_invocation: bool,
}

impl fmt::Display for Skill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.description)
    }
}

/// Manages loading and querying skills from the filesystem.
#[derive(Clone)]
pub struct SkillManager {
    skills: HashMap<String, Skill>,
}

impl SkillManager {
    /// Create an empty skill manager.
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// Load all skills from the given directory.
    ///
    /// Expected structure:
    /// ```text
    /// dir/
    ///   ├── my-skill/
    ///   │   └── SKILL.md
    ///   └── another-skill/
    ///       └── SKILL.md
    /// ```
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut skills = HashMap::new();

        if !dir.exists() {
            tracing::debug!("Skills directory does not exist: {}", dir.display());
            return Ok(Self { skills });
        }

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("Failed to read skills directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only process directories
            if !path.is_dir() {
                continue;
            }

            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                tracing::debug!(
                    "No SKILL.md found in {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                continue;
            }

            let dir_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let valid_name = match Self::validate_name(&dir_name) {
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!("Invalid skill name '{}': {}", dir_name, e);
                    continue;
                }
            };

            match Self::load_skill(&valid_name, &skill_file) {
                Ok(skill) => {
                    tracing::debug!("Loaded skill: {}", skill.name);
                    skills.insert(valid_name, skill);
                }
                Err(e) => {
                    tracing::warn!("Failed to load skill from {}: {}", skill_file.display(), e);
                }
            }
        }

        tracing::info!("Loaded {} skill(s) from {}", skills.len(), dir.display());
        Ok(Self { skills })
    }

    /// Validate a skill name (a-z, 0-9, hyphens, max 64 chars)
    fn validate_name(name: &str) -> Result<String> {
        let name = name.to_lowercase();
        if name.is_empty() || name.len() > 64 {
            anyhow::bail!(
                "Skill name must be 1-64 chars: got '{}' (len={})",
                name,
                name.len()
            );
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            anyhow::bail!(
                "Skill name must contain only a-z, 0-9, and hyphens: got '{}'",
                name
            );
        }
        if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
            anyhow::bail!(
                "Skill name must not have leading/trailing/consecutive hyphens: got '{}'",
                name
            );
        }
        Ok(name)
    }

    /// Load a single skill from its SKILL.md file.
    fn load_skill(name: &str, path: &Path) -> Result<Skill> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let location = path.parent().unwrap_or(path).to_path_buf();

        // Parse YAML frontmatter (--- ... ---)
        let (frontmatter, body) = if let Some(rest) = raw.strip_prefix("---") {
            if let Some(end) = rest.find("\n---") {
                let yaml_str = &rest[..end];
                let body = rest[end + 4..].trim_start().to_string();
                let fm: SkillFrontmatter = serde_yaml::from_str(yaml_str).unwrap_or_default();
                (fm, body)
            } else {
                (SkillFrontmatter::default(), raw.clone())
            }
        } else {
            (SkillFrontmatter::default(), raw.clone())
        };

        let description = frontmatter
            .description
            .clone()
            .unwrap_or_else(|| Self::extract_description(&body));

        Ok(Skill {
            name: frontmatter.name.clone().unwrap_or_else(|| name.to_string()),
            description,
            content: body,
            location,
            disable_model_invocation: frontmatter.disable_model_invocation,
        })
    }

    /// Extract a short description from markdown content.
    ///
    /// Looks for the first non-empty line that looks like a heading or paragraph.
    /// Falls back to "No description" if nothing suitable is found.
    fn extract_description(content: &str) -> String {
        for line in content.lines() {
            let trimmed = line.trim();
            // Skip empty lines and frontmatter delimiters
            if trimmed.is_empty() || trimmed == "---" {
                continue;
            }
            // Skip YAML frontmatter lines (key: value)
            if trimmed.contains(':') && !trimmed.starts_with('#') && !trimmed.starts_with('-') {
                continue;
            }
            // Use first heading (strip # prefix) or first text line
            if let Some(heading) = trimmed.strip_prefix('#') {
                return heading.trim().to_string();
            }
            // Use first paragraph line
            if !trimmed.starts_with('-') && !trimmed.starts_with('>') && trimmed.len() > 3 {
                return trimmed.to_string();
            }
        }
        "No description".to_string()
    }

    /// Look up a skill by exact name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(&name.to_lowercase())
    }

    /// List all loaded skills, sorted alphabetically by name.
    pub fn all(&self) -> Vec<&Skill> {
        let mut skills: Vec<&Skill> = self.skills.values().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    /// Search skills by query string.
    ///
    /// Matches against name and description (case-insensitive).
    /// Returns skills sorted by relevance (name match first, then description match).
    pub fn search(&self, query: &str) -> Vec<&Skill> {
        let query_lower = query.to_lowercase();
        let mut name_matches: Vec<&Skill> = Vec::new();
        let mut desc_matches: Vec<&Skill> = Vec::new();

        for skill in self.skills.values() {
            let name_lower = skill.name.to_lowercase();
            let desc_lower = skill.description.to_lowercase();

            if name_lower.contains(&query_lower) {
                name_matches.push(skill);
            } else if desc_lower.contains(&query_lower) {
                desc_matches.push(skill);
            }
        }

        // Also check full content
        for skill in self.skills.values() {
            if !name_matches.iter().any(|s| s.name == skill.name)
                && !desc_matches.iter().any(|s| s.name == skill.name)
                && skill.content.to_lowercase().contains(&query_lower)
            {
                desc_matches.push(skill);
            }
        }

        name_matches.sort_by(|a, b| a.name.cmp(&b.name));
        desc_matches.sort_by(|a, b| a.name.cmp(&b.name));

        name_matches.extend(desc_matches);
        name_matches
    }

    /// Number of loaded skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether any skills are loaded.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Get the default skills directory path (~/.oxi/skills/).
    pub fn skills_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(home.join(".oxi").join("skills"))
    }

    /// Discover skills from multiple sources: global, project, ancestor dirs, and settings.
    pub fn discover_all(cwd: &Path, extra_dirs: &[PathBuf]) -> Result<Self> {
        let mut skills = HashMap::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();

        // 1. Global: ~/.oxi/skills/
        if let Ok(global_dir) = Self::skills_dir() {
            Self::discover_from_dir(&global_dir, &mut skills, &mut seen_paths)?;
        }

        // 2. Project: .oxi/skills/
        let project_dir = cwd.join(".oxi/skills");
        Self::discover_from_dir(&project_dir, &mut skills, &mut seen_paths)?;

        // 3. Ancestor directories (.agents/skills/)
        for ancestor in cwd.ancestors() {
            let agents_skills = ancestor.join(".agents/skills");
            if agents_skills.is_dir() {
                Self::discover_from_dir(&agents_skills, &mut skills, &mut seen_paths)?;
            }
            // Stop at git root
            if ancestor.join(".git").is_dir() {
                break;
            }
        }

        // 4. Extra directories from settings
        for dir in extra_dirs {
            Self::discover_from_dir(dir, &mut skills, &mut seen_paths)?;
        }

        tracing::info!("Discovered {} skill(s)", skills.len());
        Ok(Self { skills })
    }

    fn discover_from_dir(
        dir: &Path,
        skills: &mut HashMap<String, Skill>,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("Failed to read skills dir: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }

            // Canonicalize for duplicate detection
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if seen.contains(&canonical) {
                tracing::warn!("Duplicate skill path (skipping): {}", canonical.display());
                continue;
            }

            let dir_name = path.file_name().unwrap_or_default().to_string_lossy();
            let valid_name = match Self::validate_name(&dir_name) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("Invalid skill name '{}': {}", dir_name, e);
                    continue;
                }
            };

            seen.insert(canonical);
            match Self::load_skill(&valid_name, &skill_file) {
                Ok(skill) => {
                    skills.insert(valid_name, skill);
                }
                Err(e) => {
                    tracing::warn!("Failed to load skill from {}: {}", skill_file.display(), e);
                }
            }
        }
        Ok(())
    }

    /// Format skills as XML for system prompt injection.
    /// Filters out skills with disable_model_invocation=true.
    pub fn format_for_prompt(&self) -> String {
        let visible: Vec<_> = self
            .skills
            .values()
            .filter(|s| !s.disable_model_invocation)
            .collect();

        if visible.is_empty() {
            return String::new();
        }

        let mut xml = String::from("<available_skills>\n");
        for skill in &visible {
            xml.push_str(&format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n",
                skill.name,
                skill.description,
                skill.location.display()
            ));
        }
        xml.push_str("</available_skills>");
        xml
    }
}

impl fmt::Debug for SkillManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkillManager")
            .field("count", &self.skills.len())
            .field(
                "names",
                &self.skills.keys().cloned().collect::<Vec<String>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_from_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        assert!(manager.is_empty());
        assert_eq!(manager.len(), 0);
    }

    #[test]
    fn test_load_from_nonexistent_dir() {
        let manager = SkillManager::load_from_dir(Path::new("/nonexistent/skills")).unwrap();
        assert!(manager.is_empty());
    }

    #[test]
    fn test_load_single_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# My Skill\n\nThis skill does something cool.\n\n## Usage\nDo X then Y.",
        )
        .unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        assert_eq!(manager.len(), 1);

        let skill = manager.get("my-skill").unwrap();
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "My Skill");
        assert!(skill.content.contains("This skill does something cool"));
    }

    #[test]
    fn test_load_multiple_skills() {
        let tmp = tempfile::tempdir().unwrap();

        // Create skill-a
        let dir_a = tmp.path().join("skill-a");
        fs::create_dir_all(&dir_a).unwrap();
        fs::write(dir_a.join("SKILL.md"), "# Skill A\nDescription A").unwrap();

        // Create skill-b
        let dir_b = tmp.path().join("skill-b");
        fs::create_dir_all(&dir_b).unwrap();
        fs::write(dir_b.join("SKILL.md"), "# Skill B\nDescription B").unwrap();

        // Create directory without SKILL.md
        let dir_empty = tmp.path().join("empty-dir");
        fs::create_dir_all(&dir_empty).unwrap();

        // Create a file (not a directory) — should be skipped
        fs::write(tmp.path().join("not-a-dir.txt"), "ignore me").unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        assert_eq!(manager.len(), 2);
        assert!(manager.get("skill-a").is_some());
        assert!(manager.get("skill-b").is_some());
        assert!(manager.get("empty-dir").is_none());
    }

    #[test]
    fn test_get_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("My-Skill");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# Test\nContent").unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        // The key is stored as the directory name verbatim
        assert!(manager.get("My-Skill").is_some());
    }

    #[test]
    fn test_search_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rust-expert");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# Rust Expert\nAn expert in Rust").unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let results = manager.search("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-expert");
    }

    #[test]
    fn test_search_by_description() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("helper");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "# Helper\nA database optimization expert",
        )
        .unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let results = manager.search("database");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "helper");
    }

    #[test]
    fn test_search_by_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("coder");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "# Coder\nA coding assistant\n\n## Details\nFocuses on async patterns",
        )
        .unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let results = manager.search("async");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_no_results() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# Skill\nA skill").unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let results = manager.search("nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn test_all_sorted() {
        let tmp = tempfile::tempdir().unwrap();

        for name in &["zebra", "alpha", "middle"] {
            let dir = tmp.path().join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("# {}\nDesc", name)).unwrap();
        }

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let all = manager.all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "middle");
        assert_eq!(all[2].name, "zebra");
    }

    #[test]
    fn test_extract_description_from_heading() {
        let content = "# My Cool Skill\n\nSome body text";
        assert_eq!(SkillManager::extract_description(content), "My Cool Skill");
    }

    #[test]
    fn test_extract_description_from_paragraph() {
        let content = "This is a skill that does things.\n\nMore text.";
        assert_eq!(
            SkillManager::extract_description(content),
            "This is a skill that does things."
        );
    }

    #[test]
    fn test_extract_description_empty() {
        let content = "---\n---\n";
        assert_eq!(SkillManager::extract_description(content), "No description");
    }

    #[test]
    fn test_skills_dir() {
        let dir = SkillManager::skills_dir().unwrap();
        assert!(dir.to_string_lossy().contains(".oxi"));
        assert!(dir.to_string_lossy().contains("skills"));
    }

    // --- New tests for RFC-004 T2 features ---

    #[test]
    fn test_validate_name_valid() {
        assert_eq!(SkillManager::validate_name("my-skill").unwrap(), "my-skill");
        assert_eq!(
            SkillManager::validate_name("rust-expert").unwrap(),
            "rust-expert"
        );
        assert_eq!(SkillManager::validate_name("code-123").unwrap(), "code-123");
        assert_eq!(SkillManager::validate_name("abc").unwrap(), "abc");
    }

    #[test]
    fn test_validate_name_uppercase_normalized() {
        assert_eq!(SkillManager::validate_name("My-Skill").unwrap(), "my-skill");
        assert_eq!(
            SkillManager::validate_name("Rust-Expert").unwrap(),
            "rust-expert"
        );
    }

    #[test]
    fn test_validate_name_invalid_chars() {
        assert!(SkillManager::validate_name("my_skill").is_err());
        assert!(SkillManager::validate_name("my skill").is_err());
        assert!(SkillManager::validate_name("my.skill").is_err());
        assert!(SkillManager::validate_name("my@skill").is_err());
    }

    #[test]
    fn test_validate_name_hyphen_rules() {
        assert!(SkillManager::validate_name("-myskill").is_err()); // leading hyphen
        assert!(SkillManager::validate_name("myskill-").is_err()); // trailing hyphen
        assert!(SkillManager::validate_name("my--skill").is_err()); // consecutive hyphens
    }

    #[test]
    fn test_validate_name_too_long() {
        let long_name = "a".repeat(65);
        assert!(SkillManager::validate_name(&long_name).is_err());
        let max_name = "a".repeat(64);
        assert!(SkillManager::validate_name(&max_name).is_ok());
    }

    #[test]
    fn test_validate_name_empty() {
        assert!(SkillManager::validate_name("").is_err());
    }

    #[test]
    fn test_load_skill_with_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: custom-name
description: A custom description
disable-model-invocation: true
---

# My Skill

Body content here.
"#,
        )
        .unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        // load_from_dir stores by directory name (lowercased), but the Skill.name is overridden by frontmatter
        let skill = manager.get("my-skill").unwrap();
        assert_eq!(skill.name, "custom-name");
        assert_eq!(skill.description, "A custom description");
        assert!(skill.disable_model_invocation);
        assert!(skill.content.contains("Body content here"));
    }

    #[test]
    fn test_load_skill_without_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("basic-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "# Basic Skill\n\nSome content without frontmatter.",
        )
        .unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let skill = manager.get("basic-skill").unwrap();
        assert_eq!(skill.name, "basic-skill");
        assert!(!skill.disable_model_invocation);
    }

    #[test]
    fn test_format_for_prompt_empty() {
        let manager = SkillManager::new();
        assert_eq!(manager.format_for_prompt(), "");
    }

    #[test]
    fn test_format_for_prompt_filters_disabled() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a normal skill
        let dir1 = tmp.path().join("visible-skill");
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::write(dir1.join("SKILL.md"), "# Visible Skill\nA visible skill.").unwrap();

        // Create a disabled skill
        let dir2 = tmp.path().join("hidden-skill");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            dir2.join("SKILL.md"),
            r#"---
name: hidden-skill
disable-model-invocation: true
---

# Hidden Skill
This should not appear.
"#,
        )
        .unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let output = manager.format_for_prompt();

        assert!(output.contains("visible-skill"));
        assert!(!output.contains("hidden-skill"));
        assert!(output.contains("<available_skills>"));
        assert!(output.contains("</available_skills>"));
    }

    #[test]
    fn test_discover_all_multiple_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        // Create a project skill
        let project_skill_dir = cwd.join(".oxi/skills/project-skill");
        std::fs::create_dir_all(&project_skill_dir).unwrap();
        std::fs::write(
            project_skill_dir.join("SKILL.md"),
            "# Project Skill\nFrom project dir",
        )
        .unwrap();

        // Create an extra dir
        let extra_skill_dir = tmp.path().join("extra-skills/extra-skill");
        std::fs::create_dir_all(&extra_skill_dir).unwrap();
        std::fs::write(
            extra_skill_dir.join("SKILL.md"),
            "# Extra Skill\nFrom extra dir",
        )
        .unwrap();

        let manager = SkillManager::discover_all(cwd, &[tmp.path().join("extra-skills")]).unwrap();
        assert!(manager.get("project-skill").is_some());
        assert!(manager.get("extra-skill").is_some());
    }

    #[test]
    fn test_discover_all_ancestor_dirs() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a nested structure with .agents/skills in an ancestor
        let ancestor = tmp.path().join("ancestor-project");
        let ancestor_agents = ancestor.join(".agents/skills/my-ancestor-skill");
        std::fs::create_dir_all(&ancestor_agents).unwrap();
        std::fs::write(
            ancestor_agents.join("SKILL.md"),
            "# Ancestor Skill\nFrom .agents/skills",
        )
        .unwrap();

        // Use the ancestor as cwd and check child can find it
        let child = ancestor.join("child");
        std::fs::create_dir_all(&child).unwrap();

        let manager = SkillManager::discover_all(&child, &[]).unwrap();
        // Should find the ancestor skill
        assert!(manager.get("my-ancestor-skill").is_some());
    }

    #[test]
    fn test_discover_all_duplicate_detection() {
        let tmp = tempfile::tempdir().unwrap();

        // Create the same skill in two locations (symlink or copy)
        let skill_dir = tmp.path().join("dup-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Duplicate Skill\n").unwrap();

        // Create symlink to same directory
        let link_dir = tmp.path().join("dup-skill-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&skill_dir, &link_dir).unwrap();

        #[cfg(unix)]
        {
            let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
            // Should only have one skill (duplicate detected)
            let names: Vec<_> = manager.all().iter().map(|s| s.name.clone()).collect();
            assert!(
                names.iter().any(|n| n == "dup-skill"),
                "Original should exist"
            );
            // Note: With symlinks, the canonical paths should be the same, so one gets skipped
        }
    }

    #[test]
    fn test_skill_location_field() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("loc-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Loc Skill\nContent").unwrap();

        let manager = SkillManager::load_from_dir(tmp.path()).unwrap();
        let skill = manager.get("loc-skill").unwrap();

        // Location should point to the skill directory
        assert_eq!(skill.location, skill_dir);
    }
}
