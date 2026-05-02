//! Context builder skill for oxi
//!
//! Requirements analysis and context generation skill that builds rich
//! context packages for other skills to consume. The context builder:
//!
//! 1. **Ingests requirements** — raw user input, specs, tickets, or conversations
//! 2. **Analyzes** — identifies key entities, constraints, and success criteria
//! 3. **Cross-references** — connects requirements to existing codebase artifacts
//! 4. **Structures** — produces a formal [`RequirementsContext`] document
//! 5. **Validates** — checks for completeness, consistency, and ambiguity
//!
//! The module provides:
//! - [`ContextBuilderSession`] — state machine for the context-building lifecycle
//! - [`RequirementsContext`] — the structured output document
//! - [`Requirement`] / [`Constraint`] / [`Entity`] — structured requirement types
//! - [`ContextReport`] — validation report with completeness checks
//! - [`ContextBuilderSkill`] — skill prompt generator

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Phase ──────────────────────────────────────────────────────────────

/// The phase a context builder session is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPhase {
    /// Ingesting raw requirements.
    Ingest,
    /// Analyzing and structuring requirements.
    Analyze,
    /// Cross-referencing with codebase.
    CrossReference,
    /// Structuring the output document.
    Structure,
    /// Validating completeness and consistency.
    Validate,
    /// Documenting the context.
    Document,
    /// Context building complete.
    Done,
}

impl Default for ContextPhase {
    fn default() -> Self {
        ContextPhase::Ingest
    }
}

impl fmt::Display for ContextPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextPhase::Ingest => write!(f, "Ingest"),
            ContextPhase::Analyze => write!(f, "Analyze"),
            ContextPhase::CrossReference => write!(f, "Cross-Reference"),
            ContextPhase::Structure => write!(f, "Structure"),
            ContextPhase::Validate => write!(f, "Validate"),
            ContextPhase::Document => write!(f, "Document"),
            ContextPhase::Done => write!(f, "Done"),
        }
    }
}

// ── Requirement types ──────────────────────────────────────────────────

/// Priority of a requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Nice to have — can be deferred.
    Low,
    /// Should have — important but not blocking.
    Medium,
    /// Must have — critical for success.
    High,
    /// Non-negotiable — system won't work without it.
    Critical,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Priority::Low => write!(f, "low"),
            Priority::Medium => write!(f, "medium"),
            Priority::High => write!(f, "high"),
            Priority::Critical => write!(f, "critical"),
        }
    }
}

/// A single structured requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    /// Unique identifier (e.g., "REQ-001").
    pub id: String,
    /// Short title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Priority level.
    pub priority: Priority,
    /// Category (e.g., "functional", "non-functional", "security").
    pub category: RequirementCategory,
    /// Acceptance criteria (testable conditions).
    pub acceptance_criteria: Vec<String>,
    /// Source of this requirement (user input, spec doc, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Related codebase files.
    #[serde(default)]
    pub related_files: Vec<String>,
    /// Whether this requirement has been validated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<RequirementValidation>,
}

/// Category of requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementCategory {
    /// Core functionality the system must provide.
    Functional,
    /// Performance, reliability, scalability requirements.
    NonFunctional,
    /// Security and access control requirements.
    Security,
    /// User interface / UX requirements.
    UserExperience,
    /// Integration with external systems.
    Integration,
    /// Data model and persistence requirements.
    Data,
    /// Testing and quality requirements.
    Testing,
    /// Deployment and operations requirements.
    Operations,
}

impl fmt::Display for RequirementCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequirementCategory::Functional => write!(f, "functional"),
            RequirementCategory::NonFunctional => write!(f, "non-functional"),
            RequirementCategory::Security => write!(f, "security"),
            RequirementCategory::UserExperience => write!(f, "ux"),
            RequirementCategory::Integration => write!(f, "integration"),
            RequirementCategory::Data => write!(f, "data"),
            RequirementCategory::Testing => write!(f, "testing"),
            RequirementCategory::Operations => write!(f, "operations"),
        }
    }
}

/// Validation result for a requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementValidation {
    /// Whether the requirement is complete and unambiguous.
    pub is_valid: bool,
    /// Issues found during validation.
    pub issues: Vec<ValidationIssue>,
}

/// A validation issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// What the issue is.
    pub description: String,
    /// Severity of the issue.
    pub severity: ValidationSeverity,
}

/// Severity of a validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    /// Ambiguous or unclear wording.
    Ambiguous,
    /// Missing critical information.
    Incomplete,
    /// Conflicts with another requirement.
    Conflicting,
    /// Not testable or measurable.
    Untestable,
}

impl fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationSeverity::Ambiguous => write!(f, "ambiguous"),
            ValidationSeverity::Incomplete => write!(f, "incomplete"),
            ValidationSeverity::Conflicting => write!(f, "conflicting"),
            ValidationSeverity::Untestable => write!(f, "untestable"),
        }
    }
}

