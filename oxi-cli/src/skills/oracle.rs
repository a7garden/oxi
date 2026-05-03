//! Oracle skill for oxi
//!
//! High-context decision oracle that provides authoritative answers when
//! the agent encounters uncertainty during implementation. The oracle:
//!
//! 1. **Loads context** — reads the current state, pending question, and
//!    relevant codebase context
//! 2. **Identifies the decision frame** — what exactly needs to be decided,
//!    what are the constraints, what are the options
//! 3. **Evaluates options** — weighs trade-offs against project-specific
//!    criteria (simplicity, performance, maintainability, etc.)
//! 4. **Produces a ruling** — a clear, justified decision with rationale
//! 5. **Documents** — records the decision for future reference
//!
//! The module provides:
//! - [`OracleSession`] — state machine for the decision process
//! - [`Decision`] — a structured decision with options, trade-offs, and ruling
//! - [`DecisionRecord`] — an ADR-like document persisted to disk
//! - [`OracleSkill`] — skill prompt generator

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Phase ──────────────────────────────────────────────────────────────

/// The phase an oracle session is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OraclePhase {
    /// Loading relevant context from the codebase.
    LoadContext,
    /// Identifying the decision frame (what, constraints, options).
    Frame,
    /// Evaluating options against project criteria.
    Evaluate,
    /// Producing the ruling with rationale.
    Rule,
    /// Documenting the decision.
    Document,
    /// Decision complete.
    Done,
}

impl Default for OraclePhase {
    fn default() -> Self {
        OraclePhase::LoadContext
    }
}

impl fmt::Display for OraclePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OraclePhase::LoadContext => write!(f, "Load Context"),
            OraclePhase::Frame => write!(f, "Frame"),
            OraclePhase::Evaluate => write!(f, "Evaluate"),
            OraclePhase::Rule => write!(f, "Rule"),
            OraclePhase::Document => write!(f, "Document"),
            OraclePhase::Done => write!(f, "Done"),
        }
    }
}

// ── Decision types ─────────────────────────────────────────────────────

/// A decision criterion (e.g., "simplicity", "performance", "maintainability").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    /// Criterion name.
    pub name: String,
    /// Description of what this criterion values.
    pub description: String,
    /// Weight of this criterion (higher = more important).
    pub weight: u8,
}

/// An option being considered for the decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionOption {
    /// Short name for this option.
    pub name: String,
    /// Detailed description.
    pub description: String,
    /// Pros of this option.
    pub pros: Vec<String>,
    /// Cons of this option.
    pub cons: Vec<String>,
    /// Scores against each criterion (criterion name → score 1-5).
    pub scores: Vec<(String, u8)>,
    /// Weighted total score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_score: Option<f64>,
}

impl DecisionOption {
    /// Calculate the weighted total score.
    pub fn calculate_score(&mut self, criteria: &[Criterion]) -> f64 {
        let mut total = 0.0;
        let mut max_possible = 0.0;

        for criterion in criteria {
            let score = self
                .scores
                .iter()
                .find(|(name, _)| name == &criterion.name)
                .map(|(_, s)| *s as f64)
                .unwrap_or(0.0);

            total += score * criterion.weight as f64;
            max_possible += 5.0 * criterion.weight as f64;
        }

        let normalized = if max_possible > 0.0 {
            (total / max_possible) * 100.0
        } else {
            0.0
        };

        self.total_score = Some(normalized);
        normalized
    }
}

/// The ruling — the oracle's final decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ruling {
    /// The chosen option.
    pub chosen: String,
    /// Why this option was chosen.
    pub rationale: String,
    /// Key trade-offs accepted.
    pub trade_offs: Vec<String>,
    /// What conditions would change this decision.
    #[serde(default)]
    pub reversibility_conditions: Vec<String>,
    /// Confidence level in this decision.
    pub confidence: Confidence,
}

/// Confidence level of a ruling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Need more information — decision is tentative.
    Low,
    /// Reasonably confident but some uncertainty remains.
    Medium,
    /// Very confident — clear best option.
    High,
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Confidence::Low => write!(f, "low"),
            Confidence::Medium => write!(f, "medium"),
            Confidence::High => write!(f, "high"),
        }
    }
}

