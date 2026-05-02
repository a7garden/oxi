//! Brainstorming skill for oxi
//!
//! An interactive design-exploration workflow that guides a user from a vague
//! idea to a concrete, documented design. The workflow proceeds through five
//! phases:
//!
//! 1. **Context** — Explore project structure, existing code, and conventions.
//! 2. **Questions** — Identify ambiguities and ask targeted clarifying questions.
//! 3. **Approaches** — Propose 2–3 candidate approaches with trade-off analysis.
//! 4. **Design** — Present a detailed design covering architecture, components,
//!    and data flow.
//! 5. **Document** — Write the approved design to a Markdown file.
//!
//! The module is designed to be usable both as a library (driven programmatically)
//! and as a skill whose SKILL.md is appended to the agent system prompt. The
//! [`BrainstormSession`] struct tracks the full lifecycle and produces a
//! structured [`DesignDocument`] that can be serialized to disk.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

// ── Types ──────────────────────────────────────────────────────────────

/// The phase a brainstorm session is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Gathering context from the project.
    Context,
    /// Asking clarifying questions.
    Questions,
    /// Proposing candidate approaches.
    Approaches,
    /// Presenting the detailed design.
    Design,
    /// Writing the design document to disk.
    Document,
    /// Session concluded.
    Done,
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Context
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Context => write!(f, "Context"),
            Phase::Questions => write!(f, "Questions"),
            Phase::Approaches => write!(f, "Approaches"),
            Phase::Design => write!(f, "Design"),
            Phase::Document => write!(f, "Document"),
            Phase::Done => write!(f, "Done"),
        }
    }
}

/// A single clarifying question with its answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarifyingQuestion {
    /// The question text.
    pub question: String,
    /// The rationale for why this question matters.
    pub rationale: String,
    /// Category for grouping (e.g., "scope", "constraints", "users").
    pub category: String,
    /// User-provided answer (filled in later).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// A candidate approach with its trade-offs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approach {
    /// Short name for this approach.
    pub name: String,
    /// One-line summary.
    pub summary: String,
    /// Detailed description.
    pub description: String,
    /// Key advantages.
    pub pros: Vec<String>,
    /// Key disadvantages or risks.
    pub cons: Vec<String>,
    /// Estimated complexity (low / medium / high).
    pub complexity: Complexity,
    /// Rough time estimate (e.g., "2-3 days", "1 week").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_effort: Option<String>,
}

/// Complexity level for an approach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
    Low,
    Medium,
    High,
}

impl fmt::Display for Complexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Complexity::Low => write!(f, "low"),
            Complexity::Medium => write!(f, "medium"),
            Complexity::High => write!(f, "high"),
        }
    }
}

/// A component within the design.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    /// Component name.
    pub name: String,
    /// What this component is responsible for.
    pub responsibility: String,
    /// Key interfaces / APIs this component exposes.
    #[serde(default)]
    pub interfaces: Vec<String>,
    /// Dependencies on other components.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// The final design document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignDocument {
    /// Title of the design.
    pub title: String,
    /// Brief summary / abstract.
    pub summary: String,
    /// Problem statement being solved.
    pub problem_statement: String,
    /// Key goals / requirements.
    pub goals: Vec<String>,
    /// Non-goals (explicitly out of scope).
    #[serde(default)]
    pub non_goals: Vec<String>,
    /// Context gathered from the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_context: Option<String>,
    /// Clarifying questions and answers.
    #[serde(default)]
    pub questions: Vec<ClarifyingQuestion>,
    /// Candidate approaches that were considered.
    pub approaches: Vec<Approach>,
    /// Index of the chosen approach (0-based).
    pub chosen_approach: usize,
    /// Rationale for the choice.
    pub choice_rationale: String,
    /// Architectural overview text.
    pub architecture: String,
    /// Components that make up the design.
    pub components: Vec<Component>,
    /// Data flow description.
    pub data_flow: String,
    /// Files that need to be created or modified.
    #[serde(default)]
    pub file_plan: Vec<FileEntry>,
    /// Open questions / risks remaining.
    #[serde(default)]
    pub open_risks: Vec<String>,
    /// Author metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
}