// ── Entity and constraint ──────────────────────────────────────────────

/// A key entity identified from requirements (e.g., "User", "Order", "Cache").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Entity name.
    pub name: String,
    /// Description of the entity.
    pub description: String,
    /// Attributes / properties of the entity.
    pub attributes: Vec<EntityAttribute>,
    /// Relationships to other entities.
    pub relationships: Vec<String>,
    /// Source requirement IDs that mention this entity.
    pub source_requirements: Vec<String>,
}

/// An attribute of an entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAttribute {
    /// Attribute name.
    pub name: String,
    /// Data type (informal, e.g., "string", "integer", "timestamp").
    pub attr_type: String,
    /// Whether this attribute is required.
    pub required: bool,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A constraint identified from requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    /// Short description.
    pub description: String,
    /// Type of constraint.
    pub constraint_type: ConstraintType,
    /// Source requirement ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Type of constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintType {
    /// Technical constraint (language, framework, platform).
    Technical,
    /// Performance constraint (latency, throughput, memory).
    Performance,
    /// Business rule constraint.
    Business,
    /// Compatibility constraint (API version, data format).
    Compatibility,
    /// Regulatory or compliance constraint.
    Regulatory,
}

impl fmt::Display for ConstraintType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstraintType::Technical => write!(f, "technical"),
            ConstraintType::Performance => write!(f, "performance"),
            ConstraintType::Business => write!(f, "business"),
            ConstraintType::Compatibility => write!(f, "compatibility"),
            ConstraintType::Regulatory => write!(f, "regulatory"),
        }
    }
}

// ── Codebase cross-reference ───────────────────────────────────────────

/// A cross-reference between requirements and codebase artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebaseCrossRef {
    /// Files relevant to the requirements.
    pub relevant_files: Vec<FileRelevance>,
    /// Existing patterns that apply.
    pub applicable_patterns: Vec<String>,
    /// Dependencies that need to be added or are already available.
    pub dependencies: Vec<String>,
    /// Potential conflicts with existing code.
    pub conflicts: Vec<String>,
}

/// A file's relevance to the requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRelevance {
    /// File path.
    pub path: String,
    /// How relevant this file is.
    pub relevance: RelevanceLevel,
    /// Why it's relevant.
    pub reason: String,
    /// Which requirements it relates to.
    pub related_requirements: Vec<String>,
}

/// How relevant a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelevanceLevel {
    /// Directly implements or will be modified.
    Direct,
    /// Related context that should be understood.
    Contextual,
    /// May be affected indirectly.
    Indirect,
}

impl fmt::Display for RelevanceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelevanceLevel::Direct => write!(f, "direct"),
            RelevanceLevel::Contextual => write!(f, "contextual"),
            RelevanceLevel::Indirect => write!(f, "indirect"),
        }
    }
}

// ── Requirements context document ──────────────────────────────────────

/// The structured requirements context document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementsContext {
    /// Document title.
    pub title: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Version.
    pub version: u32,
    /// Raw input summary (what was ingested).
    pub input_summary: String,
    /// Structured requirements.
    pub requirements: Vec<Requirement>,
    /// Key entities identified.
    pub entities: Vec<Entity>,
    /// Constraints identified.
    pub constraints: Vec<Constraint>,
    /// Success criteria (what "done" looks like).
    pub success_criteria: Vec<String>,
    /// Assumptions made during analysis.
    pub assumptions: Vec<String>,
    /// Open questions that need resolution.
    pub open_questions: Vec<String>,
    /// Codebase cross-references.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_references: Option<CodebaseCrossRef>,
    /// Validation report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_report: Option<ContextReport>,
}

impl RequirementsContext {
    /// Render as Markdown.
    pub fn render_markdown(&self) -> String {
        let mut md = String::with_capacity(6144);

        md.push_str(&format!("# {}\n\n", self.title));
        md.push_str(&format!(
            "> Created: {} | Version: {}\n\n",
            &self.created_at[..10],
            self.version,
        ));

        // Input summary
        md.push_str("## Input Summary\n\n");
        md.push_str(&self.input_summary);
        md.push_str("\n\n");

        // Success criteria
        if !self.success_criteria.is_empty() {
            md.push_str("## Success Criteria\n\n");
            for (i, criterion) in self.success_criteria.iter().enumerate() {
                md.push_str(&format!("{}. [ ] {}\n", i + 1, criterion));
            }
            md.push('\n');
        }

        // Requirements
        if !self.requirements.is_empty() {
            md.push_str("## Requirements\n\n");

            // Summary table
            md.push_str("| ID | Title | Priority | Category | Valid |\n");
            md.push_str("|----|-------|----------|----------|-------|\n");
            for req in &self.requirements {
                let valid = req
                    .validation
                    .as_ref()
                    .map(|v| if v.is_valid { "✅" } else { "⚠️" })
                    .unwrap_or("—");
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    req.id, req.title, req.priority, req.category, valid
                ));
            }
            md.push('\n');

