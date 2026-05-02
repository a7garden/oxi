//! Agent skill for oxi — creating and managing Agent Skills
//!
//! Implements the [Agent Skills standard](https://agentskills.io/specification).
//! This skill enables the agent to create, validate, and manage skills that
//! follow the standard skill directory format:
//!
//! ```text
//! my-skill/
//! ├── SKILL.md        # Required: frontmatter + instructions
//! ├── scripts/        # Optional: helper scripts
//! └── references/     # Optional: detailed docs
//! ```
//!
//! The module provides:
//! - [`SkillFrontmatter`] — frontmatter schema for SKILL.md
//! - [`SkillValidator`] — validates skills against the spec
//! - [`SkillBuilder`] — creates new skill directories with valid SKILL.md
//! - [`AgentSkill`] — skill prompt generator

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum skill name length per the Agent Skills spec.
pub const MAX_NAME_LENGTH: usize = 64;

/// Maximum description length per the Agent Skills spec.
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;

// ── Frontmatter ────────────────────────────────────────────────────────

/// Frontmatter schema for SKILL.md files.
///
/// Implements the [Agent Skills specification](https://agentskills.io/specification#frontmatter-required).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name. Must match parent directory name. Lowercase a-z, 0-9, hyphens.
    /// Max 64 chars. No leading/trailing hyphens. No consecutive hyphens.
    pub name: String,

    /// What this skill does and when to use it. Max 1024 chars.
    pub description: String,

    /// License name or reference to bundled file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Environment requirements. Max 500 chars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,

    /// Arbitrary key-value metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<toml::Value>,

    /// Space-delimited list of pre-approved tools (experimental).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowed-tools")]
    pub allowed_tools: Option<String>,

    /// When true, skill is hidden from system prompt. Users must use /skill:name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "disable-model-invocation")]
    pub disable_model_invocation: bool,
}

// ── Validation ─────────────────────────────────────────────────────────

/// Severity of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    /// Error — skill will not load.
    Error,
    /// Warning — skill loads but may not work correctly.
    Warning,
}

impl fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationSeverity::Error => write!(f, "error"),
            ValidationSeverity::Warning => write!(f, "warning"),
        }
    }
}

/// A single validation finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationFinding {
    /// Severity of the finding.
    pub severity: ValidationSeverity,
    /// Human-readable description.
    pub message: String,
    /// Optional file path that triggered the finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Result of validating a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the skill passed validation (no errors).
    pub valid: bool,
    /// All findings (errors and warnings).
    pub findings: Vec<ValidationFinding>,
}

impl ValidationResult {
    /// Create a passing result.
    pub fn pass() -> Self {
        Self {
            valid: true,
            findings: Vec::new(),
        }
    }

    /// Create a failing result with findings.
    pub fn fail(findings: Vec<ValidationFinding>) -> Self {
        let has_errors = findings.iter().any(|f| f.severity == ValidationSeverity::Error);
        Self {
            valid: !has_errors,
            findings,
        }
    }

    /// Add a finding.
    pub fn add(&mut self, severity: ValidationSeverity, message: impl Into<String>) {
        if severity == ValidationSeverity::Error {
            self.valid = false;
        }
        self.findings.push(ValidationFinding {
            severity,
            message: message.into(),
            path: None,
        });
    }

    /// Add a finding with a path.
    pub fn add_with_path(
        &mut self,
        severity: ValidationSeverity,
        message: impl Into<String>,
        path: impl Into<String>,
    ) {
        if severity == ValidationSeverity::Error {
            self.valid = false;
        }
        self.findings.push(ValidationFinding {
            severity,
            message: message.into(),
            path: Some(path.into()),
        });
    }

    /// Check if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == ValidationSeverity::Error)
    }

    /// Check if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == ValidationSeverity::Warning)
    }
}

/// Validates skills against the Agent Skills specification.
pub struct SkillValidator;