/// A file entry in the implementation plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// File path (relative to project root).
    pub path: String,
    /// Action to take.
    pub action: FileAction,
    /// Brief description of changes.
    pub description: String,
}

/// What to do with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    /// Create a new file.
    Create,
    /// Modify an existing file.
    Modify,
    /// Delete a file.
    Delete,
}

impl fmt::Display for FileAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileAction::Create => write!(f, "create"),
            FileAction::Modify => write!(f, "modify"),
            FileAction::Delete => write!(f, "delete"),
        }
    }
}

/// The state of a brainstorming session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainstormSession {
    /// Unique session identifier.
    pub id: String,
    /// Current phase.
    pub phase: Phase,
    /// The original topic / idea from the user.
    pub topic: String,
    /// Project root directory (for context gathering).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// Context gathered from the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_context: Option<String>,
    /// Clarifying questions identified.
    #[serde(default)]
    pub questions: Vec<ClarifyingQuestion>,
    /// Candidate approaches.
    #[serde(default)]
    pub approaches: Vec<Approach>,
    /// The final design document (set once in the Design phase).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design: Option<DesignDocument>,
}

// ── Session lifecycle ──────────────────────────────────────────────────

impl BrainstormSession {
    /// Start a new brainstorming session for the given topic.
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            phase: Phase::Context,
            topic: topic.into(),
            project_root: None,
            project_context: None,
            questions: Vec::new(),
            approaches: Vec::new(),
            design: None,
        }
    }

    /// Set the project root for context gathering.
    pub fn with_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    /// Advance to the next phase.
    ///
    /// Enforces ordering: Context → Questions → Approaches → Design → Document → Done.
    pub fn advance(&mut self) -> Result<()> {
        let next = match self.phase {
            Phase::Context => Phase::Questions,
            Phase::Questions => Phase::Approaches,
            Phase::Approaches => Phase::Design,
            Phase::Design => Phase::Document,
            Phase::Document => Phase::Done,
            Phase::Done => bail!("Session is already complete"),
        };
        self.phase = next;
        Ok(())
    }

    /// Jump to a specific phase (for resuming or revisiting).
    pub fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    // ── Phase 1: Context ─────────────────────────────────────────────

    /// Set the project context summary.
    pub fn set_project_context(&mut self, context: impl Into<String>) {
        self.project_context = Some(context.into());
    }

    /// Quick project context from files that commonly exist.
    ///
    /// Reads up to `max_bytes` bytes from well-known project files
    /// (README.md, Cargo.toml, package.json, etc.) to build a compact
    /// summary string.
    pub fn gather_project_context(&self, max_bytes: usize) -> Result<String> {
        let root = self
            .project_root
            .as_deref()
            .context("No project root set")?;

        let probe_files = [
            "README.md",
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "go.mod",
            "Makefile",
            "AGENTS.md",
            "CONTRIBUTING.md",
            ".oxi/settings.toml",
        ];

        let mut context_parts: Vec<String> = Vec::new();
        let mut bytes_used: usize = 0;

        for filename in &probe_files {
            let path = root.join(filename);
            if !path.exists() {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;

            // Truncate if too long
            let available = max_bytes.saturating_sub(bytes_used);
            if available == 0 {
                break;
            }

            let truncated = if content.len() > available {
                &content[..available]
            } else {
                &content
            };

            context_parts.push(format!("## {filename}\n{truncated}"));
            bytes_used += truncated.len();

            if bytes_used >= max_bytes {
                break;
            }
        }

        // Also list the top-level directory structure (first level only).
        if let Ok(entries) = std::fs::read_dir(root) {
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if e.path().is_dir() {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .filter(|n| !n.starts_with('.'))
                .collect();
            names.sort();

            if !names.is_empty() {
                context_parts.push(format!("## Directory Structure\n{}", names.join("\n")));
            }
        }

        Ok(context_parts.join("\n\n"))
    }

    // ── Phase 2: Questions ───────────────────────────────────────────

    /// Add a clarifying question.
    pub fn add_question(
        &mut self,
        question: impl Into<String>,
        rationale: impl Into<String>,
        category: impl Into<String>,
    ) {
        self.questions.push(ClarifyingQuestion {
            question: question.into(),
            rationale: rationale.into(),
            category: category.into(),
            answer: None,
        });
    }

    /// Answer a question by index.
    pub fn answer_question(&mut self, index: usize, answer: impl Into<String>) -> Result<()> {
        let q = self
            .questions
            .get_mut(index)
            .with_context(|| format!("No question at index {}", index))?;
        q.answer = Some(answer.into());
        Ok(())
    }

    /// Check whether all questions have been answered.
    pub fn all_questions_answered(&self) -> bool {
        self.questions.iter().all(|q| q.answer.is_some())
    }

    /// Get unanswered questions.
    pub fn unanswered_questions(&self) -> Vec<&ClarifyingQuestion> {
        self.questions.iter().filter(|q| q.answer.is_none()).collect()
    }

    // ── Phase 3: Approaches ──────────────────────────────────────────

    /// Add a candidate approach.
    pub fn add_approach(&mut self, approach: Approach) {
        self.approaches.push(approach);
    }

    /// Get the number of approaches.
    pub fn approach_count(&self) -> usize {
        self.approaches.len()
    }

    // ── Phase 4: Design ──────────────────────────────────────────────

    /// Finalize the design by choosing an approach and building the full
    /// [`DesignDocument`].
    pub fn finalize_design(
        &mut self,
        chosen_approach: usize,
        choice_rationale: impl Into<String>,
        architecture: impl Into<String>,
        components: Vec<Component>,
        data_flow: impl Into<String>,
        file_plan: Vec<FileEntry>,
        open_risks: Vec<String>,
    ) -> Result<()> {
        if chosen_approach >= self.approaches.len() {
            bail!(
                "Invalid approach index {} (only {} approaches defined)",
                chosen_approach,
                self.approaches.len()
            );
        }

        let goals = self.extract_goals();
        let non_goals = self.extract_non_goals();

        let doc = DesignDocument {
            title: self.topic.clone(),
            summary: self
                .approaches
                .get(chosen_approach)
                .map(|a| a.summary.clone())
                .unwrap_or_default(),
            problem_statement: self.topic.clone(),
            goals,
            non_goals,
            project_context: self.project_context.clone(),
            questions: self.questions.clone(),
            approaches: self.approaches.clone(),
            chosen_approach,
            choice_rationale: choice_rationale.into(),
            architecture: architecture.into(),
            components,
            data_flow: data_flow.into(),
            file_plan,
            open_risks,
            author: None,
            created_at: Utc::now().to_rfc3339(),
        };

        self.design = Some(doc);
        Ok(())
    }

    /// Extract goals from answered questions.
    fn extract_goals(&self) -> Vec<String> {
        self.questions
            .iter()
            .filter(|q| {
                q.category == "goals"
                    || q.category == "requirements"
                    || q.category == "scope"
            })
            .filter_map(|q| q.answer.as_ref())
            .cloned()
            .collect()
    }

    /// Extract non-goals from answered questions.
    fn extract_non_goals(&self) -> Vec<String> {
        self.questions
            .iter()
            .filter(|q| q.category == "non-goals" || q.category == "out_of_scope")
            .filter_map(|q| q.answer.as_ref())
            .cloned()
            .collect()
    }

    // ── Phase 5: Document ────────────────────────────────────────────

    /// Render the design document as Markdown.
    pub fn render_markdown(&self) -> Result<String> {
        let doc = self
            .design
            .as_ref()
            .context("Design has not been finalized yet")?;

        Ok(render_design_markdown(doc))
    }

    /// Write the design document to a Markdown file.
    ///
    /// Defaults to `docs/design/YYYY-MM-DD-<slugified-title>.md` under
    /// the project root, but an explicit path can be provided.
    pub fn write_document(&self, path: Option<&Path>) -> Result<PathBuf> {
        let doc = self
            .design
            .as_ref()
            .context("Design has not been finalized yet")?;

        let output_path = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let root = self
                    .project_root
                    .as_deref()
                    .context("No project root set and no explicit path provided")?;
                let date = &doc.created_at[..10]; // YYYY-MM-DD
                let slug = slugify(&doc.title);
                let design_dir = root.join("docs").join("design");
                std::fs::create_dir_all(&design_dir).with_context(|| {
                    format!("Failed to create {}", design_dir.display())
                })?;
                design_dir.join(format!("{date}-{slug}.md"))
            }
        };

        let markdown = render_design_markdown(doc);

        // Ensure parent directory exists.
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create {}", parent.display())
            })?;
        }

        std::fs::write(&output_path, &markdown).with_context(|| {
            format!("Failed to write design document to {}", output_path.display())
        })?;

        Ok(output_path)
    }
}

