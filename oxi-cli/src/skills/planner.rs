//! Planner skill for oxi
//!
//! Deep analysis and architecture planning skill that produces structured
//! implementation plans from requirements. The planner:
//!
//! 1. **Gathers context** — reads project structure, specs, and existing code
//! 2. **Decomposes** into vertical slices with clear dependencies
//! 3. **Batches** independent tasks for parallel execution
//! 4. **Tracks** acceptance criteria and verification methods per task
//! 5. **Outputs** a structured plan document (`docs/plan/...`)
//!
//! The module provides:
//! - [`PlannerSession`] — state machine tracking the planning lifecycle
//! - [`PlanTask`] / [`TaskBatch`] — structured task and batch definitions
//! - [`PlanDocument`] — the complete plan that can be rendered to Markdown
//! - [`PlannerSkill`] — skill prompt generator for LLM-driven planning

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Phase ──────────────────────────────────────────────────────────────

/// The phase a planner session is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerPhase {
    /// Reading project context and requirements.
    Gather,
    /// Decomposing into tasks with dependencies.
    Decompose,
    /// Batching tasks for parallel execution.
    Batch,
    /// Reviewing the plan for completeness and correctness.
    Review,
    /// Writing the plan document to disk.
    Document,
    /// Planning complete.
    Done,
}

impl Default for PlannerPhase {
    fn default() -> Self {
        PlannerPhase::Gather
    }
}

impl fmt::Display for PlannerPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlannerPhase::Gather => write!(f, "Gather"),
            PlannerPhase::Decompose => write!(f, "Decompose"),
            PlannerPhase::Batch => write!(f, "Batch"),
            PlannerPhase::Review => write!(f, "Review"),
            PlannerPhase::Document => write!(f, "Document"),
            PlannerPhase::Done => write!(f, "Done"),
        }
    }
}

// ── Task types ─────────────────────────────────────────────────────────

/// A single implementation task within a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    /// Task identifier (e.g., "T1", "T2").
    pub id: String,
    /// Short description of what this task accomplishes.
    pub title: String,
    /// Detailed approach and key changes.
    pub description: String,
    /// Files to create or modify.
    pub touches_files: Vec<String>,
    /// IDs of tasks this task depends on.
    pub depends_on: Vec<String>,
    /// Concrete acceptance criteria.
    pub acceptance_criteria: Vec<String>,
    /// How to verify this task works.
    pub verification: String,
    /// Estimated complexity.
    pub complexity: TaskComplexity,
    /// Whether this task needs TDD (test-first).
    pub tdd: bool,
    /// Which batch this task belongs to (set during batching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<usize>,
}

/// Task complexity rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskComplexity {
    /// < 30 min, single file, straightforward change.
    Trivial,
    /// 1–2 hours, 1–2 files, well-understood approach.
    Simple,
    /// Half a day, 2–4 files, some design decisions.
    Moderate,
    /// Full day, 3–5 files, significant design decisions.
    Complex,
    /// Multi-day, 5+ files, architectural changes.
    Large,
}

impl fmt::Display for TaskComplexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskComplexity::Trivial => write!(f, "trivial"),
            TaskComplexity::Simple => write!(f, "simple"),
            TaskComplexity::Moderate => write!(f, "moderate"),
            TaskComplexity::Complex => write!(f, "complex"),
            TaskComplexity::Large => write!(f, "large"),
        }
    }
}

/// A batch of tasks that can be executed in parallel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBatch {
    /// Batch index (0-based).
    pub index: usize,
    /// Tasks in this batch.
    pub tasks: Vec<String>,
    /// Whether any tasks in this batch have file conflicts.
    pub has_conflicts: bool,
    /// Execution strategy for this batch.
    pub strategy: BatchStrategy,
}

/// How to execute a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStrategy {
    /// All tasks are independent and can run in parallel.
    Parallel,
    /// Tasks must run sequentially (dependencies or file conflicts).
    Sequential,
    /// Tasks form a pipeline — each builds on the previous.
    Chain,
}

impl fmt::Display for BatchStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchStrategy::Parallel => write!(f, "parallel"),
            BatchStrategy::Sequential => write!(f, "sequential"),
            BatchStrategy::Chain => write!(f, "chain"),
        }
    }
}