            // Detailed requirements
            for req in &self.requirements {
                md.push_str(&format!("### {} — {}\n\n", req.id, req.title));
                md.push_str(&req.description);
                md.push_str("\n\n");
                md.push_str(&format!(
                    "**Priority:** {} | **Category:** {}\n\n",
                    req.priority, req.category
                ));

                if !req.acceptance_criteria.is_empty() {
                    md.push_str("**Acceptance Criteria:**\n");
                    for (i, ac) in req.acceptance_criteria.iter().enumerate() {
                        md.push_str(&format!("{}. {}\n", i + 1, ac));
                    }
                    md.push('\n');
                }

                if !req.related_files.is_empty() {
                    md.push_str("**Related Files:**\n");
                    for file in &req.related_files {
                        md.push_str(&format!("- `{}`\n", file));
                    }
                    md.push('\n');
                }

                if let Some(ref validation) = req.validation {
                    if !validation.issues.is_empty() {
                        md.push_str("**Validation Issues:**\n");
                        for issue in &validation.issues {
                            md.push_str(&format!(
                                "- [{}] {}\n",
                                issue.severity, issue.description
                            ));
                        }
                        md.push('\n');
                    }
                }
            }
        }

        // Entities
        if !self.entities.is_empty() {
            md.push_str("## Key Entities\n\n");
            for entity in &self.entities {
                md.push_str(&format!("### {}\n\n", entity.name));
                md.push_str(&entity.description);
                md.push_str("\n\n");

                if !entity.attributes.is_empty() {
                    md.push_str("| Attribute | Type | Required | Description |\n");
                    md.push_str("|-----------|------|----------|-------------|\n");
                    for attr in &entity.attributes {
                        let desc = attr.description.as_deref().unwrap_or("—");
                        md.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            attr.name,
                            attr.attr_type,
                            if attr.required { "yes" } else { "no" },
                            desc,
                        ));
                    }
                    md.push('\n');
                }

                if !entity.relationships.is_empty() {
                    md.push_str(&format!(
                        "**Relationships:** {}\n\n",
                        entity.relationships.join(", ")
                    ));
                }
            }
        }

        // Constraints
        if !self.constraints.is_empty() {
            md.push_str("## Constraints\n\n");
            md.push_str("| Constraint | Type | Source |\n");
            md.push_str("|-----------|------|--------|\n");
            for constraint in &self.constraints {
                let source = constraint.source.as_deref().unwrap_or("—");
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    constraint.description, constraint.constraint_type, source
                ));
            }
            md.push('\n');
        }

        // Cross-references
        if let Some(ref xref) = self.cross_references {
            md.push_str("## Codebase Cross-References\n\n");

            if !xref.relevant_files.is_empty() {
                md.push_str("### Relevant Files\n\n");
                md.push_str("| File | Relevance | Reason | Requirements |\n");
                md.push_str("|------|-----------|--------|-------------|\n");
                for file in &xref.relevant_files {
                    md.push_str(&format!(
                        "| `{}` | {} | {} | {} |\n",
                        file.path,
                        file.relevance,
                        file.reason,
                        file.related_requirements.join(", "),
                    ));
                }
                md.push('\n');
            }

            if !xref.applicable_patterns.is_empty() {
                md.push_str("**Applicable Patterns:**\n");
                for pattern in &xref.applicable_patterns {
                    md.push_str(&format!("- {}\n", pattern));
                }
                md.push('\n');
            }

            if !xref.conflicts.is_empty() {
                md.push_str("**Potential Conflicts:**\n");
                for conflict in &xref.conflicts {
                    md.push_str(&format!("- ⚠️ {}\n", conflict));
                }
                md.push('\n');
            }
        }

        // Assumptions
        if !self.assumptions.is_empty() {
            md.push_str("## Assumptions\n\n");
            for assumption in &self.assumptions {
                md.push_str(&format!("- {}\n", assumption));
            }
            md.push('\n');
        }

        // Open questions
        if !self.open_questions.is_empty() {
            md.push_str("## Open Questions\n\n");
            for question in &self.open_questions {
                md.push_str(&format!("- [ ] {}\n", question));
            }
            md.push('\n');
        }

        // Validation report
        if let Some(ref report) = self.validation_report {
            md.push_str("## Validation Report\n\n");
            md.push_str(&format!(
                "**Complete:** {} | **Ambiguous:** {} | **Untestable:** {}\n\n",
                if report.is_complete { "✅" } else { "❌" },
                report.ambiguous_count,
                report.untestable_count,
            ));

            if !report.issues.is_empty() {
                md.push_str("### Issues\n\n");
                for issue in &report.issues {
                    md.push_str(&format!("- {}\n", issue));
                }
                md.push('\n');
            }
        }

        md
    }

    /// Write to file.
    pub fn write_to_file(&self, dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;

        let slug = slugify(&self.title);
        let date = &self.created_at[..10];
        let filename = format!("{}-{}.md", date, slug);
        let path = dir.join(&filename);

        let content = self.render_markdown();
        fs::write(&path, &content)
            .with_context(|| format!("Failed to write context to {}", path.display()))?;

        Ok(path)
    }
}