// ── Decision context ───────────────────────────────────────────────────

/// The context loaded for making the decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionContext {
    /// What question or decision is needed.
    pub question: String,
    /// Relevant context from the codebase.
    pub codebase_context: String,
    /// Constraints on the decision.
    pub constraints: Vec<String>,
    /// What will be affected by this decision.
    pub impact_areas: Vec<String>,
    /// Related past decisions (ADR references).
    #[serde(default)]
    pub related_decisions: Vec<String>,
}

// ── Decision record ────────────────────────────────────────────────────

/// An Architecture Decision Record (ADR) produced by the oracle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// ADR number (auto-incremented within the project).
    pub number: u32,
    /// Title of the decision.
    pub title: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Status of the decision.
    pub status: DecisionStatus,
    /// The decision context.
    pub context: DecisionContext,
    /// Criteria used for evaluation.
    pub criteria: Vec<Criterion>,
    /// Options considered.
    pub options: Vec<DecisionOption>,
    /// The ruling.
    pub ruling: Ruling,
}

/// Status of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// Proposed but not yet accepted.
    Proposed,
    /// Accepted and in effect.
    Accepted,
    /// Superseded by a later decision.
    Superseded,
    /// Was accepted but later reversed.
    Deprecated,
}

impl fmt::Display for DecisionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecisionStatus::Proposed => write!(f, "Proposed"),
            DecisionStatus::Accepted => write!(f, "Accepted"),
            DecisionStatus::Superseded => write!(f, "Superseded"),
            DecisionStatus::Deprecated => write!(f, "Deprecated"),
        }
    }
}