// ── Risk and dependency ────────────────────────────────────────────────

/// A risk identified during planning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRisk {
    /// Short description of the risk.
    pub description: String,
    /// Likelihood of the risk materializing.
    pub likelihood: RiskLikelihood,
    /// Impact if the risk materializes.
    pub impact: RiskImpact,
    /// Mitigation strategy.
    pub mitigation: String,
}

/// Risk likelihood level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLikelihood {
    Low,
    Medium,
    High,
}

impl fmt::Display for RiskLikelihood {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskLikelihood::Low => write!(f, "low"),
            RiskLikelihood::Medium => write!(f, "medium"),
            RiskLikelihood::High => write!(f, "high"),
        }
    }
}

/// Risk impact level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskImpact {
    Low,
    Medium,
    High,
}

impl fmt::Display for RiskImpact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskImpact::Low => write!(f, "low"),
            RiskImpact::Medium => write!(f, "medium"),
            RiskImpact::High => write!(f, "high"),
        }
    }
}

// ── Plan document ──────────────────────────────────────────────────────

/// Project context gathered during the Gather phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanContext {
    /// Project root directory.
    pub project_root: String,
    /// Key files read during context gathering.
    pub key_files: Vec<(String, String)>,
    /// Existing patterns and conventions detected.
    pub conventions: Vec<String>,
    /// Dependencies relevant to the plan.
    pub dependencies: Vec<String>,
    /// Summary of project understanding.
    pub summary: String,
}

/// The complete plan document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDocument {
    /// Plan title.
    pub title: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Version (incremented on updates).
    pub version: u32,
    /// Objective statement.
    pub objective: String,
    /// Project context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<PlanContext>,
    /// All tasks.
    pub tasks: Vec<PlanTask>,
    /// Execution batches.
    pub batches: Vec<TaskBatch>,
    /// Risks identified.
    pub risks: Vec<PlanRisk>,
    /// Assumptions made.
    pub assumptions: Vec<String>,
    /// Out-of-scope items explicitly excluded.
    pub out_of_scope: Vec<String>,
    /// Open questions that need resolution.
    pub open_questions: Vec<String>,
}