// ── Validation report ──────────────────────────────────────────────────

/// Overall validation report for the requirements context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextReport {
    /// Whether the context is complete enough for implementation.
    pub is_complete: bool,
    /// Number of requirements with issues.
    pub requirements_with_issues: usize,
    /// Number of ambiguous requirements.
    pub ambiguous_count: usize,
    /// Number of untestable requirements.
    pub untestable_count: usize,
    /// Number of incomplete requirements.
    pub incomplete_count: usize,
    /// Overall issues.
    pub issues: Vec<String>,
}

// ── Session ────────────────────────────────────────────────────────────

/// A context builder session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBuilderSession {
    /// Current phase.
    pub phase: ContextPhase,
    /// Title / topic.
    pub title: String,
    /// Optional project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// Raw input text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<String>,
    /// Structured requirements.
    pub requirements: Vec<Requirement>,
    /// Key entities.
    pub entities: Vec<Entity>,
    /// Constraints.
    pub constraints: Vec<Constraint>,
    /// Success criteria.
    pub success_criteria: Vec<String>,
    /// Assumptions.
    pub assumptions: Vec<String>,
    /// Open questions.
    pub open_questions: Vec<String>,
    /// Codebase cross-references.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_references: Option<CodebaseCrossRef>,
    /// Validation report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_report: Option<ContextReport>,
    /// The finalized context document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<RequirementsContext>,
}

impl ContextBuilderSession {
    /// Create a new session.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            phase: ContextPhase::Ingest,
            title: title.into(),
            project_root: None,
            raw_input: None,
            requirements: Vec::new(),
            entities: Vec::new(),
            constraints: Vec::new(),
            success_criteria: Vec::new(),
            assumptions: Vec::new(),
            open_questions: Vec::new(),
            cross_references: None,
            validation_report: None,
            document: None,
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
            ContextPhase::Ingest => ContextPhase::Analyze,
            ContextPhase::Analyze => ContextPhase::CrossReference,
            ContextPhase::CrossReference => ContextPhase::Structure,
            ContextPhase::Structure => ContextPhase::Validate,
            ContextPhase::Validate => ContextPhase::Document,
            ContextPhase::Document => ContextPhase::Done,
            ContextPhase::Done => bail!("Cannot advance past Done"),
        };
        self.phase = next;
        Ok(())
    }

    /// Set phase directly.
    pub fn set_phase(&mut self, phase: ContextPhase) {
        self.phase = phase;
    }

    /// Set raw input.
    pub fn set_raw_input(&mut self, input: impl Into<String>) {
        self.raw_input = Some(input.into());
    }

    /// Add a requirement.
    pub fn add_requirement(&mut self, req: Requirement) {
        self.requirements.push(req);
    }

    /// Get requirement by ID.
    pub fn get_requirement(&self, id: &str) -> Option<&Requirement> {
        self.requirements.iter().find(|r| r.id == id)
    }

    /// Number of requirements.
    pub fn requirement_count(&self) -> usize {
        self.requirements.len()
    }

    /// Add an entity.
    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.push(entity);
    }

    /// Add a constraint.
    pub fn add_constraint(&mut self, constraint: Constraint) {
        self.constraints.push(constraint);
    }

    /// Add a success criterion.
    pub fn add_success_criterion(&mut self, criterion: impl Into<String>) {
        self.success_criteria.push(criterion.into());
    }

    /// Add an assumption.
    pub fn add_assumption(&mut self, assumption: impl Into<String>) {
        self.assumptions.push(assumption.into());
    }

    /// Add an open question.
    pub fn add_open_question(&mut self, question: impl Into<String>) {
        self.open_questions.push(question.into());
    }

    /// Set cross-references.
    pub fn set_cross_references(&mut self, xref: CodebaseCrossRef) {
        self.cross_references = Some(xref);
    }

    /// Validate the requirements and produce a report.
    pub fn validate(&mut self) -> ContextReport {
        let mut issues = Vec::new();
        let mut requirements_with_issues = 0;
        let mut ambiguous_count = 0;
        let mut untestable_count = 0;
        let mut incomplete_count = 0;

        for req in &mut self.requirements {
            let mut req_issues: Vec<ValidationIssue> = Vec::new();

            // Check for ambiguity
            let ambiguous_words = ["somehow", "maybe", "possibly", "might", "could", "should probably"];
            let desc_lower = req.description.to_lowercase();
            for word in &ambiguous_words {
                if desc_lower.contains(word) {
                    req_issues.push(ValidationIssue {
                        description: format!("Requirement {} contains ambiguous word: '{}'", req.id, word),
                        severity: ValidationSeverity::Ambiguous,
                    });
                    ambiguous_count += 1;
                    break;
                }
            }

            // Check for testability
            if req.acceptance_criteria.is_empty() {
                req_issues.push(ValidationIssue {
                    description: format!("Requirement {} has no acceptance criteria", req.id),
                    severity: ValidationSeverity::Untestable,
                });
                untestable_count += 1;
            }

            // Check for completeness
            if req.description.is_empty() {
                req_issues.push(ValidationIssue {
                    description: format!("Requirement {} has no description", req.id),
                    severity: ValidationSeverity::Incomplete,
                });
                incomplete_count += 1;
            }

            let is_valid = req_issues.is_empty();
            if !is_valid {
                requirements_with_issues += 1;
            }

            req.validation = Some(RequirementValidation {
                is_valid,
                issues: req_issues,
            });
        }

        // Check for duplicate IDs
        let mut seen_ids: HashMap<String, usize> = HashMap::new();
        for req in &self.requirements {
            *seen_ids.entry(req.id.clone()).or_default() += 1;
        }
        for (id, count) in &seen_ids {
            if *count > 1 {
                issues.push(format!("Duplicate requirement ID: {} (appears {} times)", id, count));
            }
        }

        // Check overall completeness
        if self.requirements.is_empty() {
            issues.push("No requirements defined".to_string());
        }
        if self.success_criteria.is_empty() {
            issues.push("No success criteria defined".to_string());
        }

        let is_complete = requirements_with_issues == 0
            && !self.requirements.is_empty()
            && !self.success_criteria.is_empty()
            && issues.is_empty();

        let report = ContextReport {
            is_complete,
            requirements_with_issues,
            ambiguous_count,
            untestable_count,
            incomplete_count,
            issues,
        };

        self.validation_report = Some(report.clone());
        report
    }

    /// Finalize the context document.
    pub fn finalize(&mut self) -> Result<()> {
        let doc = RequirementsContext {
            title: self.title.clone(),
            created_at: Utc::now().to_rfc3339(),
            version: 1,
            input_summary: self.raw_input.clone().unwrap_or_default(),
            requirements: self.requirements.clone(),
            entities: self.entities.clone(),
            constraints: self.constraints.clone(),
            success_criteria: self.success_criteria.clone(),
            assumptions: self.assumptions.clone(),
            open_questions: self.open_questions.clone(),
            cross_references: self.cross_references.clone(),
            validation_report: self.validation_report.clone(),
        };

        self.document = Some(doc);
        Ok(())
    }

    /// Write the context document to disk.
    pub fn write_document(&self, explicit_path: Option<&Path>) -> Result<PathBuf> {
        let doc = self.document.as_ref()
            .context("Document not finalized — call finalize() first")?;

        if let Some(path) = explicit_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            let content = doc.render_markdown();
            fs::write(path, &content)
                .with_context(|| format!("Failed to write context to {}", path.display()))?;
            Ok(path.to_path_buf())
        } else {
            let root = self.project_root.as_deref()
                .context("No project root and no explicit path")?;
            let ctx_dir = root.join("docs").join("context");
            doc.write_to_file(&ctx_dir)
        }
    }
}