// ── Markdown rendering ─────────────────────────────────────────────────

/// Render a [`DesignDocument`] into a well-structured Markdown string.
fn render_design_markdown(doc: &DesignDocument) -> String {
    let mut md = String::with_capacity(4096);

    // Title
    md.push_str(&format!("# {}\n\n", doc.title));

    // Metadata
    md.push_str(&format!("> Created: {}\n", doc.created_at));
    if let Some(ref author) = doc.author {
        md.push_str(&format!("> Author: {}\n", author));
    }
    md.push('\n');

    // Summary
    md.push_str("## Summary\n\n");
    md.push_str(&doc.summary);
    md.push_str("\n\n");

    // Problem statement
    md.push_str("## Problem Statement\n\n");
    md.push_str(&doc.problem_statement);
    md.push_str("\n\n");

    // Goals
    if !doc.goals.is_empty() {
        md.push_str("## Goals\n\n");
        for goal in &doc.goals {
            md.push_str(&format!("- {}\n", goal));
        }
        md.push('\n');
    }

    // Non-goals
    if !doc.non_goals.is_empty() {
        md.push_str("## Non-Goals\n\n");
        for ng in &doc.non_goals {
            md.push_str(&format!("- {}\n", ng));
        }
        md.push('\n');
    }

    // Project context
    if let Some(ref ctx) = doc.project_context {
        md.push_str("## Project Context\n\n");
        md.push_str(ctx);
        md.push_str("\n\n");
    }

    // Clarifying questions
    if !doc.questions.is_empty() {
        md.push_str("## Clarifying Questions\n\n");
        for (i, q) in doc.questions.iter().enumerate() {
            md.push_str(&format!("### Q{}: {}\n\n", i + 1, q.question));
            md.push_str(&format!("**Rationale:** {}\n\n", q.rationale));
            if let Some(ref answer) = q.answer {
                md.push_str(&format!("**Answer:** {}\n\n", answer));
            } else {
                md.push_str("**Answer:** *(unanswered)*\n\n");
            }
        }
    }

    // Approaches
    md.push_str("## Approaches Considered\n\n");
    for (i, approach) in doc.approaches.iter().enumerate() {
        let chosen_marker = if i == doc.chosen_approach {
            " **(chosen)**"
        } else {
            ""
        };
        md.push_str(&format!("### {}{}\n\n", approach.name, chosen_marker));
        md.push_str(&format!("{}\n\n", approach.summary));
        md.push_str(&format!("{}\n\n", approach.description));

        md.push_str("**Pros:**\n\n");
        for pro in &approach.pros {
            md.push_str(&format!("+ {}\n", pro));
        }
        md.push('\n');

        md.push_str("**Cons:**\n\n");
        for con in &approach.cons {
            md.push_str(&format!("- {}\n", con));
        }
        md.push('\n');

        md.push_str(&format!(
            "**Complexity:** {}",
            approach.complexity
        ));
        if let Some(ref effort) = approach.estimated_effort {
            md.push_str(&format!(" | **Effort:** {}", effort));
        }
        md.push_str("\n\n");
    }

    // Choice rationale
    md.push_str("## Choice Rationale\n\n");
    md.push_str(&doc.choice_rationale);
    md.push_str("\n\n");

    // Architecture
    md.push_str("## Architecture\n\n");
    md.push_str(&doc.architecture);
    md.push_str("\n\n");

    // Components
    if !doc.components.is_empty() {
        md.push_str("## Components\n\n");
        for component in &doc.components {
            md.push_str(&format!("### {}\n\n", component.name));
            md.push_str(&format!("{}\n\n", component.responsibility));

            if !component.interfaces.is_empty() {
                md.push_str("**Interfaces:**\n\n");
                for iface in &component.interfaces {
                    md.push_str(&format!("- {}\n", iface));
                }
                md.push('\n');
            }

            if !component.depends_on.is_empty() {
                md.push_str(&format!(
                    "**Depends on:** {}\n\n",
                    component.depends_on.join(", ")
                ));
            }
        }
    }

    // Data flow
    md.push_str("## Data Flow\n\n");
    md.push_str(&doc.data_flow);
    md.push_str("\n\n");

    // File plan
    if !doc.file_plan.is_empty() {
        md.push_str("## File Plan\n\n");
        md.push_str("| Action | Path | Description |\n");
        md.push_str("|--------|------|-------------|\n");
        for entry in &doc.file_plan {
            md.push_str(&format!(
                "| {} | `{}` | {} |\n",
                entry.action, entry.path, entry.description
            ));
        }
        md.push('\n');
    }

    // Open risks
    if !doc.open_risks.is_empty() {
        md.push_str("## Open Risks\n\n");
        for risk in &doc.open_risks {
            md.push_str(&format!("- {}\n", risk));
        }
        md.push('\n');
    }

    md
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert a title into a filesystem-safe slug.
fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else if c == ' ' || c == '_' {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|c| *c != '\0')
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

// ── Prompt generation ──────────────────────────────────────────────────

/// Generate a system-prompt fragment that instructs the agent to follow the
/// brainstorming workflow. This is intended to be appended to the base system
/// prompt when the brainstorming skill is active.
pub fn brainstorm_skill_prompt() -> String {
    let prompt = r#"# Brainstorming Skill

You are running the **brainstorming** skill. Your job is to guide the user
through a structured design exploration, producing a concrete design document.

## Workflow

Follow these phases strictly. Do not skip ahead.

### Phase 1: Context

1. Ask the user for the project root directory (or infer it from the current working directory).
2. Read key project files (README, manifest, config) to understand the codebase.
3. Summarize what you found and confirm understanding with the user.

### Phase 2: Clarifying Questions

1. Identify 3–8 critical ambiguities in the user's idea.
2. For each, ask a targeted question and explain *why* it matters.
3. Group questions by category (scope, constraints, users, performance, etc.).
4. Wait for the user to answer all questions before proceeding.

### Phase 3: Approaches

1. Propose 2–3 distinct approaches to solving the problem.
2. For each approach, provide:
   - Name and one-line summary
   - Detailed description
   - Pros (list)
   - Cons / risks (list)
   - Complexity rating (low / medium / high)
   - Estimated effort
3. Present a comparison table summarizing the approaches.
4. Wait for the user to select an approach.

### Phase 4: Design

1. Present a detailed design for the chosen approach, including:
   - Architecture overview
   - Component breakdown with responsibilities, interfaces, and dependencies
   - Data flow description
   - File plan (which files to create / modify / delete)
   - Open risks or unresolved questions
2. Walk through the design and invite feedback.
3. Iterate until the user approves.

### Phase 5: Document

1. Write the approved design to `docs/design/YYYY-MM-DD-<slug>.md`.
2. Confirm the file was written successfully.
3. Suggest next steps (e.g., hand off to the autonomous-loop skill).

## Rules

- Never propose more than 3 approaches. Force prioritization.
- Every question must have a clear rationale — do not ask lazy questions.
- The design must be concrete enough to hand off directly to implementation.
- Prefer simplicity. If two approaches are equally viable, recommend the simpler one.
- Document all decisions and their reasoning.
"#;
    prompt.to_string()
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_approach(name: &str) -> Approach {
        Approach {
            name: name.to_string(),
            summary: format!("{} summary", name),
            description: format!("{} description", name),
            pros: vec!["fast".to_string()],
            cons: vec!["risky".to_string()],
            complexity: Complexity::Medium,
            estimated_effort: Some("1 week".to_string()),
        }
    }

    fn sample_component(name: &str) -> Component {
        Component {
            name: name.to_string(),
            responsibility: format!("{} does things", name),
            interfaces: vec![format!("{}.run() -> Result", name)],
            depends_on: vec![],
        }
    }

    #[test]
    fn test_session_new() {
        let session = BrainstormSession::new("Build a cache layer");
        assert_eq!(session.phase, Phase::Context);
        assert_eq!(session.topic, "Build a cache layer");
        assert!(session.questions.is_empty());
        assert!(session.approaches.is_empty());
        assert!(session.design.is_none());
    }

    #[test]
    fn test_phase_advance() {
        let mut session = BrainstormSession::new("test");
        assert_eq!(session.phase, Phase::Context);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Questions);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Approaches);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Design);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Document);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Done);

        // Cannot advance past Done
        assert!(session.advance().is_err());
    }

    #[test]
    fn test_set_phase() {
        let mut session = BrainstormSession::new("test");
        session.set_phase(Phase::Design);
        assert_eq!(session.phase, Phase::Design);
    }

    #[test]
    fn test_add_and_answer_questions() {
        let mut session = BrainstormSession::new("test");

        session.add_question("What is the scope?", "Defines boundaries", "scope");
        session.add_question("Any perf requirements?", "Affects approach", "constraints");

        assert_eq!(session.questions.len(), 2);
        assert!(!session.all_questions_answered());
        assert_eq!(session.unanswered_questions().len(), 2);

        session.answer_question(0, "API layer only").unwrap();
        assert!(!session.all_questions_answered());
        assert_eq!(session.unanswered_questions().len(), 1);

        session.answer_question(1, "< 50ms p99").unwrap();
        assert!(session.all_questions_answered());
        assert!(session.unanswered_questions().is_empty());
    }

    #[test]
    fn test_answer_invalid_index() {
        let mut session = BrainstormSession::new("test");
        session.add_question("Q1?", "R1", "cat");
        let result = session.answer_question(5, "answer");
        assert!(result.is_err());
    }

    #[test]
    fn test_add_approaches() {
        let mut session = BrainstormSession::new("test");
        session.add_approach(sample_approach("A"));
        session.add_approach(sample_approach("B"));
        assert_eq!(session.approach_count(), 2);
    }

    #[test]
    fn test_finalize_design_invalid_approach() {
        let mut session = BrainstormSession::new("test");
        session.add_approach(sample_approach("A"));

        let result = session.finalize_design(
            5, // out of bounds
            "reason",
            "arch",
            vec![],
            "flow",
            vec![],
            vec![],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_finalize_and_render_design() {
        let mut session = BrainstormSession::new("Build a cache layer");
        session.add_question("Scope?", "Defines boundaries", "goals");
        session.answer_question(0, "API layer only").unwrap();
        session.add_question("Out of scope?", "What to exclude", "non-goals");
        session.answer_question(1, "CLI tools").unwrap();
        session.add_approach(sample_approach("In-memory LRU"));
        session.add_approach(sample_approach("Redis-backed"));

        session
            .finalize_design(
                0,
                "In-memory is simpler and sufficient for single-process use",
                "Layered architecture: Cache trait -> LRU implementation -> integration point",
                vec![
                    sample_component("CacheStore"),
                    sample_component("CacheConfig"),
                ],
                "Request -> CacheStore.get() -> hit: return / miss: compute -> store -> return",
                vec![
                    FileEntry {
                        path: "src/cache.rs".to_string(),
                        action: FileAction::Create,
                        description: "Core cache module".to_string(),
                    },
                    FileEntry {
                        path: "src/lib.rs".to_string(),
                        action: FileAction::Modify,
                        description: "Add cache module declaration".to_string(),
                    },
                ],
                vec!["Cache invalidation strategy TBD".to_string()],
            )
            .unwrap();

        let doc = session.design.as_ref().unwrap();
        assert_eq!(doc.title, "Build a cache layer");
        assert_eq!(doc.chosen_approach, 0);
        assert_eq!(doc.components.len(), 2);
        assert_eq!(doc.file_plan.len(), 2);
        assert_eq!(doc.open_risks.len(), 1);

        // Render markdown
        let md = session.render_markdown().unwrap();
        assert!(md.contains("# Build a cache layer"));
        assert!(md.contains("## Approaches Considered"));
        assert!(md.contains("In-memory LRU"));
        assert!(md.contains("## Architecture"));
        assert!(md.contains("## Components"));
        assert!(md.contains("## Data Flow"));
        assert!(md.contains("## File Plan"));
        assert!(md.contains("| create | `src/cache.rs` |"));
        assert!(md.contains("## Open Risks"));
        assert!(md.contains("(chosen)"));
    }

    #[test]
    fn test_render_markdown_before_finalize() {
        let session = BrainstormSession::new("test");
        assert!(session.render_markdown().is_err());
    }

    #[test]
    fn test_write_document_without_design() {
        let session = BrainstormSession::new("test");
        assert!(session.write_document(None).is_err());
    }

    #[test]
    fn test_write_document_to_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = BrainstormSession::new("Test Design");
        session.project_root = Some(tmp.path().to_path_buf());
        session.add_approach(sample_approach("Simple"));
        session
            .finalize_design(
                0,
                "Simplest option",
                "Flat architecture",
                vec![],
                "N/A",
                vec![],
                vec![],
            )
            .unwrap();

        let path = session.write_document(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("docs/design"));

        // Verify content
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Test Design"));
        assert!(content.contains("Simple"));
    }

    #[test]
    fn test_write_document_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = BrainstormSession::new("Test");
        session.add_approach(sample_approach("A"));
        session
            .finalize_design(0, "r", "a", vec![], "f", vec![], vec![])
            .unwrap();

        let explicit = tmp.path().join("custom-design.md");
        let path = session.write_document(Some(&explicit)).unwrap();
        assert_eq!(path, explicit);
        assert!(path.exists());
    }

    #[test]
    fn test_gather_project_context() {
        let tmp = tempfile::tempdir().unwrap();

        // Create some project files
        std::fs::write(tmp.path().join("README.md"), "# My Project\nA cool project.").unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let session = BrainstormSession::new("test").with_project_root(tmp.path());
        let context = session.gather_project_context(10000).unwrap();

        assert!(context.contains("README.md"));
        assert!(context.contains("My Project"));
        assert!(context.contains("Cargo.toml"));
        assert!(context.contains("Directory Structure"));
    }

    #[test]
    fn test_gather_project_context_no_root() {
        let session = BrainstormSession::new("test");
        assert!(session.gather_project_context(10000).is_err());
    }

    #[test]
    fn test_gather_project_context_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("README.md"),
            "x".repeat(5000),
        )
        .unwrap();

        let session = BrainstormSession::new("test").with_project_root(tmp.path());
        let context = session.gather_project_context(100).unwrap();

        // Should be truncated to fit within budget
        assert!(context.len() < 200);
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Build a Cache Layer!"), "build-a-cache-layer");
        assert_eq!(slugify("foo_bar baz"), "foo-bar-baz");
        assert_eq!(slugify("  spaces  "), "spaces");
        assert_eq!(slugify("API v2.0"), "api-v20");
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", Phase::Context), "Context");
        assert_eq!(format!("{}", Phase::Questions), "Questions");
        assert_eq!(format!("{}", Phase::Approaches), "Approaches");
        assert_eq!(format!("{}", Phase::Design), "Design");
        assert_eq!(format!("{}", Phase::Document), "Document");
        assert_eq!(format!("{}", Phase::Done), "Done");
    }

    #[test]
    fn test_complexity_display() {
        assert_eq!(format!("{}", Complexity::Low), "low");
        assert_eq!(format!("{}", Complexity::Medium), "medium");
        assert_eq!(format!("{}", Complexity::High), "high");
    }

    #[test]
    fn test_file_action_display() {
        assert_eq!(format!("{}", FileAction::Create), "create");
        assert_eq!(format!("{}", FileAction::Modify), "modify");
        assert_eq!(format!("{}", FileAction::Delete), "delete");
    }

    #[test]
    fn test_brainstorm_skill_prompt() {
        let prompt = brainstorm_skill_prompt();
        assert!(prompt.contains("Brainstorming Skill"));
        assert!(prompt.contains("Phase 1: Context"));
        assert!(prompt.contains("Phase 2: Clarifying Questions"));
        assert!(prompt.contains("Phase 3: Approaches"));
        assert!(prompt.contains("Phase 4: Design"));
        assert!(prompt.contains("Phase 5: Document"));
    }

    #[test]
    fn test_design_document_serialization_roundtrip() {
        let doc = DesignDocument {
            title: "Test".to_string(),
            summary: "A test design".to_string(),
            problem_statement: "Need to test serialization".to_string(),
            goals: vec!["Goal 1".to_string()],
            non_goals: vec!["Non-goal 1".to_string()],
            project_context: Some("Context".to_string()),
            questions: vec![ClarifyingQuestion {
                question: "Q?".to_string(),
                rationale: "R".to_string(),
                category: "scope".to_string(),
                answer: Some("A".to_string()),
            }],
            approaches: vec![sample_approach("X")],
            chosen_approach: 0,
            choice_rationale: "Simplest".to_string(),
            architecture: "Flat".to_string(),
            components: vec![sample_component("C")],
            data_flow: "A -> B".to_string(),
            file_plan: vec![FileEntry {
                path: "a.rs".to_string(),
                action: FileAction::Create,
                description: "desc".to_string(),
            }],
            open_risks: vec!["Risk 1".to_string()],
            author: Some("test".to_string()),
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string_pretty(&doc).unwrap();
        let parsed: DesignDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, doc.title);
        assert_eq!(parsed.goals, doc.goals);
        assert_eq!(parsed.components.len(), 1);
        assert_eq!(parsed.file_plan.len(), 1);
    }

    #[test]
    fn test_session_serialization_roundtrip() {
        let mut session = BrainstormSession::new("Test Session");
        session.add_question("Q?", "R", "scope");
        session.add_approach(sample_approach("A"));
        session.set_phase(Phase::Approaches);

        let json = serde_json::to_string(&session).unwrap();
        let parsed: BrainstormSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.topic, session.topic);
        assert_eq!(parsed.phase, Phase::Approaches);
        assert_eq!(parsed.questions.len(), 1);
        assert_eq!(parsed.approaches.len(), 1);
    }

    #[test]
    fn test_render_markdown_with_chosen_marker() {
        let mut session = BrainstormSession::new("test");
        session.add_approach(sample_approach("Alpha"));
        session.add_approach(sample_approach("Beta"));
        session.add_approach(sample_approach("Gamma"));
        session
            .finalize_design(1, "Picked Beta", "arch", vec![], "flow", vec![], vec![])
            .unwrap();

        let md = session.render_markdown().unwrap();
        // Alpha should NOT be marked as chosen
        let alpha_section = md.split("### Alpha").nth(1).unwrap();
        assert!(!alpha_section.contains("(chosen)"));
        // Beta SHOULD be marked as chosen
        let beta_section = md.split("### Beta").nth(1).unwrap();
        assert!(beta_section.contains("(chosen)"));
        // Gamma should NOT be marked as chosen
        let gamma_section = md.split("### Gamma").nth(1).unwrap();
        assert!(!gamma_section.contains("(chosen)"));
    }

    #[test]
    fn test_empty_design_render() {
        let mut session = BrainstormSession::new("Empty Design");
        session.add_approach(sample_approach("Only Option"));
        session
            .finalize_design(0, "Only one option", "Simple", vec![], "N/A", vec![], vec![])
            .unwrap();

        let md = session.render_markdown().unwrap();
        assert!(md.contains("# Empty Design"));
        // Should not crash even with empty components, files, risks
        assert!(!md.contains("## Components"));
        assert!(!md.contains("## File Plan"));
        assert!(!md.contains("## Open Risks"));
    }

    #[test]
    fn test_extract_goals_from_questions() {
        let mut session = BrainstormSession::new("test");
        session.add_question("Goal?", "Why", "goals");
        session.add_question("Req?", "Why", "requirements");
        session.add_question("Scope?", "Why", "scope");
        session.add_question("OOS?", "Why", "non-goals");
        session.answer_question(0, "G1").unwrap();
        session.answer_question(1, "R1").unwrap();
        session.answer_question(2, "S1").unwrap();
        session.answer_question(3, "NG1").unwrap();

        let goals = session.extract_goals();
        assert_eq!(goals, vec!["G1", "R1", "S1"]);

        let non_goals = session.extract_non_goals();
        assert_eq!(non_goals, vec!["NG1"]);
    }
}