impl PlanDocument {
    /// Render the plan as Markdown.
    pub fn render_markdown(&self) -> String {
        let mut md = String::with_capacity(4096);

        md.push_str(&format!("# {}\n\n", self.title));
        md.push_str(&format!(
            "> Created: {} | Version: {}\n\n",
            self.created_at, self.version
        ));

        // Objective
        md.push_str("## Objective\n\n");
        md.push_str(&self.objective);
        md.push_str("\n\n");

        // Context
        if let Some(ref ctx) = self.context {
            md.push_str("## Context\n\n");
            md.push_str(&ctx.summary);
            md.push_str("\n\n");

            if !ctx.conventions.is_empty() {
                md.push_str("**Conventions:**\n");
                for conv in &ctx.conventions {
                    md.push_str(&format!("- {}\n", conv));
                }
                md.push('\n');
            }

            if !ctx.dependencies.is_empty() {
                md.push_str("**Relevant Dependencies:**\n");
                for dep in &ctx.dependencies {
                    md.push_str(&format!("- {}\n", dep));
                }
                md.push('\n');
            }
        }

        // Tasks
        if !self.tasks.is_empty() {
            md.push_str("## Tasks\n\n");
            for task in &self.tasks {
                let batch_label = task
                    .batch
                    .map(|b| format!(" [Batch {}]", b))
                    .unwrap_or_default();
                md.push_str(&format!(
                    "### {}{}: {}\n\n",
                    task.id, batch_label, task.title
                ));
                md.push_str(&task.description);
                md.push_str("\n\n");

                if !task.touches_files.is_empty() {
                    md.push_str("**Files:**\n");
                    for file in &task.touches_files {
                        md.push_str(&format!("- `{}`\n", file));
                    }
                    md.push('\n');
                }

                if !task.depends_on.is_empty() {
                    md.push_str(&format!(
                        "**Depends on:** {}\n\n",
                        task.depends_on.join(", ")
                    ));
                }

                if !task.acceptance_criteria.is_empty() {
                    md.push_str("**Acceptance Criteria:**\n");
                    for (i, criterion) in task.acceptance_criteria.iter().enumerate() {
                        md.push_str(&format!("{}. [ ] {}\n", i + 1, criterion));
                    }
                    md.push('\n');
                }

                md.push_str(&format!(
                    "**Verification:** {} | **Complexity:** {}{}{}\n\n",
                    task.verification,
                    task.complexity,
                    if task.tdd { " | **TDD**" } else { "" },
                    "",
                ));
            }
        }

        // Batches
        if !self.batches.is_empty() {
            md.push_str("## Execution Batches\n\n");
            for batch in &self.batches {
                md.push_str(&format!(
                    "### Batch {} ({}){}\n\n",
                    batch.index,
                    batch.strategy,
                    if batch.has_conflicts {
                        " ⚠️ has file conflicts"
                    } else {
                        ""
                    },
                ));
                for task_id in &batch.tasks {
                    md.push_str(&format!("- {}\n", task_id));
                }
                md.push('\n');
            }
        }

        // Risks
        if !self.risks.is_empty() {
            md.push_str("## Risks\n\n");
            md.push_str("| Risk | Likelihood | Impact | Mitigation |\n");
            md.push_str("|------|-----------|--------|------------|\n");
            for risk in &self.risks {
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    risk.description, risk.likelihood, risk.impact, risk.mitigation
                ));
            }
            md.push('\n');
        }

        // Assumptions
        if !self.assumptions.is_empty() {
            md.push_str("## Assumptions\n\n");
            for assumption in &self.assumptions {
                md.push_str(&format!("- {}\n", assumption));
            }
            md.push('\n');
        }

        // Out of scope
        if !self.out_of_scope.is_empty() {
            md.push_str("## Out of Scope\n\n");
            for item in &self.out_of_scope {
                md.push_str(&format!("- {}\n", item));
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

        md
    }

    /// Write the plan to a Markdown file.
    pub fn write_to_file(&self, dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

        let slug = slugify(&self.title);
        let date = &self.created_at[..10];
        let filename = format!("{}-{}.md", date, slug);
        let path = dir.join(&filename);

        let content = self.render_markdown();
        fs::write(&path, &content)
            .with_context(|| format!("Failed to write plan to {}", path.display()))?;

        Ok(path)
    }
}

// ── Planner session ────────────────────────────────────────────────────

/// A planner session that tracks the planning lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerSession {
    /// Current phase.
    pub phase: PlannerPhase,
    /// Plan title / topic.
    pub title: String,
    /// Optional project root directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// Gathered context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<PlanContext>,
    /// Tasks defined during decomposition.
    pub tasks: Vec<PlanTask>,
    /// Batches built during the batch phase.
    pub batches: Vec<TaskBatch>,
    /// Risks identified.
    pub risks: Vec<PlanRisk>,
    /// Assumptions.
    pub assumptions: Vec<String>,
    /// Out-of-scope items.
    pub out_of_scope: Vec<String>,
    /// Open questions.
    pub open_questions: Vec<String>,
    /// The finalized plan document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanDocument>,
}