impl DecisionRecord {
    /// Render as Markdown (ADR format).
    pub fn render_markdown(&self) -> String {
        let mut md = String::with_capacity(4096);

        // Header
        md.push_str(&format!("# ADR-{:04}: {}\n\n", self.number, self.title));
        md.push_str(&format!(
            "> Date: {} | Status: {} | Confidence: {}\n\n",
            &self.created_at[..10],
            self.status,
            self.ruling.confidence,
        ));

        // Context
        md.push_str("## Context\n\n");
        md.push_str(&self.context.question);
        md.push_str("\n\n");

        if !self.context.constraints.is_empty() {
            md.push_str("**Constraints:**\n");
            for c in &self.context.constraints {
                md.push_str(&format!("- {}\n", c));
            }
            md.push('\n');
        }

        if !self.context.impact_areas.is_empty() {
            md.push_str("**Impact Areas:**\n");
            for area in &self.context.impact_areas {
                md.push_str(&format!("- {}\n", area));
            }
            md.push('\n');
        }

        if !self.context.codebase_context.is_empty() {
            md.push_str("### Codebase Context\n\n");
            md.push_str(&self.context.codebase_context);
            md.push_str("\n\n");
        }

        // Decision criteria
        if !self.criteria.is_empty() {
            md.push_str("## Decision Criteria\n\n");
            md.push_str("| Criterion | Description | Weight |\n");
            md.push_str("|-----------|-------------|--------|\n");
            for criterion in &self.criteria {
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    criterion.name, criterion.description, criterion.weight
                ));
            }
            md.push('\n');
        }

        // Options
        if !self.options.is_empty() {
            md.push_str("## Options Considered\n\n");

            // Comparison table
            md.push_str("| Option | ");
            for criterion in &self.criteria {
                md.push_str(&format!("{} | ", criterion.name));
            }
            md.push_str("Score |\n");

            md.push_str("|--------|");
            for _ in &self.criteria {
                md.push_str("---|");
            }
            md.push_str("------|\n");

            for option in &self.options {
                md.push_str(&format!("| {} | ", option.name));
                for criterion in &self.criteria {
                    let score = option
                        .scores
                        .iter()
                        .find(|(name, _)| name == &criterion.name)
                        .map(|(_, s)| s.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    md.push_str(&format!("{} | ", score));
                }
                let score_str = option
                    .total_score
                    .map(|s| format!("{:.0}/100", s))
                    .unwrap_or_else(|| "-".to_string());
                md.push_str(&format!("{} |\n", score_str));
            }
            md.push('\n');

            // Detailed breakdowns
            for option in &self.options {
                let chosen_marker = if option.name == self.ruling.chosen {
                    " ✅ **(chosen)**"
                } else {
                    ""
                };
                md.push_str(&format!("### {}{}\n\n", option.name, chosen_marker));
                md.push_str(&option.description);
                md.push_str("\n\n");

                if !option.pros.is_empty() {
                    md.push_str("**Pros:**\n");
                    for pro in &option.pros {
                        md.push_str(&format!("+ {}\n", pro));
                    }
                    md.push('\n');
                }

                if !option.cons.is_empty() {
                    md.push_str("**Cons:**\n");
                    for con in &option.cons {
                        md.push_str(&format!("- {}\n", con));
                    }
                    md.push('\n');
                }
            }
        }

        // Decision
        md.push_str("## Decision\n\n");
        md.push_str(&format!("**{}**\n\n", self.ruling.chosen));
        md.push_str(&self.ruling.rationale);
        md.push_str("\n\n");

        if !self.ruling.trade_offs.is_empty() {
            md.push_str("### Trade-offs Accepted\n\n");
            for trade_off in &self.ruling.trade_offs {
                md.push_str(&format!("- {}\n", trade_off));
            }
            md.push('\n');
        }

        if !self.ruling.reversibility_conditions.is_empty() {
            md.push_str("### Reversibility Conditions\n\n");
            md.push_str("This decision should be revisited if:\n\n");
            for condition in &self.ruling.reversibility_conditions {
                md.push_str(&format!("- {}\n", condition));
            }
            md.push('\n');
        }

        // Related decisions
        if !self.context.related_decisions.is_empty() {
            md.push_str("## Related Decisions\n\n");
            for related in &self.context.related_decisions {
                md.push_str(&format!("- {}\n", related));
            }
            md.push('\n');
        }

        md
    }

    /// Write the ADR to a file.
    pub fn write_to_file(&self, dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;

        let slug = slugify(&self.title);
        let filename = format!("ADR-{:04}-{}.md", self.number, slug);
        let path = dir.join(&filename);

        let content = self.render_markdown();
        fs::write(&path, &content)
            .with_context(|| format!("Failed to write ADR to {}", path.display()))?;

        Ok(path)
    }

    /// Get the next ADR number by scanning existing ADR files in a directory.
    pub fn next_number(dir: &Path) -> u32 {
        if !dir.exists() {
            return 1;
        }

        let mut max: u32 = 0;
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(rest) = name.strip_prefix("ADR-") {
                    if let Some(num_str) = rest.split('-').next() {
                        if let Ok(num) = num_str.parse::<u32>() {
                            max = max.max(num);
                        }
                    }
                }
            }
        }

        max + 1
    }
}

// ── Oracle session ─────────────────────────────────────────────────────

/// An oracle session that tracks the decision-making lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleSession {
    /// Current phase.
    pub phase: OraclePhase,
    /// The question being decided.
    pub question: String,
    /// Optional project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// Loaded context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<DecisionContext>,
    /// Evaluation criteria.
    pub criteria: Vec<Criterion>,
    /// Options being considered.
    pub options: Vec<DecisionOption>,
    /// The ruling (set in Rule phase).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruling: Option<Ruling>,
    /// The final ADR (set in Document phase).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<DecisionRecord>,
}

impl OracleSession {
    /// Create a new oracle session.
    pub fn new(question: impl Into<String>) -> Self {
        Self {
            phase: OraclePhase::LoadContext,
            question: question.into(),
            project_root: None,
            context: None,
            criteria: Vec::new(),
            options: Vec::new(),
            ruling: None,
            record: None,
        }
    }