impl SkillValidator {
    /// Validate a skill name per the Agent Skills spec.
    pub fn validate_name(name: &str) -> Vec<ValidationFinding> {
        let mut findings = Vec::new();

        if name.is_empty() {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Error,
                message: "name is required".to_string(),
                path: None,
            });
            return findings;
        }

        if name.len() > MAX_NAME_LENGTH {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Warning,
                message: format!(
                    "name exceeds {} characters ({} chars)",
                    MAX_NAME_LENGTH,
                    name.len()
                ),
                path: None,
            });
        }

        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Warning,
                message: "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                    .to_string(),
                path: None,
            });
        }

        if name.starts_with('-') || name.ends_with('-') {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Warning,
                message: "name must not start or end with a hyphen".to_string(),
                path: None,
            });
        }

        if name.contains("--") {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Warning,
                message: "name must not contain consecutive hyphens".to_string(),
                path: None,
            });
        }

        findings
    }

    /// Validate a skill description per the Agent Skills spec.
    pub fn validate_description(description: &str) -> Vec<ValidationFinding> {
        let mut findings = Vec::new();

        if description.trim().is_empty() {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Error,
                message: "description is required".to_string(),
                path: None,
            });
        } else if description.len() > MAX_DESCRIPTION_LENGTH {
            findings.push(ValidationFinding {
                severity: ValidationSeverity::Warning,
                message: format!(
                    "description exceeds {} characters ({} chars)",
                    MAX_DESCRIPTION_LENGTH,
                    description.len()
                ),
                path: None,
            });
        }

        findings
    }

    /// Validate that a skill name matches its parent directory.
    pub fn validate_name_matches_dir(
        name: &str,
        dir_path: &Path,
    ) -> Vec<ValidationFinding> {
        let dir_name = dir_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        if name != dir_name {
            vec![ValidationFinding {
                severity: ValidationSeverity::Warning,
                message: format!(
                    "name \"{}\" does not match parent directory \"{}\"",
                    name, dir_name
                ),
                path: Some(dir_path.to_string_lossy().to_string()),
            }]
        } else {
            Vec::new()
        }
    }

    /// Validate a complete skill directory.
    pub fn validate_skill_dir(dir: &Path) -> ValidationResult {
        let mut result = ValidationResult::pass();

        // Check SKILL.md exists
        let skill_file = dir.join("SKILL.md");
        if !skill_file.exists() {
            result.add_with_path(
                ValidationSeverity::Error,
                "SKILL.md not found in skill directory",
                dir.to_string_lossy(),
            );
            return result;
        }

        // Read and parse SKILL.md
        let content = match fs::read_to_string(&skill_file) {
            Ok(c) => c,
            Err(e) => {
                result.add_with_path(
                    ValidationSeverity::Error,
                    format!("Failed to read SKILL.md: {}", e),
                    skill_file.to_string_lossy(),
                );
                return result;
            }
        };

        // Parse frontmatter
        let frontmatter = match Self::parse_frontmatter(&content) {
            Ok(fm) => fm,
            Err(e) => {
                result.add_with_path(
                    ValidationSeverity::Error,
                    format!("Failed to parse frontmatter: {}", e),
                    skill_file.to_string_lossy(),
                );
                return result;
            }
        };

        // Validate name
        for finding in Self::validate_name(&frontmatter.name) {
            result.add(finding.severity, finding.message);
        }

        // Validate name matches directory
        for finding in Self::validate_name_matches_dir(&frontmatter.name, dir) {
            result.findings.push(finding);
            if result.has_errors() {
                result.valid = false;
            }
        }

        // Validate description
        for finding in Self::validate_description(&frontmatter.description) {
            result.add(finding.severity, finding.message);
        }

        // Validate compatibility length
        if let Some(ref compat) = frontmatter.compatibility {
            if compat.len() > 500 {
                result.add(
                    ValidationSeverity::Warning,
                    format!(
                        "compatibility exceeds 500 characters ({} chars)",
                        compat.len()
                    ),
                );
            }
        }

        result
    }

    /// Parse YAML frontmatter from SKILL.md content.
    fn parse_frontmatter(content: &str) -> Result<SkillFrontmatter> {
        let trimmed = content.trim();

        // Must start with ---
        if !trimmed.starts_with("---") {
            bail!("SKILL.md must start with YAML frontmatter (---)");
        }

        // Find closing ---
        let rest = &trimmed[3..];
        let end = rest
            .find("---")
            .context("Unclosed frontmatter — missing closing ---")?;

        let yaml_str = &rest[..end];

        // Parse as YAML
        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(yaml_str).context("Invalid YAML frontmatter")?;

        Ok(frontmatter)
    }
}

