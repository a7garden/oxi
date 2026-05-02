//! Skills system for oxi CLI
//!
//! Skills are on-demand capability packages stored as markdown files.
//! Each skill lives in its own directory under `~/.oxi/skills/<name>/SKILL.md`.
//!
//! When a skill is invoked, its content is appended to the system prompt,
//! giving the agent additional context and instructions for that skill.

pub mod autonomous_loop;
pub mod brainstorming;
pub mod context_builder;
pub mod deep_research;
pub mod design_farmer;
pub mod obsidian;
pub mod oracle;
pub mod planner;
pub mod playwright_cli;
pub mod reviewer;
pub mod scout;
pub mod super_review;
pub mod worktree;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// A single skill loaded from a SKILL.md file.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name (directory name, e.g. "my-skill")
    pub name: String,
    /// Short description extracted from the first line or frontmatter
    pub description: String,
    /// Full markdown content from SKILL.md
    pub content: String,
}

impl fmt::Display for Skill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.description)
    }
}

/// Manages loading and querying skills from the filesystem.
pub struct SkillManager {
    skills: HashMap<String, Skill>,
}

impl SkillManager {
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

            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            match Self::load_skill(&name, &skill_file) {
                Ok(skill) => {
                    tracing::debug!("Loaded skill: {}", skill.name);
                    skills.insert(name.to_lowercase(), skill);
                }
                Err(e) => {
                    tracing::warn!("Failed to load skill from {}: {}", skill_file.display(), e);
                }
            }
        }

        tracing::info!("Loaded {} skill(s) from {}", skills.len(), dir.display());
        Ok(Self { skills })
    }

    /// Load a single skill from its SKILL.md file.
    fn load_skill(name: &str, path: &Path) -> Result<Skill> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let description = Self::extract_description(&content);

        Ok(Skill {
            name: name.to_string(),
            description,
            content,
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
}

impl fmt::Debug for SkillManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkillManager")
            .field("count", &self.skills.len())
            .field(
                "names",
                &self
                    .skills
                    .keys()
                    .cloned()
                    .collect::<Vec<String>>(),
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
        fs::write(dir.join("SKILL.md"), "# Helper\nA database optimization expert").unwrap();

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
        fs::write(dir.join("SKILL.md"), "# Coder\nA coding assistant\n\n## Details\nFocuses on async patterns").unwrap();

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
}