// ── Skill prompt ───────────────────────────────────────────────────────

/// The context builder skill struct.
pub struct ContextBuilderSkill;

impl ContextBuilderSkill {
    /// Create a new context builder skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Generate the system-prompt fragment for the context builder skill.
    pub fn skill_prompt() -> String {
        r#"# Context Builder Skill

You are running the **context-builder** skill. Your job is to take raw
requirements (user input, specs, tickets, or conversations) and produce
a structured requirements context that other skills can consume.

## Workflow

### Phase 1: Ingest

1. Accept raw input — text, spec documents, ticket descriptions, or conversation transcripts.
2. Identify the high-level goal or feature being described.
3. Note any explicit constraints or preferences mentioned.

### Phase 2: Analyze

1. Extract individual requirements from the raw input.
2. For each requirement:
   - Assign a unique ID (REQ-001, REQ-002, ...)
   - Write a clear title and description
   - Classify by category (functional, non-functional, security, etc.)
   - Assign priority (low, medium, high, critical)
   - Define acceptance criteria (testable conditions)
3. Identify key entities (nouns that represent data or concepts).
4. Identify constraints (technical, performance, business, compatibility).

### Phase 3: Cross-Reference

1. Read the codebase to find files relevant to the requirements.
2. Identify existing patterns that apply.
3. Note potential conflicts with existing code.
4. Map requirements to specific files.

### Phase 4: Structure

1. Define clear success criteria — what does "done" look like?
2. List assumptions made during analysis.
3. Capture open questions that need resolution.
4. Organize everything into a structured document.

### Phase 5: Validate

1. Check every requirement for:
   - Ambiguity — vague words, unclear scope
   - Completeness — missing acceptance criteria, no description
   - Testability — can we verify this is implemented?
   - Consistency — conflicts between requirements
2. Flag issues and suggest clarifications.

### Phase 6: Document

1. Write the structured context to `docs/context/YYYY-MM-DD-<slug>.md`.
2. The document is now ready for consumption by planner, oracle, or other skills.

## Rules

- Every requirement MUST have at least one acceptance criterion.
- Priority must be justified — don't mark everything as critical.
- Entities are nouns with attributes — if you can't name attributes, it's not an entity.
- Constraints must be specific — "must be fast" is not a constraint; "< 50ms p99" is.
- If information is missing, list it as an open question rather than guessing.
- The goal is to produce context precise enough for another skill to act on without asking questions.
"#
        .to_string()
    }
}