// ── Skill builder ──────────────────────────────────────────────────────

/// Builds new skill directories with valid SKILL.md files.
pub struct SkillBuilder {
    name: String,
    description: String,
    body: String,
    license: Option<String>,
    compatibility: Option<String>,
    allowed_tools: Vec<String>,
    disable_model_invocation: bool,
}

impl SkillBuilder {
    /// Create a new skill builder with the required fields.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: String::new(),
            license: None,
            compatibility: None,
            allowed_tools: Vec::new(),
            disable_model_invocation: false,
        }
    }

    /// Set the skill body (markdown content after frontmatter).
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Set the license.
    pub fn license(mut self, license: impl Into<String>) -> Self {
        self.license = Some(license.into());
        self
    }

    /// Set compatibility requirements.
    pub fn compatibility(mut self, compat: impl Into<String>) -> Self {
        self.compatibility = Some(compat.into());
        self
    }

    /// Add an allowed tool.
    pub fn allowed_tool(mut self, tool: impl Into<String>) -> Self {
        self.allowed_tools.push(tool.into());
        self
    }

    /// Set whether model invocation is disabled.
    pub fn disable_model_invocation(mut self, disabled: bool) -> Self {
        self.disable_model_invocation = disabled;
        self
    }

    /// Validate the builder state before building.
    pub fn validate(&self) -> ValidationResult {
        let mut result = ValidationResult::pass();

        for finding in SkillValidator::validate_name(&self.name) {
            result.add(finding.severity, finding.message);
        }

        for finding in SkillValidator::validate_description(&self.description) {
            result.add(finding.severity, finding.message);
        }

        if self.body.trim().is_empty() {
            result.add(ValidationSeverity::Warning, "skill body is empty");
        }

        result
    }

    /// Generate the SKILL.md content.
    pub fn render_skill_md(&self) -> String {
        let mut md = String::with_capacity(1024);

        // Frontmatter
        md.push_str("---\n");
        md.push_str(&format!("name: {}\n", self.name));
        md.push_str(&format!("description: {}\n", escape_yaml_string(&self.description)));

        if let Some(ref license) = self.license {
            md.push_str(&format!("license: {}\n", escape_yaml_string(license)));
        }

        if let Some(ref compat) = self.compatibility {
            md.push_str(&format!("compatibility: {}\n", escape_yaml_string(compat)));
        }

        if !self.allowed_tools.is_empty() {
            md.push_str(&format!(
                "allowed-tools: {}\n",
                self.allowed_tools.join(" ")
            ));
        }

        if self.disable_model_invocation {
            md.push_str("disable-model-invocation: true\n");
        }

        md.push_str("---\n\n");

        // Body
        if !self.body.is_empty() {
            md.push_str(&self.body);
            if !self.body.ends_with('\n') {
                md.push('\n');
            }
        } else {
            // Default body template
            md.push_str(&format!("# {}\n\n", capitalize_words(&self.name)));
            md.push_str(&self.description);
            md.push_str("\n\n## Usage\n\nTODO: Add usage instructions.\n");
        }

        md
    }

    /// Build the skill directory on disk.
    pub fn build(&self, parent_dir: &Path) -> Result<PathBuf> {
        // Validate first
        let validation = self.validate();
        if validation.has_errors() {
            let errors: Vec<&str> = validation
                .findings
                .iter()
                .filter(|f| f.severity == ValidationSeverity::Error)
                .map(|f| f.message.as_str())
                .collect();
            bail!("Validation errors: {}", errors.join("; "));
        }

        let skill_dir = parent_dir.join(&self.name);
        fs::create_dir_all(&skill_dir)
            .with_context(|| format!("Failed to create skill directory {}", skill_dir.display()))?;

        let skill_md = self.render_skill_md();
        let skill_path = skill_dir.join("SKILL.md");

        fs::write(&skill_path, &skill_md)
            .with_context(|| format!("Failed to write {}", skill_path.display()))?;

        Ok(skill_dir)
    }
}