impl PlannerSession {
    /// Create a new planner session.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            phase: PlannerPhase::Gather,
            title: title.into(),
            project_root: None,
            context: None,
            tasks: Vec::new(),
            batches: Vec::new(),
            risks: Vec::new(),
            assumptions: Vec::new(),
            out_of_scope: Vec::new(),
            open_questions: Vec::new(),
            plan: None,
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
            PlannerPhase::Gather => PlannerPhase::Decompose,
            PlannerPhase::Decompose => PlannerPhase::Batch,
            PlannerPhase::Batch => PlannerPhase::Review,
            PlannerPhase::Review => PlannerPhase::Document,
            PlannerPhase::Document => PlannerPhase::Done,
            PlannerPhase::Done => bail!("Cannot advance past Done"),
        };
        self.phase = next;
        Ok(())
    }

    /// Set phase directly.
    pub fn set_phase(&mut self, phase: PlannerPhase) {
        self.phase = phase;
    }

    /// Set the project context.
    pub fn set_context(&mut self, ctx: PlanContext) {
        self.context = Some(ctx);
    }

    /// Add a task.
    pub fn add_task(&mut self, task: PlanTask) {
        self.tasks.push(task);
    }

    /// Get a task by ID.
    pub fn get_task(&self, id: &str) -> Option<&PlanTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Get a mutable task by ID.
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut PlanTask> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Number of tasks.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Add a risk.
    pub fn add_risk(&mut self, risk: PlanRisk) {
        self.risks.push(risk);
    }

    /// Add an assumption.
    pub fn add_assumption(&mut self, assumption: impl Into<String>) {
        self.assumptions.push(assumption.into());
    }

    /// Add an out-of-scope item.
    pub fn add_out_of_scope(&mut self, item: impl Into<String>) {
        self.out_of_scope.push(item.into());
    }

    /// Add an open question.
    pub fn add_open_question(&mut self, question: impl Into<String>) {
        self.open_questions.push(question.into());
    }

    /// Build execution batches from tasks using dependency analysis.
    ///
    /// Groups tasks into batches where each batch contains tasks whose
    /// dependencies are all in earlier batches. Detects file conflicts
    /// between tasks in the same batch.
    pub fn build_batches(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            bail!("No tasks to batch");
        }

        let mut batches: Vec<TaskBatch> = Vec::new();
        let mut assigned: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Iteratively assign tasks to batches
        loop {
            let mut ready: Vec<String> = Vec::new();

            for task in &self.tasks {
                if assigned.contains(&task.id) {
                    continue;
                }
                // Check if all dependencies are assigned
                let deps_met = task.depends_on.iter().all(|dep| assigned.contains(dep));
                if deps_met {
                    ready.push(task.id.clone());
                }
            }

            if ready.is_empty() {
                break;
            }

            let batch_index = batches.len();

            // Check for file conflicts within this batch
            let mut file_owners: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut has_conflicts = false;

            for task_id in &ready {
                if let Some(task) = self.get_task(task_id) {
                    for file in &task.touches_files {
                        if let Some(existing) = file_owners.get(file) {
                            tracing::debug!(
                                "File conflict: {} touched by {} and {}",
                                file,
                                existing,
                                task_id
                            );
                            has_conflicts = true;
                        } else {
                            file_owners.insert(file.clone(), task_id.clone());
                        }
                    }
                }
            }

            let strategy = if has_conflicts {
                BatchStrategy::Sequential
            } else if batch_index == 0 {
                BatchStrategy::Parallel
            } else {
                BatchStrategy::Parallel
            };

            // Mark tasks as assigned
            for task_id in &ready {
                assigned.insert(task_id.clone());
                if let Some(task) = self.get_task_mut(task_id) {
                    task.batch = Some(batch_index);
                }
            }

            batches.push(TaskBatch {
                index: batch_index,
                tasks: ready,
                has_conflicts,
                strategy,
            });
        }

        // Check for unassigned tasks (circular dependencies)
        let unassigned: Vec<&str> = self
            .tasks
            .iter()
            .filter(|t| !assigned.contains(&t.id))
            .map(|t| t.id.as_str())
            .collect();

        if !unassigned.is_empty() {
            bail!(
                "Circular dependency detected — these tasks could not be batched: {}",
                unassigned.join(", ")
            );
        }

        self.batches = batches;
        Ok(())
    }

    /// Validate the plan: check for missing dependencies, empty tasks, etc.
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let task_ids: std::collections::HashSet<&str> =
            self.tasks.iter().map(|t| t.id.as_str()).collect();

        for task in &self.tasks {
            // Check dependencies exist
            for dep in &task.depends_on {
                if !task_ids.contains(dep.as_str()) {
                    issues.push(ValidationIssue {
                        severity: ValidationSeverity::Error,
                        task_id: Some(task.id.clone()),
                        message: format!("Task {} depends on non-existent task '{}'", task.id, dep),
                    });
                }
            }

            // Check self-dependency
            if task.depends_on.contains(&task.id) {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Error,
                    task_id: Some(task.id.clone()),
                    message: format!("Task {} depends on itself", task.id),
                });
            }

            // Check for empty acceptance criteria
            if task.acceptance_criteria.is_empty() {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    task_id: Some(task.id.clone()),
                    message: format!("Task {} has no acceptance criteria", task.id),
                });
            }

            // Check for missing verification
            if task.verification.is_empty() {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    task_id: Some(task.id.clone()),
                    message: format!("Task {} has no verification method", task.id),
                });
            }

            // Check for missing files
            if task.touches_files.is_empty() {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    task_id: Some(task.id.clone()),
                    message: format!("Task {} has no files specified", task.id),
                });
            }
        }

        issues
    }

    /// Finalize the plan document.
    pub fn finalize(&mut self) -> Result<()> {
        let doc = PlanDocument {
            title: self.title.clone(),
            created_at: Utc::now().to_rfc3339(),
            version: 1,
            objective: self.title.clone(),
            context: self.context.clone(),
            tasks: self.tasks.clone(),
            batches: self.batches.clone(),
            risks: self.risks.clone(),
            assumptions: self.assumptions.clone(),
            out_of_scope: self.out_of_scope.clone(),
            open_questions: self.open_questions.clone(),
        };
        self.plan = Some(doc);
        Ok(())
    }

    /// Write the plan document to disk.
    pub fn write_plan(&self, explicit_path: Option<&Path>) -> Result<PathBuf> {
        let doc = self
            .plan
            .as_ref()
            .context("Plan has not been finalized — call finalize() first")?;

        if let Some(path) = explicit_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            let content = doc.render_markdown();
            fs::write(path, &content)
                .with_context(|| format!("Failed to write plan to {}", path.display()))?;
            Ok(path.to_path_buf())
        } else {
            let root = self
                .project_root
                .as_deref()
                .context("No project root set and no explicit path provided")?;
            let plan_dir = root.join("docs").join("plan");
            doc.write_to_file(&plan_dir)
        }
    }
}