    /// Set the project root.
    pub fn with_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    /// Advance to the next phase.
    pub fn advance(&mut self) -> Result<()> {
        let next = match self.phase {
            OraclePhase::LoadContext => OraclePhase::Frame,
            OraclePhase::Frame => OraclePhase::Evaluate,
            OraclePhase::Evaluate => OraclePhase::Rule,
            OraclePhase::Rule => OraclePhase::Document,
            OraclePhase::Document => OraclePhase::Done,
            OraclePhase::Done => bail!("Cannot advance past Done"),
        };
        self.phase = next;
        Ok(())
    }

    /// Set phase directly.
    pub fn set_phase(&mut self, phase: OraclePhase) {
        self.phase = phase;
    }

    /// Set the decision context.
    pub fn set_context(&mut self, ctx: DecisionContext) {
        self.context = Some(ctx);
    }

    /// Add a criterion.
    pub fn add_criterion(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        weight: u8,
    ) {
        self.criteria.push(Criterion {
            name: name.into(),
            description: description.into(),
            weight,
        });
    }

    /// Add an option.
    pub fn add_option(&mut self, option: DecisionOption) {
        self.options.push(option);
    }

    /// Number of options.
    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    /// Score all options against the criteria.
    pub fn score_options(&mut self) {
        let criteria = self.criteria.clone();
        for option in &mut self.options {
            option.calculate_score(&criteria);
        }
    }

    /// Set the ruling.
    pub fn set_ruling(&mut self, ruling: Ruling) {
        self.ruling = Some(ruling);
    }

    /// Finalize the decision record.
    pub fn finalize(&mut self, status: DecisionStatus) -> Result<()> {
        let ctx = self.context.clone().context("Decision context not set")?;
        let ruling = self
            .ruling
            .clone()
            .context("Ruling not set — call set_ruling() first")?;

        // Determine ADR number
        let number = if let Some(ref root) = self.project_root {
            let adr_dir = root.join("docs").join("decisions");
            DecisionRecord::next_number(&adr_dir)
        } else {
            1
        };

        let record = DecisionRecord {
            number,
            title: self.question.clone(),
            created_at: Utc::now().to_rfc3339(),
            status,
            context: ctx,
            criteria: self.criteria.clone(),
            options: self.options.clone(),
            ruling,
        };

        self.record = Some(record);
        Ok(())
    }

    /// Write the ADR to disk.
    pub fn write_record(&self, explicit_path: Option<&Path>) -> Result<PathBuf> {
        let record = self
            .record
            .as_ref()
            .context("Record not finalized — call finalize() first")?;

        if let Some(path) = explicit_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            let content = record.render_markdown();
            fs::write(path, &content)
                .with_context(|| format!("Failed to write ADR to {}", path.display()))?;
            Ok(path.to_path_buf())
        } else {
            let root = self
                .project_root
                .as_deref()
                .context("No project root and no explicit path")?;
            let adr_dir = root.join("docs").join("decisions");
            record.write_to_file(&adr_dir)
        }
    }
}

// ── Skill prompt ───────────────────────────────────────────────────────

/// The oracle skill struct.
pub struct OracleSkill;