// ── Agent skill ────────────────────────────────────────────────────────

/// The agent-skill skill struct.
pub struct AgentSkill;

impl AgentSkill {
    /// Create a new agent-skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Generate the system-prompt fragment for the agent-skill skill.
    pub fn skill_prompt() -> String {
        r#"# Agent Skill

You are running the **agent-skill** skill. You create, validate, and
manage Agent Skills following the [Agent Skills standard](https://agentskills.io/specification).

## Skill Structure

A skill is a directory with a `SKILL.md` file:

```
my-skill/
├── SKILL.md              # Required: frontmatter + instructions
├── scripts/              # Optional: helper scripts
│   └── process.sh
├── references/           # Optional: detailed docs loaded on-demand
│   └── api-reference.md
└── assets/               # Optional: templates, configs
    └── template.json
```

## SKILL.md Format

```markdown
---
name: my-skill
description: What this skill does and when to use it. Be specific.
---

# My Skill

## Usage

Do X then Y.
```

## Frontmatter Rules

| Field | Required | Rules |
|-------|----------|-------|
| `name` | Yes | 1-64 chars, lowercase a-z/0-9/hyphens, no leading/trailing hyphens, no consecutive hyphens, must match parent directory |
| `description` | Yes | 1-1024 chars, describes what the skill does and when to use it |
| `license` | No | License identifier |
| `compatibility` | No | Max 500 chars, environment requirements |
| `allowed-tools` | No | Space-delimited pre-approved tools |
| `disable-model-invocation` | No | When true, hidden from system prompt |

## Validation Checklist

- [ ] Name matches parent directory name
- [ ] Name is lowercase, 1-64 chars, valid characters only
- [ ] Description is present and specific
- [ ] SKILL.md starts and ends with `---` frontmatter delimiters
- [ ] Instructions are clear and actionable
- [ ] Relative paths are used for files within the skill directory
"#
        .to_string()
    }
}

impl Default for AgentSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AgentSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentSkill").finish()
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Escape a string for safe inclusion in YAML value position.
fn escape_yaml_string(s: &str) -> String {
    if s.contains(':') || s.contains('#') || s.contains('"') || s.contains('\'') || s.contains('\n')
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Capitalize the first letter of each word in a hyphenated name.
fn capitalize_words(s: &str) -> String {
    s.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    upper + &chars.as_str().to_lowercase()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_name_valid() {
        let findings = SkillValidator::validate_name("pdf-tools");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_validate_name_empty() {
        let findings = SkillValidator::validate_name("");
        assert!(findings.iter().any(|f| f.severity == ValidationSeverity::Error));
    }

    #[test]
    fn test_validate_name_too_long() {
        let name = "a".repeat(65);
        let findings = SkillValidator::validate_name(&name);
        assert!(findings.iter().any(|f| f.message.contains("exceeds 64 characters")));
    }

    #[test]
    fn test_validate_name_at_limit() {
        let name = "a".repeat(64);
        let findings = SkillValidator::validate_name(&name);
        assert!(!findings.iter().any(|f| f.message.contains("exceeds")));
    }

    #[test]
    fn test_validate_name_uppercase() {
        let findings = SkillValidator::validate_name("My-Skill");
        assert!(findings.iter().any(|f| f.message.contains("invalid characters")));
    }

    #[test]
    fn test_validate_name_leading_hyphen() {
        let findings = SkillValidator::validate_name("-skill");
        assert!(findings.iter().any(|f| f.message.contains("start or end with a hyphen")));
    }

    #[test]
    fn test_validate_name_trailing_hyphen() {
        let findings = SkillValidator::validate_name("skill-");
        assert!(findings.iter().any(|f| f.message.contains("start or end with a hyphen")));
    }

    #[test]
    fn test_validate_name_consecutive_hyphens() {
        let findings = SkillValidator::validate_name("my--skill");
        assert!(findings.iter().any(|f| f.message.contains("consecutive hyphens")));
    }

    #[test]
    fn test_validate_name_with_numbers() {
        let findings = SkillValidator::validate_name("pdf2text");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_validate_description_valid() {
        let findings = SkillValidator::validate_description("A useful skill");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_validate_description_empty() {
        let findings = SkillValidator::validate_description("");
        assert!(findings.iter().any(|f| f.severity == ValidationSeverity::Error));
    }

    #[test]
    fn test_validate_description_whitespace_only() {
        let findings = SkillValidator::validate_description("   ");
        assert!(findings.iter().any(|f| f.severity == ValidationSeverity::Error));
    }

    #[test]
    fn test_validate_description_too_long() {
        let desc = "x".repeat(1025);
        let findings = SkillValidator::validate_description(&desc);
        assert!(findings.iter().any(|f| f.message.contains("exceeds 1024 characters")));
    }

    #[test]
    fn test_validate_description_at_limit() {
        let desc = "x".repeat(1024);
        let findings = SkillValidator::validate_description(&desc);
        assert!(!findings.iter().any(|f| f.message.contains("exceeds")));
    }

    #[test]
    fn test_validate_name_matches_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my-skill");
        fs::create_dir_all(&dir).unwrap();
        let findings = SkillValidator::validate_name_matches_dir("my-skill", &dir);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_validate_name_mismatches_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("other-skill");
        fs::create_dir_all(&dir).unwrap();
        let findings = SkillValidator::validate_name_matches_dir("my-skill", &dir);
        assert!(!findings.is_empty());
        assert!(findings[0].message.contains("does not match parent directory"));
    }

    #[test]
    fn test_validate_skill_dir_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my-skill");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: A test skill\n---\n\n# My Skill\n",
        )
        .unwrap();
        let result = SkillValidator::validate_skill_dir(&dir);
        assert!(result.valid);
        assert!(result.findings.is_empty());
    }

    #[test]
    fn test_validate_skill_dir_no_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my-skill");
        fs::create_dir_all(&dir).unwrap();
        let result = SkillValidator::validate_skill_dir(&dir);
        assert!(!result.valid);
        assert!(result.findings.iter().any(|f| f.message.contains("SKILL.md not found")));
    }

    #[test]
    fn test_validation_result_pass() {
        let result = ValidationResult::pass();
        assert!(result.valid);
        assert!(result.findings.is_empty());
    }

    #[test]
    fn test_validation_result_add_error() {
        let mut result = ValidationResult::pass();
        result.add(ValidationSeverity::Error, "something wrong");
        assert!(!result.valid);
        assert!(result.has_errors());
    }

    #[test]
    fn test_validation_result_add_warning() {
        let mut result = ValidationResult::pass();
        result.add(ValidationSeverity::Warning, "minor issue");
        assert!(result.valid);
        assert!(!result.has_errors());
        assert!(result.has_warnings());
    }

    #[test]
    fn test_builder_new() {
        let builder = SkillBuilder::new("my-skill", "A test skill");
        assert_eq!(builder.name, "my-skill");
        assert_eq!(builder.description, "A test skill");
    }

    #[test]
    fn test_builder_validate_valid() {
        let builder = SkillBuilder::new("my-skill", "A test skill");
        let result = builder.validate();
        assert!(result.valid);
    }

    #[test]
    fn test_builder_validate_bad_name() {
        let builder = SkillBuilder::new("MY-SKILL", "A test skill");
        let result = builder.validate();
        assert!(result.has_warnings());
    }

    #[test]
    fn test_builder_validate_empty_description() {
        let builder = SkillBuilder::new("my-skill", "");
        let result = builder.validate();
        assert!(result.has_errors());
    }

    #[test]
    fn test_builder_render_skill_md() {
        let builder = SkillBuilder::new("pdf-tools", "Extract text from PDFs")
            .body("# PDF Tools\n\n## Usage\n\n```bash\npdftotext input.pdf\n```");
        let md = builder.render_skill_md();
        assert!(md.starts_with("---\n"));
        assert!(md.contains("name: pdf-tools"));
        assert!(md.contains("description: Extract text from PDFs"));
        assert!(md.contains("# PDF Tools"));
    }

    #[test]
    fn test_builder_render_with_options() {
        let builder = SkillBuilder::new("my-skill", "Test")
            .license("MIT")
            .compatibility("Node.js >= 18")
            .allowed_tool("read")
            .allowed_tool("bash")
            .disable_model_invocation(true);
        let md = builder.render_skill_md();
        assert!(md.contains("license: MIT"));
        assert!(md.contains("compatibility: Node.js >= 18"));
        assert!(md.contains("allowed-tools: read bash"));
        assert!(md.contains("disable-model-invocation: true"));
    }

    #[test]
    fn test_builder_build() {
        let tmp = tempfile::tempdir().unwrap();
        let builder = SkillBuilder::new("my-skill", "A test skill for building");
        let dir = builder.build(tmp.path()).unwrap();
        assert!(dir.exists());
        assert_eq!(dir.file_name().unwrap(), "my-skill");
        let skill_md = dir.join("SKILL.md");
        assert!(skill_md.exists());
        let content = fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("name: my-skill"));
    }