// ── Validation ─────────────────────────────────────────────────────────

/// Severity of a validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    /// Must be fixed before proceeding.
    Error,
    /// Should be fixed but doesn't block progress.
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

/// A validation issue found in the plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// Severity of the issue.
    pub severity: ValidationSeverity,
    /// Task ID this issue relates to (if applicable).
    pub task_id: Option<String>,
    /// Description of the issue.
    pub message: String,
}

// ── Skill prompt ───────────────────────────────────────────────────────

/// The planner skill struct for prompt generation.
pub struct PlannerSkill;

impl PlannerSkill {
    /// Create a new planner skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Generate the system-prompt fragment for the planner skill.
    pub fn skill_prompt() -> String {
        r#"# Planner Skill

You are running the **planner** skill. Your job is to produce a structured
implementation plan from requirements or a design document.

## Workflow

### Phase 1: Gather Context

1. Read the project structure (directory tree, key config files).
2. Read any existing specs, designs, or requirements documents.
3. Identify conventions (coding style, testing patterns, module layout).
4. Summarize your understanding and confirm with the user.

### Phase 2: Decompose into Tasks

1. Break the objective into vertical slices — each delivers a working, testable increment.
2. For each task, define:
   - **ID** (T1, T2, ...)
   - **Title** — one-line description
   - **Description** — detailed approach and key changes
   - **Files** — exact files to create or modify
   - **Depends on** — task IDs this depends on
   - **Acceptance criteria** — concrete, testable conditions
   - **Verification** — how to confirm it works (test command, build, manual check)
   - **Complexity** — trivial / simple / moderate / complex / large
   - **TDD** — whether to write tests first

3. Rules:
   - Every task must have acceptance criteria and a verification method.
   - No task should exceed ~5 files.
   - No vague tasks — each must have a clear approach.
   - Mark logic tasks (parsers, algorithms, data transforms) for TDD.

### Phase 3: Build Batches

1. Group tasks into execution batches based on dependencies:
   - Batch 1: tasks with no dependencies (max parallelism)
   - Batch N: tasks whose dependencies are all in earlier batches
2. Detect file conflicts between tasks in the same batch.
3. Mark conflicting batches as sequential; non-conflicting as parallel.
4. Present the batch plan for review.

### Phase 4: Review

1. Validate the plan:
   - All dependencies exist
   - No circular dependencies
   - Every task has acceptance criteria and verification
   - Every requirement is covered by at least one task
2. Identify risks and mitigations
3. List assumptions and open questions
4. Iterate until the user approves

### Phase 5: Document

1. Write the plan to `docs/plan/YYYY-MM-DD-<slug>.md`.
2. Confirm the file was written.
3. The plan is now ready for hand-off to implementation.

## Rules

- Simplicity first: fewer tasks is better than more.
- Every task must be independently verifiable.
- Dependencies must form a DAG (no cycles).
- If a task is too complex to describe in a paragraph, split it.
- Prefer vertical slices over horizontal layers.
"#
        .to_string()
    }
}