impl OracleSkill {
    /// Create a new oracle skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Generate the system-prompt fragment for the oracle skill.
    pub fn skill_prompt() -> String {
        r#"# Oracle Skill

You are running the **oracle** skill. You are the high-context decision
maker, called when the implementing agent encounters uncertainty. You make
clear, justified decisions quickly.

## Workflow

### Phase 1: Load Context

1. Understand the question being asked.
2. Read relevant code files to understand the current state.
3. Identify what's already decided and what's genuinely uncertain.
4. Don't over-gather — only read what's directly relevant.

### Phase 2: Frame the Decision

1. State the decision clearly in one sentence.
2. List the constraints (what's NOT optional).
3. List the options (usually 2–4 viable approaches).
4. Identify who/what this decision affects.

### Phase 3: Evaluate Options

1. Define criteria weighted by project priorities:
   - **Simplicity** — fewer moving parts, easier to understand
   - **Correctness** — handles edge cases, doesn't introduce bugs
   - **Performance** — meets performance requirements
   - **Maintainability** — easy to change later
   - **Consistency** — follows existing patterns in the codebase

2. Score each option against each criterion (1–5).
3. Calculate weighted scores.
4. Don't over-optimize — a 5% score difference is noise.

### Phase 4: Rule

1. State the decision clearly: "We will X."
2. Explain WHY — reference the scores and criteria.
3. List trade-offs being accepted.
4. State what conditions would change this decision.

### Phase 5: Document

1. Write the ADR to `docs/decisions/ADR-NNNN-<slug>.md`.
2. Use the Architecture Decision Record format.

## Rules

- **Decide, don't deliberate.** You exist to break deadlocks, not to explore.
- **Default to simplicity.** When options are close, pick the simpler one.
- **Be specific.** "Use a HashMap" not "use a data structure."
- **Consider reversibility.** Prefer decisions that are easy to reverse.
- **Don't gold-plate.** Solve the problem at hand, not hypothetical future ones.
- **One decision per session.** If there are multiple questions, handle them separately.
- **Be honest about uncertainty.** Low confidence is fine — just say so.
"#
        .to_string()
    }
}

impl Default for OracleSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for OracleSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OracleSkill").finish()
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else if c == ' ' || c == '_' || c == '-' {
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

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sample_option(name: &str) -> DecisionOption {
        DecisionOption {
            name: name.to_string(),
            description: format!("{} approach", name),
            pros: vec!["Simple".to_string()],
            cons: vec!["Less flexible".to_string()],
            scores: vec![
                ("Simplicity".to_string(), 4),
                ("Performance".to_string(), 3),
            ],
            total_score: None,
        }
    }

    fn sample_criteria() -> Vec<Criterion> {
        vec![
            Criterion {
                name: "Simplicity".to_string(),
                description: "Fewer moving parts".to_string(),
                weight: 3,
            },
            Criterion {
                name: "Performance".to_string(),
                description: "Meets perf requirements".to_string(),
                weight: 2,
            },
        ]
    }

    #[test]
    fn test_session_new() {
        let session = OracleSession::new("Which DB to use?");
        assert_eq!(session.phase, OraclePhase::LoadContext);
        assert_eq!(session.question, "Which DB to use?");
        assert!(session.criteria.is_empty());
        assert!(session.options.is_empty());
    }

    #[test]
    fn test_phase_advance() {
        let mut session = OracleSession::new("test");
        assert_eq!(session.phase, OraclePhase::LoadContext);

        session.advance().unwrap();
        assert_eq!(session.phase, OraclePhase::Frame);

        session.advance().unwrap();
        assert_eq!(session.phase, OraclePhase::Evaluate);

        session.advance().unwrap();
        assert_eq!(session.phase, OraclePhase::Rule);

        session.advance().unwrap();
        assert_eq!(session.phase, OraclePhase::Document);

        session.advance().unwrap();
        assert_eq!(session.phase, OraclePhase::Done);

        assert!(session.advance().is_err());
    }

    #[test]
    fn test_set_phase() {
        let mut session = OracleSession::new("test");
        session.set_phase(OraclePhase::Evaluate);
        assert_eq!(session.phase, OraclePhase::Evaluate);
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", OraclePhase::LoadContext), "Load Context");
        assert_eq!(format!("{}", OraclePhase::Frame), "Frame");
        assert_eq!(format!("{}", OraclePhase::Evaluate), "Evaluate");
        assert_eq!(format!("{}", OraclePhase::Rule), "Rule");
        assert_eq!(format!("{}", OraclePhase::Document), "Document");
        assert_eq!(format!("{}", OraclePhase::Done), "Done");
    }

    #[test]
    fn test_confidence_display() {
        assert_eq!(format!("{}", Confidence::Low), "low");
        assert_eq!(format!("{}", Confidence::Medium), "medium");
        assert_eq!(format!("{}", Confidence::High), "high");
    }

    #[test]
    fn test_decision_status_display() {
        assert_eq!(format!("{}", DecisionStatus::Proposed), "Proposed");
        assert_eq!(format!("{}", DecisionStatus::Accepted), "Accepted");
        assert_eq!(format!("{}", DecisionStatus::Superseded), "Superseded");
        assert_eq!(format!("{}", DecisionStatus::Deprecated), "Deprecated");
    }

    #[test]
    fn test_add_criteria() {
        let mut session = OracleSession::new("test");
        session.add_criterion("Simplicity", "Fewer moving parts", 3);
        session.add_criterion("Performance", "Fast enough", 2);

        assert_eq!(session.criteria.len(), 2);
        assert_eq!(session.criteria[0].name, "Simplicity");
        assert_eq!(session.criteria[0].weight, 3);
    }

    #[test]
    fn test_add_options() {
        let mut session = OracleSession::new("test");
        session.add_option(sample_option("HashMap"));
        session.add_option(sample_option("BTreeMap"));

        assert_eq!(session.option_count(), 2);
    }

    #[test]
    fn test_score_options() {
        let mut session = OracleSession::new("test");
        session.criteria = sample_criteria();

        let mut opt1 = sample_option("HashMap");
        opt1.scores = vec![
            ("Simplicity".to_string(), 5),
            ("Performance".to_string(), 4),
        ];

        let mut opt2 = sample_option("BTreeMap");
        opt2.scores = vec![
            ("Simplicity".to_string(), 3),
            ("Performance".to_string(), 5),
        ];

        session.add_option(opt1);
        session.add_option(opt2);
        session.score_options();

        // HashMap: (5*3 + 4*2) / (5*3 + 5*2) * 100 = 23/25 * 100 = 92
        assert_eq!(session.options[0].total_score, Some(92.0));
        // BTreeMap: (3*3 + 5*2) / 25 * 100 = 19/25 * 100 = 76
        assert_eq!(session.options[1].total_score, Some(76.0));
    }

    #[test]
    fn test_score_options_empty_criteria() {
        let mut session = OracleSession::new("test");
        session.add_option(sample_option("A"));
        session.score_options();
        assert_eq!(session.options[0].total_score, Some(0.0));
    }

    #[test]
    fn test_option_calculate_score() {
        let criteria = sample_criteria();
        let mut opt = DecisionOption {
            name: "Test".to_string(),
            description: "Test".to_string(),
            pros: vec![],
            cons: vec![],
            scores: vec![
                ("Simplicity".to_string(), 5),
                ("Performance".to_string(), 5),
            ],
            total_score: None,
        };

        let score = opt.calculate_score(&criteria);
        // Perfect score: (5*3 + 5*2) / (5*3 + 5*2) = 100
        assert_eq!(score, 100.0);
        assert_eq!(opt.total_score, Some(100.0));
    }

    #[test]
    fn test_set_ruling() {
        let mut session = OracleSession::new("test");
        session.set_ruling(Ruling {
            chosen: "HashMap".to_string(),
            rationale: "Simpler and fast enough".to_string(),
            trade_offs: vec!["No ordering".to_string()],
            reversibility_conditions: vec!["If we need ordered iteration".to_string()],
            confidence: Confidence::High,
        });

        assert!(session.ruling.is_some());
        assert_eq!(session.ruling.as_ref().unwrap().chosen, "HashMap");
        assert_eq!(
            session.ruling.as_ref().unwrap().confidence,
            Confidence::High
        );
    }

    #[test]
    fn test_finalize_no_context() {
        let mut session = OracleSession::new("test");
        assert!(session.finalize(DecisionStatus::Accepted).is_err());
    }

    #[test]
    fn test_finalize_no_ruling() {
        let mut session = OracleSession::new("test");
        session.set_context(DecisionContext {
            question: "test".to_string(),
            codebase_context: String::new(),
            constraints: vec![],
            impact_areas: vec![],
            related_decisions: vec![],
        });
        assert!(session.finalize(DecisionStatus::Accepted).is_err());
    }

    #[test]
    fn test_finalize_and_write() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session =
            OracleSession::new("Which data structure for cache?").with_project_root(tmp.path());

        session.set_context(DecisionContext {
            question: "Which data structure for cache?".to_string(),
            codebase_context: "Single-process CLI tool".to_string(),
            constraints: vec!["Must be fast".to_string()],
            impact_areas: vec!["src/cache.rs".to_string()],
            related_decisions: vec![],
        });

        session.criteria = sample_criteria();
        session.add_option(sample_option("HashMap"));
        session.add_option(sample_option("BTreeMap"));
        session.score_options();

        session.set_ruling(Ruling {
            chosen: "HashMap".to_string(),
            rationale: "Simpler and O(1) lookup".to_string(),
            trade_offs: vec!["No ordering guarantees".to_string()],
            reversibility_conditions: vec!["If ordered iteration needed".to_string()],
            confidence: Confidence::High,
        });

        session.finalize(DecisionStatus::Accepted).unwrap();

        let record = session.record.as_ref().unwrap();
        assert_eq!(record.number, 1);
        assert_eq!(record.status, DecisionStatus::Accepted);
        assert_eq!(record.ruling.chosen, "HashMap");

        // Write to file
        let path = session.write_record(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("docs/decisions"));
        assert!(path.to_string_lossy().contains("ADR-0001"));

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# ADR-0001"));
        assert!(content.contains("HashMap"));
        assert!(content.contains("Accepted"));
    }