    #[test]
    fn test_builder_build_invalid_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let builder = SkillBuilder::new("MY BAD SKILL", "");
        assert!(builder.build(tmp.path()).is_err());
    }

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = AgentSkill::skill_prompt();
        assert!(prompt.contains("Agent Skill"));
        assert!(prompt.contains("SKILL.md"));
    }

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = "---\nname: my-skill\ndescription: A test\n---\n\n# Body";
        let fm = SkillValidator::parse_frontmatter(content).unwrap();
        assert_eq!(fm.name, "my-skill");
        assert_eq!(fm.description, "A test");
    }

    #[test]
    fn test_parse_frontmatter_no_delimiter() {
        let content = "# Just markdown";
        assert!(SkillValidator::parse_frontmatter(content).is_err());
    }

    #[test]
    fn test_parse_frontmatter_unclosed() {
        let content = "---\nname: my-skill";
        assert!(SkillValidator::parse_frontmatter(content).is_err());
    }

    #[test]
    fn test_capitalize_words() {
        assert_eq!(capitalize_words("pdf-tools"), "Pdf Tools");
        assert_eq!(capitalize_words("my-skill"), "My Skill");
        assert_eq!(capitalize_words("a"), "A");
        assert_eq!(capitalize_words(""), "");
    }

    #[test]
    fn test_full_create_and_validate() {
        let tmp = tempfile::tempdir().unwrap();
        let builder = SkillBuilder::new("web-search", "Search the web using Brave Search API")
            .body("# Web Search\n\n## Usage\n\n```bash\n./scripts/search.sh \"query\"\n```")
            .allowed_tool("bash")
            .allowed_tool("read");
        let dir = builder.build(tmp.path()).unwrap();
        let result = SkillValidator::validate_skill_dir(&dir);
        assert!(result.valid, "Validation failed: {:?}", result.findings);
    }
}