impl Default for ContextBuilderSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ContextBuilderSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextBuilderSkill").finish()
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

    fn sample_requirement(id: &str) -> Requirement {
        Requirement {
            id: id.to_string(),
            title: format!("Requirement {}", id),
            description: format!("Description for {}", id),
            priority: Priority::High,
            category: RequirementCategory::Functional,
            acceptance_criteria: vec![format!("{} works correctly", id)],
            source: Some("user input".to_string()),
            related_files: vec![format!("src/{}.rs", id.to_lowercase())],
            validation: None,
        }
    }

    fn sample_entity(name: &str) -> Entity {
        Entity {
            name: name.to_string(),
            description: format!("{} entity", name),
            attributes: vec![EntityAttribute {
                name: "id".to_string(),
                attr_type: "string".to_string(),
                required: true,
                description: Some("Unique identifier".to_string()),
            }],
            relationships: vec![],
            source_requirements: vec!["REQ-001".to_string()],
        }
    }

    #[test]
    fn test_session_new() {
        let session = ContextBuilderSession::new("Auth feature");
        assert_eq!(session.phase, ContextPhase::Ingest);
        assert_eq!(session.title, "Auth feature");
        assert!(session.requirements.is_empty());
    }

    #[test]
    fn test_phase_advance() {
        let mut session = ContextBuilderSession::new("test");
        assert_eq!(session.phase, ContextPhase::Ingest);

        session.advance().unwrap(); // Analyze
        assert_eq!(session.phase, ContextPhase::Analyze);

        session.advance().unwrap(); // CrossReference
        assert_eq!(session.phase, ContextPhase::CrossReference);

        session.advance().unwrap(); // Structure
        assert_eq!(session.phase, ContextPhase::Structure);

        session.advance().unwrap(); // Validate
        assert_eq!(session.phase, ContextPhase::Validate);

        session.advance().unwrap(); // Document
        assert_eq!(session.phase, ContextPhase::Document);

        session.advance().unwrap(); // Done
        assert_eq!(session.phase, ContextPhase::Done);

        assert!(session.advance().is_err());
    }

    #[test]
    fn test_set_phase() {
        let mut session = ContextBuilderSession::new("test");
        session.set_phase(ContextPhase::Validate);
        assert_eq!(session.phase, ContextPhase::Validate);
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", ContextPhase::Ingest), "Ingest");
        assert_eq!(format!("{}", ContextPhase::Analyze), "Analyze");
        assert_eq!(format!("{}", ContextPhase::CrossReference), "Cross-Reference");
        assert_eq!(format!("{}", ContextPhase::Structure), "Structure");
        assert_eq!(format!("{}", ContextPhase::Validate), "Validate");
        assert_eq!(format!("{}", ContextPhase::Document), "Document");
        assert_eq!(format!("{}", ContextPhase::Done), "Done");
    }

    #[test]
    fn test_priority_display() {
        assert_eq!(format!("{}", Priority::Low), "low");
        assert_eq!(format!("{}", Priority::Medium), "medium");
        assert_eq!(format!("{}", Priority::High), "high");
        assert_eq!(format!("{}", Priority::Critical), "critical");
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::Critical > Priority::High);
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
    }

    #[test]
    fn test_requirement_category_display() {
        assert_eq!(format!("{}", RequirementCategory::Functional), "functional");
        assert_eq!(format!("{}", RequirementCategory::NonFunctional), "non-functional");
        assert_eq!(format!("{}", RequirementCategory::Security), "security");
    }

    #[test]
    fn test_constraint_type_display() {
        assert_eq!(format!("{}", ConstraintType::Technical), "technical");
        assert_eq!(format!("{}", ConstraintType::Performance), "performance");
        assert_eq!(format!("{}", ConstraintType::Business), "business");
    }

    #[test]
    fn test_relevance_level_display() {
        assert_eq!(format!("{}", RelevanceLevel::Direct), "direct");
        assert_eq!(format!("{}", RelevanceLevel::Contextual), "contextual");
        assert_eq!(format!("{}", RelevanceLevel::Indirect), "indirect");
    }

    #[test]
    fn test_validation_severity_display() {
        assert_eq!(format!("{}", ValidationSeverity::Ambiguous), "ambiguous");
        assert_eq!(format!("{}", ValidationSeverity::Incomplete), "incomplete");
        assert_eq!(format!("{}", ValidationSeverity::Conflicting), "conflicting");
        assert_eq!(format!("{}", ValidationSeverity::Untestable), "untestable");
    }

    #[test]
    fn test_add_requirements() {
        let mut session = ContextBuilderSession::new("test");
        session.add_requirement(sample_requirement("REQ-001"));
        session.add_requirement(sample_requirement("REQ-002"));

        assert_eq!(session.requirement_count(), 2);
        assert!(session.get_requirement("REQ-001").is_some());
        assert!(session.get_requirement("REQ-999").is_none());
    }

    #[test]
    fn test_add_entities_and_constraints() {
        let mut session = ContextBuilderSession::new("test");
        session.add_entity(sample_entity("User"));
        session.add_constraint(Constraint {
            description: "Must be offline-first".to_string(),
            constraint_type: ConstraintType::Technical,
            source: Some("REQ-001".to_string()),
        });

        assert_eq!(session.entities.len(), 1);
        assert_eq!(session.constraints.len(), 1);
    }

    #[test]
    fn test_add_success_criteria_and_questions() {
        let mut session = ContextBuilderSession::new("test");
        session.add_success_criterion("User can log in");
        session.add_assumption("Node.js >= 18");
        session.add_open_question("Which OAuth provider?");

        assert_eq!(session.success_criteria, vec!["User can log in"]);
        assert_eq!(session.assumptions, vec!["Node.js >= 18"]);
        assert_eq!(session.open_questions, vec!["Which OAuth provider?"]);
    }

    #[test]
    fn test_validate_clean() {
        let mut session = ContextBuilderSession::new("test");
        session.add_requirement(sample_requirement("REQ-001"));
        session.add_success_criterion("Works");

        let report = session.validate();
        assert!(report.is_complete);
        assert_eq!(report.requirements_with_issues, 0);
        assert!(session.requirements[0].validation.as_ref().unwrap().is_valid);
    }

    #[test]
    fn test_validate_ambiguous() {
        let mut session = ContextBuilderSession::new("test");
        let mut req = sample_requirement("REQ-001");
        req.description = "The system should somehow handle errors".to_string();
        session.add_requirement(req);
        session.add_success_criterion("Works");

        let report = session.validate();
        assert!(!report.is_complete);
        assert_eq!(report.ambiguous_count, 1);
        assert!(!session.requirements[0].validation.as_ref().unwrap().is_valid);
    }

    #[test]
    fn test_validate_no_acceptance_criteria() {
        let mut session = ContextBuilderSession::new("test");
        let mut req = sample_requirement("REQ-001");
        req.acceptance_criteria = vec![];
        session.add_requirement(req);
        session.add_success_criterion("Works");

        let report = session.validate();
        assert_eq!(report.untestable_count, 1);
    }

    #[test]
    fn test_validate_empty_description() {
        let mut session = ContextBuilderSession::new("test");
        let mut req = sample_requirement("REQ-001");
        req.description = String::new();
        session.add_requirement(req);
        session.add_success_criterion("Works");

        let report = session.validate();
        assert_eq!(report.incomplete_count, 1);
    }

    #[test]
    fn test_validate_no_requirements() {
        let mut session = ContextBuilderSession::new("test");
        let report = session.validate();
        assert!(!report.is_complete);
        assert!(report.issues.iter().any(|i| i.contains("No requirements")));
    }

    #[test]
    fn test_validate_no_success_criteria() {
        let mut session = ContextBuilderSession::new("test");
        session.add_requirement(sample_requirement("REQ-001"));
        let report = session.validate();
        assert!(!report.is_complete);
        assert!(report.issues.iter().any(|i| i.contains("No success criteria")));
    }

    #[test]
    fn test_validate_duplicate_ids() {
        let mut session = ContextBuilderSession::new("test");
        session.add_requirement(sample_requirement("REQ-001"));
        session.add_requirement(sample_requirement("REQ-001"));
        session.add_success_criterion("Works");

        let report = session.validate();
        assert!(report.issues.iter().any(|i| i.contains("Duplicate")));
    }

    #[test]
    fn test_finalize_and_write() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = ContextBuilderSession::new("Auth Context")
            .with_project_root(tmp.path());

        session.set_raw_input("Build authentication for the API");
        session.add_requirement(sample_requirement("REQ-001"));
        session.add_success_criterion("User can authenticate");
        session.add_assumption("JWT tokens");
        session.validate();

        session.finalize().unwrap();
        let path = session.write_document(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("docs/context"));

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Auth Context"));
        assert!(content.contains("## Input Summary"));
        assert!(content.contains("Build authentication"));
        assert!(content.contains("## Requirements"));
        assert!(content.contains("REQ-001"));
    }

    #[test]
    fn test_write_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = ContextBuilderSession::new("test");
        session.add_requirement(sample_requirement("REQ-001"));
        session.finalize().unwrap();

        let explicit = tmp.path().join("context.md");
        let path = session.write_document(Some(&explicit)).unwrap();
        assert_eq!(path, explicit);
        assert!(path.exists());
    }

    #[test]
    fn test_write_not_finalized() {
        let session = ContextBuilderSession::new("test");
        assert!(session.write_document(None).is_err());
    }

    #[test]
    fn test_render_markdown_full() {
        let mut session = ContextBuilderSession::new("Full Test");
        session.set_raw_input("Raw input text");
        session.add_requirement(Requirement {
            id: "REQ-001".to_string(),
            title: "User login".to_string(),
            description: "Users must be able to log in".to_string(),
            priority: Priority::Critical,
            category: RequirementCategory::Functional,
            acceptance_criteria: vec!["Login form accepts credentials".to_string()],
            source: Some("product spec".to_string()),
            related_files: vec!["src/auth.rs".to_string()],
            validation: Some(RequirementValidation {
                is_valid: true,
                issues: vec![],
            }),
        });

        session.add_entity(Entity {
            name: "User".to_string(),
            description: "A system user".to_string(),
            attributes: vec![
                EntityAttribute {
                    name: "email".to_string(),
                    attr_type: "string".to_string(),
                    required: true,
                    description: Some("User email".to_string()),
                },
                EntityAttribute {
                    name: "password_hash".to_string(),
                    attr_type: "string".to_string(),
                    required: true,
                    description: None,
                },
            ],
            relationships: vec!["Session (1:N)".to_string()],
            source_requirements: vec!["REQ-001".to_string()],
        });

        session.add_constraint(Constraint {
            description: "Passwords must be hashed with bcrypt".to_string(),
            constraint_type: ConstraintType::Regulatory,
            source: Some("REQ-001".to_string()),
        });

        session.add_success_criterion("Users can log in and receive a token");
        session.add_assumption("Single-factor auth only");
        session.add_open_question("Password reset flow?");

        session.set_cross_references(CodebaseCrossRef {
            relevant_files: vec![FileRelevance {
                path: "src/auth.rs".to_string(),
                relevance: RelevanceLevel::Direct,
                reason: "Auth module".to_string(),
                related_requirements: vec!["REQ-001".to_string()],
            }],
            applicable_patterns: vec!["Middleware pattern".to_string()],
            dependencies: vec!["bcrypt".to_string()],
            conflicts: vec![],
        });

        session.validate();
        session.finalize().unwrap();

        let md = session.document.as_ref().unwrap().render_markdown();
        assert!(md.contains("# Full Test"));
        assert!(md.contains("## Input Summary"));
        assert!(md.contains("## Success Criteria"));
        assert!(md.contains("## Requirements"));
        assert!(md.contains("REQ-001"));
        assert!(md.contains("User login"));
        assert!(md.contains("critical"));
        assert!(md.contains("✅"));
        assert!(md.contains("## Key Entities"));
        assert!(md.contains("### User"));
        assert!(md.contains("email"));
        assert!(md.contains("password_hash"));
        assert!(md.contains("Session (1:N)"));
        assert!(md.contains("## Constraints"));
        assert!(md.contains("bcrypt"));
        assert!(md.contains("regulatory"));
        assert!(md.contains("## Codebase Cross-References"));
        assert!(md.contains("`src/auth.rs`"));
        assert!(md.contains("Middleware pattern"));
        assert!(md.contains("## Assumptions"));
        assert!(md.contains("Single-factor auth"));
        assert!(md.contains("## Open Questions"));
        assert!(md.contains("Password reset flow?"));
        assert!(md.contains("## Validation Report"));
    }

    #[test]
    fn test_session_serialization_roundtrip() {
        let mut session = ContextBuilderSession::new("Test");
        session.add_requirement(sample_requirement("REQ-001"));
        session.add_success_criterion("Works");
        session.set_phase(ContextPhase::Analyze);

        let json = serde_json::to_string(&session).unwrap();
        let parsed: ContextBuilderSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.phase, ContextPhase::Analyze);
        assert_eq!(parsed.requirement_count(), 1);
    }

    #[test]
    fn test_requirements_context_serialization_roundtrip() {
        let ctx = RequirementsContext {
            title: "Test".to_string(),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            version: 1,
            input_summary: "Raw".to_string(),
            requirements: vec![sample_requirement("REQ-001")],
            entities: vec![sample_entity("User")],
            constraints: vec![Constraint {
                description: "Test".to_string(),
                constraint_type: ConstraintType::Technical,
                source: None,
            }],
            success_criteria: vec!["Done".to_string()],
            assumptions: vec![],
            open_questions: vec![],
            cross_references: None,
            validation_report: None,
        };

        let json = serde_json::to_string_pretty(&ctx).unwrap();
        let parsed: RequirementsContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.requirements.len(), 1);
        assert_eq!(parsed.entities.len(), 1);
        assert_eq!(parsed.constraints.len(), 1);
    }

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = ContextBuilderSkill::skill_prompt();
        assert!(prompt.contains("Context Builder Skill"));
        assert!(prompt.contains("Phase 1: Ingest"));
        assert!(prompt.contains("Phase 6: Document"));
    }
}