    #[test]
    fn test_write_record_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = OracleSession::new("test");
        session.set_context(DecisionContext {
            question: "test".to_string(),
            codebase_context: String::new(),
            constraints: vec![],
            impact_areas: vec![],
            related_decisions: vec![],
        });
        session.set_ruling(Ruling {
            chosen: "A".to_string(),
            rationale: "Best".to_string(),
            trade_offs: vec![],
            reversibility_conditions: vec![],
            confidence: Confidence::Medium,
        });
        session.finalize(DecisionStatus::Proposed).unwrap();

        let explicit = tmp.path().join("decision.md");
        let path = session.write_record(Some(&explicit)).unwrap();
        assert_eq!(path, explicit);
        assert!(path.exists());
    }

    #[test]
    fn test_next_number_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(DecisionRecord::next_number(tmp.path()), 1);
    }

    #[test]
    fn test_next_number_with_existing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("ADR-0001-test.md"), "").unwrap();
        fs::write(tmp.path().join("ADR-0003-test.md"), "").unwrap();

        assert_eq!(DecisionRecord::next_number(tmp.path()), 4);
    }

    #[test]
    fn test_next_number_nonexistent_dir() {
        assert_eq!(DecisionRecord::next_number(Path::new("/nonexistent")), 1);
    }

    #[test]
    fn test_render_markdown() {
        let record = DecisionRecord {
            number: 7,
            title: "Use SQLite for local storage".to_string(),
            created_at: "2025-06-15T10:30:00Z".to_string(),
            status: DecisionStatus::Accepted,
            context: DecisionContext {
                question: "Which embedded DB?".to_string(),
                codebase_context: "Desktop app, local data".to_string(),
                constraints: vec!["Single-file DB".to_string()],
                impact_areas: vec!["src/storage.rs".to_string()],
                related_decisions: vec!["ADR-0003".to_string()],
            },
            criteria: sample_criteria(),
            options: {
                let mut opt = sample_option("SQLite");
                opt.total_score = Some(90.0);
                vec![opt]
            },
            ruling: Ruling {
                chosen: "SQLite".to_string(),
                rationale: "Battle-tested, single-file, good perf".to_string(),
                trade_offs: vec!["Write concurrency limited".to_string()],
                reversibility_conditions: vec!["If we need multi-process writes".to_string()],
                confidence: Confidence::High,
            },
        };

        let md = record.render_markdown();
        assert!(md.contains("# ADR-0007: Use SQLite for local storage"));
        assert!(md.contains("Status: Accepted"));
        assert!(md.contains("Confidence: high"));
        assert!(md.contains("## Context"));
        assert!(md.contains("Which embedded DB?"));
        assert!(md.contains("## Decision Criteria"));
        assert!(md.contains("| Simplicity |"));
        assert!(md.contains("## Options Considered"));
        assert!(md.contains("SQLite ✅ **(chosen)**"));
        assert!(md.contains("## Decision"));
        assert!(md.contains("Battle-tested"));
        assert!(md.contains("### Trade-offs Accepted"));
        assert!(md.contains("Write concurrency limited"));
        assert!(md.contains("### Reversibility Conditions"));
        assert!(md.contains("multi-process writes"));
        assert!(md.contains("## Related Decisions"));
        assert!(md.contains("ADR-0003"));
    }

    #[test]
    fn test_session_serialization_roundtrip() {
        let mut session = OracleSession::new("Which cache strategy?");
        session.add_criterion("Simplicity", "Few parts", 3);
        session.add_option(sample_option("LRU"));
        session.set_phase(OraclePhase::Evaluate);

        let json = serde_json::to_string(&session).unwrap();
        let parsed: OracleSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.question, "Which cache strategy?");
        assert_eq!(parsed.phase, OraclePhase::Evaluate);
        assert_eq!(parsed.criteria.len(), 1);
        assert_eq!(parsed.option_count(), 1);
    }

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = OracleSkill::skill_prompt();
        assert!(prompt.contains("Oracle Skill"));
        assert!(prompt.contains("Phase 1: Load Context"));
        assert!(prompt.contains("Phase 4: Rule"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Use SQLite for storage"), "use-sqlite-for-storage");
        assert_eq!(slugify("hello_world"), "hello-world");
    }

    #[test]
    fn test_full_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();

        // Create existing ADR to test numbering
        let adr_dir = tmp.path().join("docs").join("decisions");
        fs::create_dir_all(&adr_dir).unwrap();
        fs::write(adr_dir.join("ADR-0001-test.md"), "").unwrap();

        let mut session = OracleSession::new("REST vs gRPC?").with_project_root(tmp.path());

        // Phase 1: LoadContext
        session.set_context(DecisionContext {
            question: "REST vs gRPC for internal service communication?".to_string(),
            codebase_context: "Microservices, Rust backend".to_string(),
            constraints: vec!["Must support streaming".to_string()],
            impact_areas: vec!["src/api/".to_string()],
            related_decisions: vec!["ADR-0001".to_string()],
        });
        session.advance().unwrap();

        // Phase 2: Frame
        session.add_criterion("Simplicity", "Easy to debug", 3);
        session.add_criterion("Performance", "Low latency", 2);
        session.add_criterion("Ecosystem", "Tool support", 2);

        let mut rest = DecisionOption {
            name: "REST".to_string(),
            description: "HTTP+JSON REST API".to_string(),
            pros: vec!["Ubiquitous".to_string(), "Easy to debug".to_string()],
            cons: vec!["No native streaming".to_string()],
            scores: vec![
                ("Simplicity".to_string(), 5),
                ("Performance".to_string(), 3),
                ("Ecosystem".to_string(), 5),
            ],
            total_score: None,
        };

        let mut grpc = DecisionOption {
            name: "gRPC".to_string(),
            description: "gRPC with protobuf".to_string(),
            pros: vec!["Native streaming".to_string(), "Codegen".to_string()],
            cons: vec!["Complex setup".to_string(), "Harder to debug".to_string()],
            scores: vec![
                ("Simplicity".to_string(), 2),
                ("Performance".to_string(), 5),
                ("Ecosystem".to_string(), 3),
            ],
            total_score: None,
        };

        session.advance().unwrap(); // Evaluate

        rest.calculate_score(&session.criteria);
        grpc.calculate_score(&session.criteria);
        session.add_option(rest);
        session.add_option(grpc);

        session.advance().unwrap(); // Rule

        session.set_ruling(Ruling {
            chosen: "gRPC".to_string(),
            rationale: "Streaming requirement rules out plain REST".to_string(),
            trade_offs: vec!["More complex tooling".to_string()],
            reversibility_conditions: vec!["If streaming requirement is dropped".to_string()],
            confidence: Confidence::Medium,
        });

        session.advance().unwrap(); // Document

        session.finalize(DecisionStatus::Accepted).unwrap();

        // Should be ADR-0002 since ADR-0001 exists
        assert_eq!(session.record.as_ref().unwrap().number, 2);

        let path = session.write_record(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("ADR-0002"));

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("gRPC"));
        assert!(content.contains("Streaming requirement"));
    }
}