impl Default for PlannerSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PlannerSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannerSkill").finish()
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert a title into a filesystem-safe slug.
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

    fn sample_task(id: &str, title: &str, depends_on: Vec<&str>) -> PlanTask {
        PlanTask {
            id: id.to_string(),
            title: title.to_string(),
            description: format!("Implement {}", title),
            touches_files: vec![format!("src/{}.rs", id.to_lowercase())],
            depends_on: depends_on.into_iter().map(|s| s.to_string()).collect(),
            acceptance_criteria: vec![format!("{} works correctly", title)],
            verification: format!("cargo test {}", id.to_lowercase()),
            complexity: TaskComplexity::Moderate,
            tdd: false,
            batch: None,
        }
    }

    #[test]
    fn test_session_new() {
        let session = PlannerSession::new("Build auth system");
        assert_eq!(session.phase, PlannerPhase::Gather);
        assert_eq!(session.title, "Build auth system");
        assert!(session.tasks.is_empty());
        assert!(session.batches.is_empty());
    }

    #[test]
    fn test_phase_advance() {
        let mut session = PlannerSession::new("test");
        assert_eq!(session.phase, PlannerPhase::Gather);

        session.advance().unwrap();
        assert_eq!(session.phase, PlannerPhase::Decompose);

        session.advance().unwrap();
        assert_eq!(session.phase, PlannerPhase::Batch);

        session.advance().unwrap();
        assert_eq!(session.phase, PlannerPhase::Review);

        session.advance().unwrap();
        assert_eq!(session.phase, PlannerPhase::Document);

        session.advance().unwrap();
        assert_eq!(session.phase, PlannerPhase::Done);

        assert!(session.advance().is_err());
    }

    #[test]
    fn test_set_phase() {
        let mut session = PlannerSession::new("test");
        session.set_phase(PlannerPhase::Review);
        assert_eq!(session.phase, PlannerPhase::Review);
    }

    #[test]
    fn test_add_and_get_tasks() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "Core module", vec![]));
        session.add_task(sample_task("T2", "API layer", vec!["T1"]));

        assert_eq!(session.task_count(), 2);
        assert_eq!(session.get_task("T1").unwrap().title, "Core module");
        assert_eq!(
            session.get_task("T2").unwrap().depends_on,
            vec!["T1".to_string()]
        );
        assert!(session.get_task("T99").is_none());
    }

    #[test]
    fn test_risks_and_assumptions() {
        let mut session = PlannerSession::new("test");
        session.add_risk(PlanRisk {
            description: "Third-party API may change".to_string(),
            likelihood: RiskLikelihood::Medium,
            impact: RiskImpact::High,
            mitigation: "Abstract behind interface".to_string(),
        });
        session.add_assumption("Node.js >= 18");
        session.add_out_of_scope("Mobile app");
        session.add_open_question("Which DB to use?");

        assert_eq!(session.risks.len(), 1);
        assert_eq!(session.assumptions, vec!["Node.js >= 18"]);
        assert_eq!(session.out_of_scope, vec!["Mobile app"]);
        assert_eq!(session.open_questions, vec!["Which DB to use?"]);
    }

    #[test]
    fn test_build_batches_linear() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "Base", vec![]));
        session.add_task(sample_task("T2", "Mid", vec!["T1"]));
        session.add_task(sample_task("T3", "Top", vec!["T2"]));

        session.build_batches().unwrap();

        assert_eq!(session.batches.len(), 3);
        assert_eq!(session.batches[0].tasks, vec!["T1"]);
        assert_eq!(session.batches[1].tasks, vec!["T2"]);
        assert_eq!(session.batches[2].tasks, vec!["T3"]);
    }

    #[test]
    fn test_build_batches_parallel() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "A", vec![]));
        session.add_task(sample_task("T2", "B", vec![]));
        session.add_task(sample_task("T3", "C", vec!["T1", "T2"]));

        session.build_batches().unwrap();

        assert_eq!(session.batches.len(), 2);
        // T1 and T2 in batch 0 (parallel)
        assert_eq!(session.batches[0].tasks.len(), 2);
        assert!(session.batches[0].tasks.contains(&"T1".to_string()));
        assert!(session.batches[0].tasks.contains(&"T2".to_string()));
        assert_eq!(session.batches[0].strategy, BatchStrategy::Parallel);
        // T3 in batch 1
        assert_eq!(session.batches[1].tasks, vec!["T3"]);
    }

    #[test]
    fn test_build_batches_file_conflicts() {
        let mut session = PlannerSession::new("test");
        let mut t1 = sample_task("T1", "A", vec![]);
        let mut t2 = sample_task("T2", "B", vec![]);
        // Both touch the same file
        t1.touches_files = vec!["src/lib.rs".to_string()];
        t2.touches_files = vec!["src/lib.rs".to_string()];

        session.add_task(t1);
        session.add_task(t2);

        session.build_batches().unwrap();

        assert_eq!(session.batches.len(), 1);
        assert!(session.batches[0].has_conflicts);
        assert_eq!(session.batches[0].strategy, BatchStrategy::Sequential);
    }

    #[test]
    fn test_build_batches_circular_dependency() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "A", vec!["T2"]));
        session.add_task(sample_task("T2", "B", vec!["T1"]));

        let result = session.build_batches();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Circular"));
    }

    #[test]
    fn test_build_batches_empty_tasks() {
        let mut session = PlannerSession::new("test");
        assert!(session.build_batches().is_err());
    }

    #[test]
    fn test_validate_clean() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "Good task", vec![]));

        let issues = session.validate();
        assert!(issues.is_empty());
    }

    #[test]
    fn test_validate_missing_dependency() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "Task", vec!["NONEXISTENT"]));

        let issues = session.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.severity == ValidationSeverity::Error
                    && i.message.contains("non-existent"))
        );
    }

    #[test]
    fn test_validate_self_dependency() {
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "Task", vec!["T1"]));

        let issues = session.validate();
        assert!(issues
            .iter()
            .any(|i| i.message.contains("depends on itself")));
    }

    #[test]
    fn test_validate_warnings() {
        let mut session = PlannerSession::new("test");
        session.add_task(PlanTask {
            id: "T1".to_string(),
            title: "Vague".to_string(),
            description: "Do something".to_string(),
            touches_files: vec![],
            depends_on: vec![],
            acceptance_criteria: vec![],
            verification: String::new(),
            complexity: TaskComplexity::Simple,
            tdd: false,
            batch: None,
        });

        let issues = session.validate();
        assert!(issues
            .iter()
            .any(|i| i.message.contains("no acceptance criteria")));
        assert!(issues
            .iter()
            .any(|i| i.message.contains("no verification method")));
        assert!(issues
            .iter()
            .any(|i| i.message.contains("no files specified")));
    }

    #[test]
    fn test_finalize_and_write() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = PlannerSession::new("Auth System Plan").with_project_root(tmp.path());
        session.add_task(sample_task("T1", "Core auth", vec![]));
        session.build_batches().unwrap();
        session.finalize().unwrap();

        let path = session.write_plan(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("docs/plan"));

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Auth System Plan"));
        assert!(content.contains("## Tasks"));
        assert!(content.contains("T1"));
    }

    #[test]
    fn test_write_plan_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = PlannerSession::new("test");
        session.add_task(sample_task("T1", "Task", vec![]));
        session.build_batches().unwrap();
        session.finalize().unwrap();

        let explicit = tmp.path().join("custom-plan.md");
        let path = session.write_plan(Some(&explicit)).unwrap();
        assert_eq!(path, explicit);
        assert!(path.exists());
    }

    #[test]
    fn test_write_plan_not_finalized() {
        let session = PlannerSession::new("test");
        assert!(session.write_plan(None).is_err());
    }

    #[test]
    fn test_render_markdown() {
        let mut session = PlannerSession::new("Test Plan");
        session.add_assumption("Rust stable");
        session.add_out_of_scope("Benchmarking");
        session.add_open_question("DB choice?");
        session.add_risk(PlanRisk {
            description: "API unstable".to_string(),
            likelihood: RiskLikelihood::Medium,
            impact: RiskImpact::High,
            mitigation: "Pin version".to_string(),
        });
        session.add_task(PlanTask {
            id: "T1".to_string(),
            title: "Setup project".to_string(),
            description: "Initialize the project structure".to_string(),
            touches_files: vec!["Cargo.toml".to_string(), "src/lib.rs".to_string()],
            depends_on: vec![],
            acceptance_criteria: vec!["Project compiles".to_string()],
            verification: "cargo build".to_string(),
            complexity: TaskComplexity::Trivial,
            tdd: false,
            batch: Some(0),
        });
        session.finalize().unwrap();

        let md = session.plan.unwrap().render_markdown();
        assert!(md.contains("# Test Plan"));
        assert!(md.contains("## Objective"));
        assert!(md.contains("## Tasks"));
        assert!(md.contains("### T1 [Batch 0]: Setup project"));
        assert!(md.contains("`Cargo.toml`"));
        assert!(md.contains("## Risks"));
        assert!(md.contains("API unstable"));
        assert!(md.contains("## Assumptions"));
        assert!(md.contains("Rust stable"));
        assert!(md.contains("## Out of Scope"));
        assert!(md.contains("Benchmarking"));
        assert!(md.contains("## Open Questions"));
        assert!(md.contains("DB choice?"));
    }

    #[test]
    fn test_session_serialization_roundtrip() {
        let mut session = PlannerSession::new("Test");
        session.add_task(sample_task("T1", "Task", vec![]));
        session.set_phase(PlannerPhase::Batch);

        let json = serde_json::to_string(&session).unwrap();
        let parsed: PlannerSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.phase, PlannerPhase::Batch);
        assert_eq!(parsed.tasks.len(), 1);
    }

    #[test]
    fn test_task_complexity_display() {
        assert_eq!(format!("{}", TaskComplexity::Trivial), "trivial");
        assert_eq!(format!("{}", TaskComplexity::Simple), "simple");
        assert_eq!(format!("{}", TaskComplexity::Moderate), "moderate");
        assert_eq!(format!("{}", TaskComplexity::Complex), "complex");
        assert_eq!(format!("{}", TaskComplexity::Large), "large");
    }

    #[test]
    fn test_batch_strategy_display() {
        assert_eq!(format!("{}", BatchStrategy::Parallel), "parallel");
        assert_eq!(format!("{}", BatchStrategy::Sequential), "sequential");
        assert_eq!(format!("{}", BatchStrategy::Chain), "chain");
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", PlannerPhase::Gather), "Gather");
        assert_eq!(format!("{}", PlannerPhase::Decompose), "Decompose");
        assert_eq!(format!("{}", PlannerPhase::Batch), "Batch");
        assert_eq!(format!("{}", PlannerPhase::Review), "Review");
        assert_eq!(format!("{}", PlannerPhase::Document), "Document");
        assert_eq!(format!("{}", PlannerPhase::Done), "Done");
    }

    #[test]
    fn test_risk_likelihood_display() {
        assert_eq!(format!("{}", RiskLikelihood::Low), "low");
        assert_eq!(format!("{}", RiskLikelihood::Medium), "medium");
        assert_eq!(format!("{}", RiskLikelihood::High), "high");
    }

    #[test]
    fn test_risk_impact_display() {
        assert_eq!(format!("{}", RiskImpact::Low), "low");
        assert_eq!(format!("{}", RiskImpact::Medium), "medium");
        assert_eq!(format!("{}", RiskImpact::High), "high");
    }

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = PlannerSkill::skill_prompt();
        assert!(prompt.contains("Planner Skill"));
        assert!(prompt.contains("Phase 1: Gather"));
        assert!(prompt.contains("Phase 5: Document"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Build Auth System"), "build-auth-system");
        assert_eq!(slugify("hello_world"), "hello-world");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn test_validation_issue_severity_display() {
        assert_eq!(format!("{}", ValidationSeverity::Error), "error");
        assert_eq!(format!("{}", ValidationSeverity::Warning), "warning");
    }
}
