//! Autonomous development loop skill for oxi
//!
//! A fully autonomous, recursive development cycle that runs
//! **Design → Plan → Implement → Verify → Fix** in a loop until zero
//! *genuine* issues remain. The operator issues one command and gets a
//! finished, verified, committed result.
//!
//! This module provides:
//! - [`AutonomousLoop`] — state machine that tracks loop iterations, phases,
//!   batches, issues, and verification results.
//! - [`LoopTask`] / [`TaskBatch`] — structured task and batch definitions
//!   with dependency tracking.
//! - [`Issue`] / [`IssueSeverity`] / [`IssueVerdict`] — issue tracking with
//!   severity classification and false-positive filtering.
//! - [`LoopPhase`] — phase enum for the six phases of the loop.
//! - [`AutonomousLoopSkill`] — skill content generator that produces the
//!   system-prompt instructions for the LLM-driven autonomous workflow.
//! - [`LoopStatus`] — serializable status snapshot for diagnostics.

use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

// ── Constants ──────────────────────────────────────────────────────────

/// Maximum number of full loop iterations before forced stop.
pub const MAX_ITERATIONS: u8 = 8;

// ── Phase ──────────────────────────────────────────────────────────────

/// The phases of the autonomous development loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhase {
    /// Understand requirements and produce a clear design.
    Design,
    /// Decompose into ordered, verifiable implementation steps.
    Plan,
    /// Execute the plan by batch, parallelizing independent tasks.
    Implement,
    /// Multi-axis verification that catches real problems.
    Verify,
    /// Confirm that every issue found is a REAL issue.
    ReValidate,
    /// Fix only the confirmed, genuine issues.
    Fix,
    /// Session concluded — all criteria met.
    Done,
}

impl Default for LoopPhase {
    fn default() -> Self {
        LoopPhase::Design
    }
}

impl fmt::Display for LoopPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoopPhase::Design => write!(f, "DESIGN"),
            LoopPhase::Plan => write!(f, "PLAN"),
            LoopPhase::Implement => write!(f, "IMPLEMENT"),
            LoopPhase::Verify => write!(f, "VERIFY"),
            LoopPhase::ReValidate => write!(f, "RE-VALIDATE"),
            LoopPhase::Fix => write!(f, "FIX"),
            LoopPhase::Done => write!(f, "DONE"),
        }
    }
}

impl LoopPhase {
    /// Return all phases in loop order.
    pub fn all() -> &'static [LoopPhase] {
        &[
            LoopPhase::Design,
            LoopPhase::Plan,
            LoopPhase::Implement,
            LoopPhase::Verify,
            LoopPhase::ReValidate,
            LoopPhase::Fix,
            LoopPhase::Done,
        ]
    }

    /// Advance to the next phase in the standard sequence.
    ///
    /// The loop flow is:
    /// ```text
    /// Design → Plan → Implement → Verify
    ///   ↑                              │
    ///   └── Fix ← ReValidate ←────────┘
    ///                                    ↘ Done (no issues)
    /// ```
    pub fn next(&self) -> Option<LoopPhase> {
        match self {
            LoopPhase::Design => Some(LoopPhase::Plan),
            LoopPhase::Plan => Some(LoopPhase::Implement),
            LoopPhase::Implement => Some(LoopPhase::Verify),
            LoopPhase::Verify => Some(LoopPhase::ReValidate),
            LoopPhase::ReValidate => Some(LoopPhase::Fix),
            LoopPhase::Fix => Some(LoopPhase::Verify), // back to verify after fix
            LoopPhase::Done => None,
        }
    }

    /// Returns `true` if this phase should proceed to `Done` when no issues
    /// are found (as opposed to continuing the loop).
    pub fn can_exit_on_clean(&self) -> bool {
        matches!(self, LoopPhase::Verify | LoopPhase::ReValidate)
    }
}

// ── Task and Batch ─────────────────────────────────────────────────────

/// Status of a task or batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Successfully completed.
    Done,
    /// Failed with an error.
    Failed,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::Running => write!(f, "running"),
            TaskStatus::Done => write!(f, "done"),
            TaskStatus::Failed => write!(f, "failed"),
        }
    }
}

/// A single task in the implementation plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopTask {
    /// Unique task identifier (e.g., "T1", "T2").
    pub id: String,
    /// Short description of what this task accomplishes.
    pub description: String,
    /// File paths this task creates or modifies.
    pub touches_files: Vec<PathBuf>,
    /// IDs of tasks this task depends on.
    pub depends_on: Vec<String>,
    /// How to verify this task works.
    pub verification: String,
    /// Current status.
    pub status: TaskStatus,
    /// Commit hash after completion, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
}

impl LoopTask {
    /// Create a new task with the given ID and description.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            touches_files: Vec::new(),
            depends_on: Vec::new(),
            verification: String::new(),
            status: TaskStatus::Pending,
            commit_hash: None,
        }
    }

    /// Add a file this task touches.
    pub fn touches(mut self, path: impl Into<PathBuf>) -> Self {
        self.touches_files.push(path.into());
        self
    }

    /// Declare a dependency on another task.
    pub fn depends_on(mut self, task_id: impl Into<String>) -> Self {
        self.depends_on.push(task_id.into());
        self
    }

    /// Set verification method.
    pub fn verify_with(mut self, method: impl Into<String>) -> Self {
        self.verification = method.into();
        self
    }

    /// Mark the task as running.
    pub fn start(&mut self) {
        self.status = TaskStatus::Running;
    }

    /// Mark the task as completed with an optional commit hash.
    pub fn complete(&mut self, commit_hash: Option<String>) {
        self.status = TaskStatus::Done;
        self.commit_hash = commit_hash;
    }

    /// Mark the task as failed.
    pub fn fail(&mut self) {
        self.status = TaskStatus::Failed;
    }
}

/// A batch of tasks that can be executed in parallel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBatch {
    /// Batch index (0-based).
    pub index: usize,
    /// Tasks in this batch.
    pub tasks: Vec<LoopTask>,
    /// Whether tasks in this batch have file conflicts (must run sequentially).
    pub has_conflicts: bool,
    /// Current status of the batch.
    pub status: TaskStatus,
}

impl TaskBatch {
    /// Create a new batch at the given index.
    pub fn new(index: usize) -> Self {
        Self {
            index,
            tasks: Vec::new(),
            has_conflicts: false,
            status: TaskStatus::Pending,
        }
    }

    /// Add a task to this batch.
    pub fn add_task(&mut self, task: LoopTask) {
        self.tasks.push(task);
    }

    /// Mark all tasks as running.
    pub fn start(&mut self) {
        self.status = TaskStatus::Running;
        for task in &mut self.tasks {
            task.start();
        }
    }

    /// Mark the batch as completed.
    pub fn complete(&mut self) {
        self.status = TaskStatus::Done;
    }

    /// Check if all tasks in this batch are done.
    pub fn all_done(&self) -> bool {
        self.tasks.iter().all(|t| t.status == TaskStatus::Done)
    }

    /// Check if any task in this batch failed.
    pub fn any_failed(&self) -> bool {
        self.tasks.iter().any(|t| t.status == TaskStatus::Failed)
    }
}

// ── Issue tracking ─────────────────────────────────────────────────────

/// Severity of an issue found during verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    /// Formatting, naming preference — optional.
    Nit = 0,
    /// Style inconsistency, missing edge case — should fix.
    Minor = 1,
    /// Incorrect behavior, failing tests, broken feature — must fix.
    Important = 2,
    /// Build broken, data loss, security vulnerability — must fix now.
    Critical = 3,
}

impl fmt::Display for IssueSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueSeverity::Nit => write!(f, "Nit"),
            IssueSeverity::Minor => write!(f, "Minor"),
            IssueSeverity::Important => write!(f, "Important"),
            IssueSeverity::Critical => write!(f, "Critical"),
        }
    }
}

/// Verdict after re-validating an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueVerdict {
    /// Real issue — proceed to fix.
    Confirmed,
    /// Not actually a problem — discard.
    FalsePositive,
    /// Real but out of scope — log for future.
    Deferred,
    /// Cannot determine — needs user input.
    NeedsContext,
}

impl fmt::Display for IssueVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueVerdict::Confirmed => write!(f, "CONFIRMED"),
            IssueVerdict::FalsePositive => write!(f, "FALSE_POSITIVE"),
            IssueVerdict::Deferred => write!(f, "DEFERRED"),
            IssueVerdict::NeedsContext => write!(f, "NEEDS_CONTEXT"),
        }
    }
}

/// An issue found during verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    /// Issue number (1-based, for display).
    pub number: usize,
    /// One-line description.
    pub description: String,
    /// Severity.
    pub severity: IssueSeverity,
    /// Location (file:line or component name).
    pub location: String,
    /// Evidence (error message, test output, or concrete observation).
    pub evidence: String,
    /// Whether the issue is reproducible.
    pub reproducible: bool,
    /// Suggested fix approach.
    pub fix_approach: String,
    /// Verdict after re-validation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<IssueVerdict>,
    /// Reason for the verdict (especially for false positives).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict_reason: Option<String>,
    /// Whether this issue has been fixed.
    pub fixed: bool,
    /// Commit hash of the fix, if applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_commit: Option<String>,
}

impl Issue {
    /// Create a new issue.
    pub fn new(
        number: usize,
        description: impl Into<String>,
        severity: IssueSeverity,
        location: impl Into<String>,
    ) -> Self {
        Self {
            number,
            description: description.into(),
            severity,
            location: location.into(),
            evidence: String::new(),
            reproducible: false,
            fix_approach: String::new(),
            verdict: None,
            verdict_reason: None,
            fixed: false,
            fix_commit: None,
        }
    }

    /// Set evidence for this issue.
    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = evidence.into();
        self
    }

    /// Set whether the issue is reproducible.
    pub fn reproducible(mut self, yes: bool) -> Self {
        self.reproducible = yes;
        self
    }

    /// Set the fix approach.
    pub fn fix_approach(mut self, approach: impl Into<String>) -> Self {
        self.fix_approach = approach.into();
        self
    }

    /// Record a verdict after re-validation.
    pub fn set_verdict(&mut self, verdict: IssueVerdict, reason: impl Into<String>) {
        self.verdict = Some(verdict);
        self.verdict_reason = Some(reason.into());
    }

    /// Mark the issue as fixed.
    pub fn mark_fixed(&mut self, commit_hash: Option<String>) {
        self.fixed = true;
        self.fix_commit = commit_hash;
    }

    /// Whether this issue must be fixed (confirmed and not yet fixed).
    pub fn needs_fix(&self) -> bool {
        self.verdict == Some(IssueVerdict::Confirmed) && !self.fixed
    }

    /// Whether this issue is actionable (confirmed or needs context, not fixed).
    pub fn is_actionable(&self) -> bool {
        matches!(
            self.verdict,
            Some(IssueVerdict::Confirmed) | Some(IssueVerdict::NeedsContext)
        ) && !self.fixed
    }
}

// ── Verification result ───────────────────────────────────────────────

/// Result of a verification pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether the build succeeded.
    pub build_passed: bool,
    /// Whether all tests passed.
    pub tests_passed: bool,
    /// Whether type checking passed.
    pub type_check_passed: bool,
    /// Whether linting passed.
    pub lint_passed: bool,
    /// Issues found during this verification pass.
    pub issues: Vec<Issue>,
    /// Timestamp of this verification.
    pub timestamp: String,
}

impl VerificationResult {
    /// Create a new verification result.
    pub fn new() -> Self {
        Self {
            build_passed: false,
            tests_passed: false,
            type_check_passed: false,
            lint_passed: false,
            issues: Vec::new(),
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Whether all checks passed with no issues.
    pub fn is_clean(&self) -> bool {
        self.build_passed
            && self.tests_passed
            && self.type_check_passed
            && self.lint_passed
            && self.issues.is_empty()
    }

    /// Whether the critical gates passed (build + tests).
    pub fn critical_passed(&self) -> bool {
        self.build_passed && self.tests_passed
    }

    /// Count issues by severity.
    pub fn issue_count_by_severity(&self, severity: IssueSeverity) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == severity)
            .count()
    }

    /// Count confirmed issues that still need fixing.
    pub fn confirmed_unfixed(&self) -> usize {
        self.issues.iter().filter(|i| i.needs_fix()).count()
    }
}

impl Default for VerificationResult {
    fn default() -> Self {
        Self::new()
    }
}

// ── Loop status snapshot ───────────────────────────────────────────────

/// A serializable snapshot of the autonomous loop state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopStatus {
    /// The task being executed.
    pub task: String,
    /// Current iteration number (1-based).
    pub iteration: u8,
    /// Maximum iterations allowed.
    pub max_iterations: u8,
    /// Current phase.
    pub phase: LoopPhase,
    /// Execution batches.
    pub batches: Vec<TaskBatch>,
    /// Issues found across all iterations.
    pub issues: Vec<Issue>,
    /// Latest verification result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_verification: Option<VerificationResult>,
    /// Most recent commit hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit: Option<String>,
    /// Whether git working tree is clean.
    pub git_clean: bool,
    /// Any blocking condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
    /// Timestamp of this status snapshot.
    pub timestamp: String,
}

impl LoopStatus {
    /// Render the status as a formatted string for display.
    pub fn render(&self) -> String {
        let mut s = String::with_capacity(2048);

        s.push_str("AUTONOMOUS LOOP STATUS\n");
        s.push_str("═══════════════════════\n");
        s.push_str(&format!("Task: {}\n", self.task));
        s.push_str(&format!(
            "Iteration: {} / {}\n",
            self.iteration, self.max_iterations
        ));
        s.push_str(&format!("Phase: {}\n", self.phase));

        // Batch summary
        let done_count = self
            .batches
            .iter()
            .filter(|b| b.status == TaskStatus::Done)
            .count();
        let total_count = self.batches.len();
        s.push_str(&format!("Batches: {} / {} done\n", done_count, total_count));
        for batch in &self.batches {
            let task_ids: Vec<&str> = batch.tasks.iter().map(|t| t.id.as_str()).collect();
            let mode = if batch.has_conflicts {
                "sequential"
            } else {
                "parallel"
            };
            s.push_str(&format!(
                "  Batch {}: [{}] ({}) — {}\n",
                batch.index,
                task_ids.join(", "),
                mode,
                batch.status
            ));
        }

        // Issue summary
        let total = self.issues.len();
        let confirmed = self
            .issues
            .iter()
            .filter(|i| i.verdict == Some(IssueVerdict::Confirmed))
            .count();
        let fixed = self.issues.iter().filter(|i| i.fixed).count();
        s.push_str(&format!(
            "Issues: {} found → {} confirmed → {} fixed\n",
            total, confirmed, fixed
        ));

        // Progress bar
        let pct = if total_count > 0 {
            (done_count * 10) / total_count
        } else {
            0
        };
        let filled: String = "▓".repeat(pct);
        let empty: String = "░".repeat(10 - pct);
        s.push_str(&format!("Progress: {}{} \n", filled, empty));

        // Commit and git status
        if let Some(ref hash) = self.last_commit {
            s.push_str(&format!("Last commit: {}\n", &hash[..7.min(hash.len())]));
        }
        s.push_str(&format!(
            "Git status: {}\n",
            if self.git_clean { "clean" } else { "dirty" }
        ));

        // Blocker
        if let Some(ref blocker) = self.blocker {
            s.push_str(&format!("Blocks: {}\n", blocker));
        }

        s
    }
}

// ── Autonomous Loop state machine ──────────────────────────────────────

/// The autonomous development loop state machine.
///
/// Tracks the full lifecycle of an autonomous development task across
/// multiple iterations of the Design → Plan → Implement → Verify →
/// ReValidate → Fix loop.
///
/// # Usage
///
/// ```rust,ignore
/// let mut al = AutonomousLoop::new("Implement user authentication");
/// al.start()?;
///
/// // Design phase
/// al.advance()?; // → Plan
///
/// // Plan phase — add tasks and compute batches
/// al.add_task(LoopTask::new("T1", "Create auth module").touches("src/auth.rs"));
/// al.add_task(LoopTask::new("T2", "Add login route").depends_on("T1"));
/// al.compute_batches()?;
/// al.advance()?; // → Implement
///
/// // ... execute batches ...
/// al.advance()?; // → Verify
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousLoop {
    /// Unique loop instance ID.
    pub id: String,
    /// The task description.
    pub task: String,
    /// Current iteration (1-based).
    pub iteration: u8,
    /// Maximum iterations allowed.
    pub max_iterations: u8,
    /// Current phase.
    pub phase: LoopPhase,
    /// Whether the loop has been started.
    pub started: bool,
    /// Whether the loop has been forcefully stopped.
    pub emergency_stopped: bool,
    /// All tasks in the plan.
    pub tasks: Vec<LoopTask>,
    /// Computed execution batches.
    pub batches: Vec<TaskBatch>,
    /// Issues found across all iterations.
    pub issues: Vec<Issue>,
    /// Latest verification result.
    pub last_verification: Option<VerificationResult>,
    /// Most recent commit hash.
    pub last_commit: Option<String>,
    /// Whether the git working tree is clean.
    pub git_clean: bool,
    /// Any blocking condition preventing progress.
    pub blocker: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

impl AutonomousLoop {
    /// Create a new autonomous loop for the given task.
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task: task.into(),
            iteration: 0,
            max_iterations: MAX_ITERATIONS,
            phase: LoopPhase::Design,
            started: false,
            emergency_stopped: false,
            tasks: Vec::new(),
            batches: Vec::new(),
            issues: Vec::new(),
            last_verification: None,
            last_commit: None,
            git_clean: true,
            blocker: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    /// Set the maximum number of iterations.
    pub fn with_max_iterations(mut self, max: u8) -> Self {
        self.max_iterations = max.min(MAX_ITERATIONS).max(1);
        self
    }

    /// Start the loop (transitions to iteration 1, Design phase).
    pub fn start(&mut self) -> Result<()> {
        if self.started {
            bail!("Loop already started");
        }
        if self.emergency_stopped {
            bail!("Loop was emergency-stopped and cannot be restarted");
        }
        self.started = true;
        self.iteration = 1;
        self.phase = LoopPhase::Design;
        self.touch();
        Ok(())
    }

    /// Emergency stop — halts the loop immediately.
    pub fn emergency_stop(&mut self, reason: impl Into<String>) {
        self.emergency_stopped = true;
        self.blocker = Some(reason.into());
        self.touch();
    }

    /// Advance to the next phase.
    ///
    /// Follows the standard loop flow:
    /// - After ReValidate with no confirmed issues → Done
    /// - After ReValidate with confirmed issues → Fix → Verify (new iteration)
    /// - After Verify with no issues → Done
    /// - After Verify with issues → ReValidate
    pub fn advance(&mut self) -> Result<LoopPhase> {
        if !self.started {
            bail!("Loop has not been started");
        }
        if self.emergency_stopped {
            bail!(
                "Loop was emergency-stopped: {}",
                self.blocker.as_deref().unwrap_or("unknown")
            );
        }

        let next = match self.phase {
            LoopPhase::Design => Some(LoopPhase::Plan),
            LoopPhase::Plan => Some(LoopPhase::Implement),
            LoopPhase::Implement => Some(LoopPhase::Verify),

            LoopPhase::Verify => {
                // If clean, we're done
                if self.is_clean() {
                    Some(LoopPhase::Done)
                } else {
                    Some(LoopPhase::ReValidate)
                }
            }

            LoopPhase::ReValidate => {
                let has_confirmed = self.issues.iter().any(|i| i.needs_fix());
                if has_confirmed {
                    Some(LoopPhase::Fix)
                } else {
                    // All issues are false positives or deferred — done!
                    Some(LoopPhase::Done)
                }
            }

            LoopPhase::Fix => {
                // After fixing, increment iteration and go back to verify
                self.iteration += 1;
                if self.iteration > self.max_iterations {
                    self.emergency_stop(format!(
                        "Maximum iterations ({}) reached",
                        self.max_iterations
                    ));
                    bail!(
                        "Maximum iterations ({}) reached. Diagnostic:\n{}",
                        self.max_iterations,
                        self.diagnostic()
                    );
                }
                Some(LoopPhase::Verify)
            }

            LoopPhase::Done => {
                bail!("Loop is already complete");
            }
        };

        if let Some(phase) = next {
            self.phase = phase;
            self.touch();
            Ok(phase)
        } else {
            bail!("No valid next phase from {:?}", self.phase);
        }
    }

    /// Jump to a specific phase (for recovery or testing).
    pub fn set_phase(&mut self, phase: LoopPhase) {
        self.phase = phase;
        self.touch();
    }

    // ── Task management ─────────────────────────────────────────────

    /// Add a task to the plan.
    pub fn add_task(&mut self, task: LoopTask) {
        self.tasks.push(task);
        self.touch();
    }

    /// Get a task by ID.
    pub fn get_task(&self, id: &str) -> Option<&LoopTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Get a mutable reference to a task by ID.
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut LoopTask> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    // ── Batch computation ───────────────────────────────────────────

    /// Compute execution batches from the current task list.
    ///
    /// Groups tasks into batches using topological ordering. Each batch
    /// contains tasks whose dependencies are all in *earlier* batches.
    /// Tasks within a batch that touch overlapping files are flagged
    /// as having conflicts.
    ///
    /// Uses Kahn's algorithm for topological sorting, grouping tasks
    /// by their dependency depth level.
    pub fn compute_batches(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            self.batches.clear();
            return Ok(());
        }

        use std::collections::{HashMap, HashSet, VecDeque};

        // Build adjacency graph
        let task_ids: HashSet<&str> = self.tasks.iter().map(|t| t.id.as_str()).collect();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new(); // task -> tasks that depend on it

        for task in &self.tasks {
            in_degree.entry(task.id.as_str()).or_insert(0);
            dependents.entry(task.id.as_str()).or_insert_with(Vec::new);

            for dep in &task.depends_on {
                if !task_ids.contains(dep.as_str()) {
                    bail!(
                        "Task '{}' depends on '{}' which does not exist",
                        task.id,
                        dep
                    );
                }
                *in_degree.entry(task.id.as_str()).or_insert(0) += 1;
                dependents
                    .entry(dep.as_str())
                    .or_insert_with(Vec::new)
                    .push(task.id.as_str());
            }
        }

        // Kahn's algorithm with level tracking
        let mut queue: VecDeque<(&str, usize)> = VecDeque::new(); // (task_id, level)
        for task in &self.tasks {
            if task.depends_on.is_empty() {
                queue.push_back((task.id.as_str(), 0));
            }
        }

        let mut levels: HashMap<&str, usize> = HashMap::new();
        let mut processed: HashSet<&str> = HashSet::new();

        while let Some((task_id, level)) = queue.pop_front() {
            if processed.contains(task_id) {
                continue;
            }
            processed.insert(task_id);
            levels.insert(task_id, level);

            // Decrease in-degree of dependents
            if let Some(deps) = dependents.get(task_id) {
                for &dep_id in deps {
                    let deg = in_degree.get_mut(dep_id).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back((dep_id, level + 1));
                    }
                }
            }
        }

        // Check for circular dependencies
        if processed.len() != self.tasks.len() {
            let unassigned: Vec<&str> = self
                .tasks
                .iter()
                .filter(|t| !processed.contains(t.id.as_str()))
                .map(|t| t.id.as_str())
                .collect();
            bail!(
                "Cannot compute batches: circular dependency detected. Unassigned tasks: {:?}",
                unassigned
            );
        }

        // Group tasks by level into batches
        let max_level = levels.values().copied().max().unwrap_or(0);
        let mut batches: Vec<TaskBatch> = Vec::new();

        for level in 0..=max_level {
            let batch_idx = level;
            let mut batch = TaskBatch::new(batch_idx);

            for task in &self.tasks {
                let task_level = levels.get(task.id.as_str()).copied().unwrap_or(0);
                if task_level != level {
                    continue;
                }

                // Check for file conflicts with tasks already in this batch
                let has_conflict = batch.tasks.iter().any(|bt| {
                    let bt_files: HashSet<_> = bt.touches_files.iter().collect();
                    task.touches_files.iter().any(|f| bt_files.contains(f))
                });

                if has_conflict {
                    batch.has_conflicts = true;
                }

                batch.add_task(task.clone());
            }

            if !batch.tasks.is_empty() {
                batches.push(batch);
            }
        }

        self.batches = batches;
        self.touch();
        Ok(())
    }

    /// Get the next pending batch, if any.
    pub fn next_pending_batch(&self) -> Option<&TaskBatch> {
        self.batches
            .iter()
            .find(|b| b.status == TaskStatus::Pending)
    }

    /// Get a mutable reference to a batch by index.
    pub fn get_batch_mut(&mut self, index: usize) -> Option<&mut TaskBatch> {
        self.batches.get_mut(index)
    }

    /// Count completed batches.
    pub fn completed_batch_count(&self) -> usize {
        self.batches
            .iter()
            .filter(|b| b.status == TaskStatus::Done)
            .count()
    }

    /// Count total batches.
    pub fn total_batch_count(&self) -> usize {
        self.batches.len()
    }

    // ── Issue management ────────────────────────────────────────────

    /// Add an issue found during verification.
    pub fn add_issue(&mut self, issue: Issue) {
        self.issues.push(issue);
        self.touch();
    }

    /// Get all confirmed, unfixed issues.
    pub fn confirmed_issues(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| i.needs_fix()).collect()
    }

    /// Count issues by verdict.
    pub fn issues_by_verdict(&self, verdict: IssueVerdict) -> usize {
        self.issues
            .iter()
            .filter(|i| i.verdict == Some(verdict))
            .count()
    }

    /// Count fixed issues.
    pub fn fixed_issue_count(&self) -> usize {
        self.issues.iter().filter(|i| i.fixed).count()
    }

    // ── Verification ────────────────────────────────────────────────

    /// Record a verification result.
    pub fn record_verification(&mut self, result: VerificationResult) {
        // Merge issues into the global list
        let next_number = self.issues.len() + 1;
        for (i, mut issue) in result.issues.into_iter().enumerate() {
            issue.number = next_number + i;
            self.issues.push(issue);
        }
        // Store the result (without the moved issues)
        let mut stored = VerificationResult {
            issues: Vec::new(), // issues are in the global list now
            ..result
        };
        stored.issues = self
            .issues
            .iter()
            .filter(|i| i.number >= next_number)
            .cloned()
            .collect();
        self.last_verification = Some(stored);
        self.touch();
    }

    /// Whether the current state is "clean" (no unfixed confirmed issues,
    /// all verification gates passed).
    pub fn is_clean(&self) -> bool {
        let no_unfixed = !self.issues.iter().any(|i| i.needs_fix());
        let verify_ok = self
            .last_verification
            .as_ref()
            .map(|v| v.is_clean())
            .unwrap_or(false);
        no_unfixed && verify_ok
    }

    // ── Git integration ─────────────────────────────────────────────

    /// Record a commit hash.
    pub fn record_commit(&mut self, hash: impl Into<String>) {
        self.last_commit = Some(hash.into());
        self.touch();
    }

    /// Set the git clean status.
    pub fn set_git_clean(&mut self, clean: bool) {
        self.git_clean = clean;
        self.touch();
    }

    // ── Status and diagnostics ──────────────────────────────────────

    /// Produce a serializable status snapshot.
    pub fn status(&self) -> LoopStatus {
        LoopStatus {
            task: self.task.clone(),
            iteration: self.iteration,
            max_iterations: self.max_iterations,
            phase: self.phase,
            batches: self.batches.clone(),
            issues: self.issues.clone(),
            last_verification: self.last_verification.clone(),
            last_commit: self.last_commit.clone(),
            git_clean: self.git_clean,
            blocker: self.blocker.clone(),
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Produce a diagnostic report when the loop hits max iterations or
    /// emergency stop.
    pub fn diagnostic(&self) -> String {
        let mut s = String::with_capacity(4096);

        s.push_str("═══ AUTONOMOUS LOOP DIAGNOSTIC ═══\n\n");
        s.push_str(&format!("Task: {}\n", self.task));
        s.push_str(&format!(
            "Iterations used: {} / {}\n",
            self.iteration, self.max_iterations
        ));
        s.push_str(&format!("Phase at stop: {}\n", self.phase));

        if let Some(ref blocker) = self.blocker {
            s.push_str(&format!("Blocker: {}\n", blocker));
        }

        s.push_str(&format!("Emergency stopped: {}\n", self.emergency_stopped));

        // Batch progress
        s.push_str("\n── Batches ──\n");
        for batch in &self.batches {
            let task_ids: Vec<&str> = batch.tasks.iter().map(|t| t.id.as_str()).collect();
            s.push_str(&format!(
                "  Batch {}: [{}] — {}\n",
                batch.index,
                task_ids.join(", "),
                batch.status
            ));
            for task in &batch.tasks {
                s.push_str(&format!(
                    "    {}: {} [{}]\n",
                    task.id, task.description, task.status
                ));
            }
        }

        // Issue summary
        s.push_str("\n── Issues ──\n");
        let total = self.issues.len();
        let confirmed = self.issues_by_verdict(IssueVerdict::Confirmed);
        let false_pos = self.issues_by_verdict(IssueVerdict::FalsePositive);
        let deferred = self.issues_by_verdict(IssueVerdict::Deferred);
        let fixed = self.fixed_issue_count();
        s.push_str(&format!(
            "  Total: {} | Confirmed: {} | False positives: {} | Deferred: {} | Fixed: {}\n",
            total, confirmed, false_pos, deferred, fixed
        ));

        // Recurring issues
        s.push_str("\n── Unfixed Confirmed Issues ──\n");
        for issue in self.issues.iter().filter(|i| i.needs_fix()) {
            s.push_str(&format!(
                "  #{} [{}] {} — {}\n",
                issue.number, issue.severity, issue.description, issue.location
            ));
            if !issue.evidence.is_empty() {
                s.push_str(&format!("    Evidence: {}\n", issue.evidence));
            }
            if !issue.fix_approach.is_empty() {
                s.push_str(&format!("    Fix approach: {}\n", issue.fix_approach));
            }
        }

        // Verification history
        if let Some(ref v) = self.last_verification {
            s.push_str("\n── Last Verification ──\n");
            s.push_str(&format!(
                "  Build: {}\n",
                if v.build_passed { "✅" } else { "❌" }
            ));
            s.push_str(&format!(
                "  Tests: {}\n",
                if v.tests_passed { "✅" } else { "❌" }
            ));
            s.push_str(&format!(
                "  Type check: {}\n",
                if v.type_check_passed { "✅" } else { "❌" }
            ));
            s.push_str(&format!(
                "  Lint: {}\n",
                if v.lint_passed { "✅" } else { "❌" }
            ));
        }

        s.push('\n');
        s
    }

    /// Update the timestamp.
    fn touch(&mut self) {
        self.updated_at = Utc::now().to_rfc3339();
    }
}

// ── Skill prompt ───────────────────────────────────────────────────────

/// The autonomous-loop skill content generator.
///
/// Produces the system-prompt instructions that guide the LLM through the
/// autonomous development workflow.
pub struct AutonomousLoopSkill;

impl AutonomousLoopSkill {
    /// Generate the full skill instructions to be injected into the system
    /// prompt when the autonomous-loop skill is active.
    pub fn skill_instructions() -> String {
        let prompt = r#"# Autonomous Development Loop Skill

You are operating the **autonomous-loop** skill. Your goal is to execute a
fully autonomous development cycle that produces a finished, verified, and
committed result from a single task description.

## Core Principles

1. **Never stop until genuinely done.** No "I think this looks good, please review" — keep going until verification gates pass clean.
2. **Every finding must survive cross-examination.** A bug is only a bug if it can be proven. An issue is only an issue if it can be demonstrated.
3. **Every checkpoint is a save point.** Git commits at every stable state mean any step is reversible.
4. **TDD for logic.** When implementing logic, algorithms, or data transformations — write the failing test first. When implementing UI layout or configuration, TDD is optional.

## Maximum Iterations

The loop runs at most **8 full iterations**. If still failing after 8 iterations, stop and produce a diagnostic report explaining what went wrong.

## Loop Phases

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                 │
│  1. DESIGN ──── 2. PLAN ──── 3. IMPLEMENT ──── 4. VERIFY       │
│                                                  │              │
│                                                  ▼              │
│                                           Issues found?         │
│                                           ┌─────┴─────┐        │
│                                           │ YES       │ NO     │
│                                           ▼           ▼        │
│                                     5. RE-VALIDATE   7. DONE   │
│                                           │                     │
│                                     Real issues?                │
│                                     ┌────┴────┐                │
│                                     │ YES     │ NO (false +)   │
│                                     ▼         ▼                │
│                                   6. FIX    Discard,           │
│                                     │       re-verify           │
│                                     ▼                           │
│                              Commit fix ──→ back to 4           │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Phase 1: DESIGN

**Goal:** Understand requirements and produce a clear design before touching code.

### Design Quality Gate

Before proceeding, evaluate:

- [ ] Spec or design doc exists for this feature?
- [ ] Design is up-to-date with current codebase state?
- [ ] Approach is defined with specific files to touch?
- [ ] Acceptance criteria are concrete and testable?
- [ ] No known gaps or ambiguities in requirements?

**If ANY of these are "no":** Stop. Use the deep-research skill to investigate
and produce a solid design before continuing.

### Steps

1. Read all relevant context — specs, existing code, AGENTS.md, project conventions
2. If no spec/design exists, produce a minimal design doc
3. If a design already exists, validate it against the current codebase state
4. Identify risks and unknowns
5. **Commit checkpoint** if you created or updated a design doc

### Exit Criteria

- [ ] Objective is clear and testable
- [ ] Approach is defined
- [ ] Files to touch are identified
- [ ] Acceptance criteria are concrete

## Phase 2: PLAN

**Goal:** Decompose into ordered, verifiable implementation steps.

### Steps

1. Break into vertical slices — each slice delivers a working, testable increment
2. Order by dependency — foundations first, consumers last
3. Each task must have:
   - Task ID (e.g., T1, T2)
   - Exact file paths
   - What it accomplishes
   - How to verify it works
   - `dependsOn` — list of task IDs
   - `touchesFiles` — files this task creates or modifies
4. Group tasks into parallel execution batches
5. Mark commit points — commit after each batch completes

### Exit Criteria

- [ ] Every task has acceptance criteria
- [ ] Every task has a verification method
- [ ] Every task has dependsOn and touchesFiles
- [ ] Tasks are grouped into execution batches
- [ ] No circular dependencies
- [ ] No task exceeds ~5 files

## Phase 3: IMPLEMENT

**Goal:** Execute the plan by batch, parallelizing independent tasks, with commits at every stable point.

### Rules

**Rule 0: Simplicity First.** Before writing code: "What is the simplest thing that could work?"

**Rule 1: Batch Execution.** Execute tasks by batch, respecting the dependency graph.

**Rule 2: Build Must Stay Green.** After each batch: build compiles, existing tests pass.

**Rule 3: Scope Discipline.** Touch only what the task requires. No unsolicited refactoring.

**Rule 4: Commit Frequently.** After every successful batch:
```
git commit -m "<type>(<scope>): <what this batch accomplishes>"
```

### Commit Message Format

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`
Scopes: match the module/area being changed

Examples:
- ✅ `feat(auth): add JWT token generation`
- ✅ `test(cache): add LRU eviction tests`
- ❌ `feat: implement phase 1` — too coarse

### Safety Protocol

At the START of implementation:
```bash
git add -A && git commit -m "chore: checkpoint before <feature> implementation"
```

## Phase 4: VERIFY

**Goal:** Multi-axis verification that catches real problems.

### Steps

1. Run build, test, lint:
   ```bash
   npm run build && npm test && npm run lint
   # or: cargo build && cargo test && cargo clippy
   ```
   - [ ] Build succeeds with zero errors
   - [ ] All tests pass (existing + new)
   - [ ] Zero type/lint errors

2. Walk through acceptance criteria from Phase 2:
   - [ ] Every acceptance criterion is met
   - [ ] Edge cases handled
   - [ ] Error paths handled

3. **Log any issues found:**
   ```
   ISSUE [N]: [one-line description]
     Severity: Critical | Important | Minor | Nit
     Location: file:line or component
     Evidence: [exact error message or concrete observation]
     Reproducible: YES/NO
     Fix approach: [brief description]
   ```

**Severity definitions:**
- **Critical:** Build broken, data loss, security vulnerability — must fix
- **Important:** Incorrect behavior, failing tests, broken feature — must fix
- **Minor:** Style inconsistency, missing edge case — should fix
- **Nit:** Formatting, naming preference — optional

## Phase 5: RE-VALIDATE (The False Positive Filter)

**Goal:** Confirm that every issue found in Phase 4 is a REAL issue.

### For EVERY issue:

**Step 1: Reproduce or Demonstrate**
- Build error? → Re-read the exact error message and the code
- Test failure? → Re-run the specific failing test in isolation
- Logic bug? → Trace the data flow: input → wrong output

**Step 2: Cross-Examine** — if ANY answer is "no," it's likely a false positive:

| Question | Why it matters |
|----------|---------------|
| Does this violate a project convention? | Many "issues" are intentional styles |
| Is this actually in scope? | Adjacent code may look "wrong" but isn't this change |
| Would a staff engineer flag this? | Distinguishes real from theoretical |
| Is the "correct" version actually better here? | Context-dependent patterns exist |
| Does this affect actual behavior? | Theoretical issues waste time |

**Step 3: Verdict**

| Verdict | Action |
|---------|--------|
| **CONFIRMED** | Real issue → proceed to Phase 6 |
| **FALSE_POSITIVE** | Not a problem → discard, document why |
| **DEFERRED** | Real but out of scope → log, don't fix now |
| **NEEDS_CONTEXT** | Can't determine → ask the user |

### Common False Positive Patterns

- **Over-applying best practices** on internal-only functions
- **Misunderstanding intent** (variable "unused" but used in templates)
- **Generic rules vs project context** (project disables a linter rule intentionally)
- **Theoretical concerns** ("could be slow" with bounded data)
- **Adjacent code problems** outside task scope

## Phase 6: FIX (If CONFIRMED Issues Exist)

**Goal:** Fix only the confirmed, genuine issues.

### Rules

- **One fix per commit.** Each fix is independently revertable.
- **Fix the root cause, not the symptom.** Ask "why?" at least twice.
- **Re-run specific verification after each fix.**

After all fixes committed:
```bash
npm run build && npm test && npm run lint
```

Then **return to Phase 4 (VERIFY)** for a fresh pass.

## Phase 7: DONE

**Goal:** Final confirmation that the task is genuinely complete.

### Final Verification

- [ ] Build succeeds with zero errors
- [ ] Full test suite passes
- [ ] Type check passes
- [ ] Lint passes
- [ ] All acceptance criteria met
- [ ] No uncommitted changes (`git status` is clean)
- [ ] No TODO/FIXME/HACK that should have been resolved
- [ ] No debug logging left behind

### Completion Report

```
## Task Complete: [Task Name]

### Summary
[1-2 sentences]

### Changes
- [file list with one-line descriptions]

### Commits
[newest first]

### Verification
- Build: ✅ PASS
- Tests: ✅ PASS (N tests)
- Type check: ✅ PASS
- Lint: ✅ PASS

### Issues Found & Resolved
- [Issues confirmed and fixed]

### Discarded False Positives
- [Issues discarded with reasons]
```

## Emergency Stop Conditions

Stop immediately and report if:
- Build broken and can't fix within 2 attempts
- Tests failing and fix introduces new failures
- Hit 8 loop iterations
- Fundamental design flaw discovered
- Something genuinely not understood

## Anti-Rationalization Table

| Rationalization | Reality |
|---|---|
| "Build passes, probably good enough" | Build ≠ working. Tests + type checks + acceptance criteria matter. |
| "Mental review is sufficient" | Mental review misses the same bugs introduced. |
| "Minor issues can wait" | Minor issues compound into tomorrow's bugs. |
| "Skip re-validation, issue is obvious" | False positives waste hours. 5 min cross-examination saves 30 min. |
| "Commit everything at the end" | Catastrophic failure at min 45 = losing 45 min. |
| "These issues are all real" | When finding many issues at once, false positive rate is highest. |
| "Fix all issues at once" | Batch fixes hide which fix solved which issue. |
| "Improve nearby code while here" | Every unsolicited change is a risk. Stay in scope. |

## Red Flags (Self-Monitoring)

- Skipping re-validation because "the issue is obvious"
- Committing >100 lines without a build/test check
- Finding the same issue in iteration 3 that was "fixed" in iteration 2
- Rationalizing why a failed test "doesn't count"
- Broadening scope beyond the original task
- More than 3 consecutive fix-and-reverify cycles on the same issue
"#;
        prompt.to_string()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── LoopPhase tests ────────────────────────────────────────────

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", LoopPhase::Design), "DESIGN");
        assert_eq!(format!("{}", LoopPhase::Plan), "PLAN");
        assert_eq!(format!("{}", LoopPhase::Implement), "IMPLEMENT");
        assert_eq!(format!("{}", LoopPhase::Verify), "VERIFY");
        assert_eq!(format!("{}", LoopPhase::ReValidate), "RE-VALIDATE");
        assert_eq!(format!("{}", LoopPhase::Fix), "FIX");
        assert_eq!(format!("{}", LoopPhase::Done), "DONE");
    }

    #[test]
    fn test_phase_next() {
        assert_eq!(LoopPhase::Design.next(), Some(LoopPhase::Plan));
        assert_eq!(LoopPhase::Plan.next(), Some(LoopPhase::Implement));
        assert_eq!(LoopPhase::Implement.next(), Some(LoopPhase::Verify));
        assert_eq!(LoopPhase::Verify.next(), Some(LoopPhase::ReValidate));
        assert_eq!(LoopPhase::ReValidate.next(), Some(LoopPhase::Fix));
        assert_eq!(LoopPhase::Fix.next(), Some(LoopPhase::Verify));
        assert_eq!(LoopPhase::Done.next(), None);
    }

    #[test]
    fn test_phase_can_exit_on_clean() {
        assert!(LoopPhase::Verify.can_exit_on_clean());
        assert!(LoopPhase::ReValidate.can_exit_on_clean());
        assert!(!LoopPhase::Design.can_exit_on_clean());
        assert!(!LoopPhase::Fix.can_exit_on_clean());
    }

    #[test]
    fn test_phase_all() {
        let all = LoopPhase::all();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], LoopPhase::Design);
        assert_eq!(all[6], LoopPhase::Done);
    }

    // ── TaskStatus tests ───────────────────────────────────────────

    #[test]
    fn test_task_status_display() {
        assert_eq!(format!("{}", TaskStatus::Pending), "pending");
        assert_eq!(format!("{}", TaskStatus::Running), "running");
        assert_eq!(format!("{}", TaskStatus::Done), "done");
        assert_eq!(format!("{}", TaskStatus::Failed), "failed");
    }

    // ── LoopTask tests ─────────────────────────────────────────────

    #[test]
    fn test_task_builder() {
        let task = LoopTask::new("T1", "Create auth module")
            .touches("src/auth.rs")
            .touches("src/lib.rs")
            .depends_on("T0")
            .verify_with("cargo test auth");

        assert_eq!(task.id, "T1");
        assert_eq!(task.description, "Create auth module");
        assert_eq!(task.touches_files.len(), 2);
        assert_eq!(task.depends_on, vec!["T0"]);
        assert_eq!(task.verification, "cargo test auth");
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn test_task_lifecycle() {
        let mut task = LoopTask::new("T1", "Do something");
        assert_eq!(task.status, TaskStatus::Pending);

        task.start();
        assert_eq!(task.status, TaskStatus::Running);

        task.complete(Some("abc123".to_string()));
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.commit_hash, Some("abc123".to_string()));
    }

    #[test]
    fn test_task_fail() {
        let mut task = LoopTask::new("T1", "Do something");
        task.start();
        task.fail();
        assert_eq!(task.status, TaskStatus::Failed);
    }

    // ── TaskBatch tests ────────────────────────────────────────────

    #[test]
    fn test_batch_lifecycle() {
        let mut batch = TaskBatch::new(0);
        batch.add_task(LoopTask::new("T1", "Task 1"));
        batch.add_task(LoopTask::new("T2", "Task 2"));

        assert!(!batch.all_done());
        assert!(!batch.any_failed());

        batch.start();
        assert_eq!(batch.status, TaskStatus::Running);
        assert_eq!(batch.tasks[0].status, TaskStatus::Running);

        batch.complete();
        assert_eq!(batch.status, TaskStatus::Done);
    }

    #[test]
    fn test_batch_all_done() {
        let mut batch = TaskBatch::new(0);
        let mut t1 = LoopTask::new("T1", "Task 1");
        t1.complete(None);
        let mut t2 = LoopTask::new("T2", "Task 2");
        t2.complete(None);
        batch.add_task(t1);
        batch.add_task(t2);
        assert!(batch.all_done());
    }

    #[test]
    fn test_batch_any_failed() {
        let mut batch = TaskBatch::new(0);
        batch.add_task(LoopTask::new("T1", "Task 1"));
        let mut t2 = LoopTask::new("T2", "Task 2");
        t2.fail();
        batch.add_task(t2);
        assert!(batch.any_failed());
    }

    // ── Issue tests ────────────────────────────────────────────────

    #[test]
    fn test_issue_builder() {
        let issue = Issue::new(
            1,
            "Build fails on ARM64",
            IssueSeverity::Critical,
            "src/build.rs:42",
        )
        .with_evidence("error: unsupported target")
        .reproducible(true)
        .fix_approach("Add ARM64 target detection");

        assert_eq!(issue.number, 1);
        assert_eq!(issue.severity, IssueSeverity::Critical);
        assert!(issue.reproducible);
        assert!(issue.verdict.is_none());
        assert!(!issue.fixed);
    }

    #[test]
    fn test_issue_verdict() {
        let mut issue = Issue::new(1, "Test", IssueSeverity::Minor, "main.rs");
        assert!(!issue.needs_fix());

        issue.set_verdict(IssueVerdict::Confirmed, "Reproduced locally");
        assert!(issue.needs_fix());
        assert!(issue.is_actionable());

        issue.mark_fixed(Some("abc123".to_string()));
        assert!(!issue.needs_fix());
        assert!(issue.fixed);
        assert_eq!(issue.fix_commit, Some("abc123".to_string()));
    }

    #[test]
    fn test_issue_false_positive() {
        let mut issue = Issue::new(1, "Test", IssueSeverity::Nit, "main.rs");
        issue.set_verdict(
            IssueVerdict::FalsePositive,
            "Internal function, callers trusted",
        );
        assert!(!issue.needs_fix());
        assert!(!issue.is_actionable());
    }

    #[test]
    fn test_issue_deferred() {
        let mut issue = Issue::new(1, "Test", IssueSeverity::Minor, "main.rs");
        issue.set_verdict(IssueVerdict::Deferred, "Out of scope for this task");
        assert!(!issue.needs_fix());
        assert!(!issue.is_actionable());
    }

    #[test]
    fn test_severity_ordering() {
        assert!(IssueSeverity::Critical > IssueSeverity::Important);
        assert!(IssueSeverity::Important > IssueSeverity::Minor);
        assert!(IssueSeverity::Minor > IssueSeverity::Nit);
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(format!("{}", IssueSeverity::Critical), "Critical");
        assert_eq!(format!("{}", IssueSeverity::Important), "Important");
        assert_eq!(format!("{}", IssueSeverity::Minor), "Minor");
        assert_eq!(format!("{}", IssueSeverity::Nit), "Nit");
    }

    #[test]
    fn test_verdict_display() {
        assert_eq!(format!("{}", IssueVerdict::Confirmed), "CONFIRMED");
        assert_eq!(format!("{}", IssueVerdict::FalsePositive), "FALSE_POSITIVE");
        assert_eq!(format!("{}", IssueVerdict::Deferred), "DEFERRED");
        assert_eq!(format!("{}", IssueVerdict::NeedsContext), "NEEDS_CONTEXT");
    }

    // ── VerificationResult tests ───────────────────────────────────

    #[test]
    fn test_verification_result_new() {
        let result = VerificationResult::new();
        assert!(!result.build_passed);
        assert!(!result.tests_passed);
        assert!(!result.type_check_passed);
        assert!(!result.lint_passed);
        assert!(result.issues.is_empty());
        assert!(!result.is_clean());
    }

    #[test]
    fn test_verification_result_clean() {
        let result = VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        };
        assert!(result.is_clean());
        assert!(result.critical_passed());
    }

    #[test]
    fn test_verification_result_with_issues() {
        let mut result = VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        };
        result
            .issues
            .push(Issue::new(1, "Bug", IssueSeverity::Minor, "main.rs"));
        assert!(!result.is_clean());
    }

    #[test]
    fn test_verification_critical_failed() {
        let result = VerificationResult {
            build_passed: false,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        };
        assert!(!result.critical_passed());
    }

    #[test]
    fn test_verification_issue_count_by_severity() {
        let mut result = VerificationResult::new();
        result
            .issues
            .push(Issue::new(1, "A", IssueSeverity::Critical, "a.rs"));
        result
            .issues
            .push(Issue::new(2, "B", IssueSeverity::Critical, "b.rs"));
        result
            .issues
            .push(Issue::new(3, "C", IssueSeverity::Minor, "c.rs"));
        assert_eq!(result.issue_count_by_severity(IssueSeverity::Critical), 2);
        assert_eq!(result.issue_count_by_severity(IssueSeverity::Minor), 1);
        assert_eq!(result.issue_count_by_severity(IssueSeverity::Nit), 0);
    }

    // ── AutonomousLoop tests ───────────────────────────────────────

    #[test]
    fn test_loop_new() {
        let al = AutonomousLoop::new("Implement auth");
        assert_eq!(al.task, "Implement auth");
        assert_eq!(al.iteration, 0);
        assert_eq!(al.max_iterations, MAX_ITERATIONS);
        assert_eq!(al.phase, LoopPhase::Design);
        assert!(!al.started);
        assert!(al.tasks.is_empty());
        assert!(al.issues.is_empty());
    }

    #[test]
    fn test_loop_with_max_iterations() {
        let al = AutonomousLoop::new("Test").with_max_iterations(4);
        assert_eq!(al.max_iterations, 4);
    }

    #[test]
    fn test_loop_with_max_iterations_clamped() {
        let al = AutonomousLoop::new("Test").with_max_iterations(0);
        assert_eq!(al.max_iterations, 1);

        let al = AutonomousLoop::new("Test").with_max_iterations(100);
        assert_eq!(al.max_iterations, MAX_ITERATIONS);
    }

    #[test]
    fn test_loop_start() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        assert!(al.started);
        assert_eq!(al.iteration, 1);
        assert_eq!(al.phase, LoopPhase::Design);
    }

    #[test]
    fn test_loop_start_twice() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        assert!(al.start().is_err());
    }

    #[test]
    fn test_loop_emergency_stop() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.emergency_stop("Build is broken beyond repair");
        assert!(al.emergency_stopped);
        assert_eq!(
            al.blocker,
            Some("Build is broken beyond repair".to_string())
        );
    }

    #[test]
    fn test_loop_advance_not_started() {
        let mut al = AutonomousLoop::new("Test");
        assert!(al.advance().is_err());
    }

    #[test]
    fn test_loop_advance_after_emergency_stop() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.emergency_stop("Nope");
        assert!(al.advance().is_err());
    }

    #[test]
    fn test_loop_advance_design_to_plan() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        let next = al.advance().unwrap();
        assert_eq!(next, LoopPhase::Plan);
        assert_eq!(al.phase, LoopPhase::Plan);
    }

    #[test]
    fn test_loop_advance_plan_to_implement() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.advance().unwrap(); // Design → Plan
        let next = al.advance().unwrap(); // Plan → Implement
        assert_eq!(next, LoopPhase::Implement);
    }

    #[test]
    fn test_loop_advance_implement_to_verify() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.advance().unwrap(); // → Plan
        al.advance().unwrap(); // → Implement
        let next = al.advance().unwrap(); // → Verify
        assert_eq!(next, LoopPhase::Verify);
    }

    #[test]
    fn test_loop_advance_verify_clean_goes_to_done() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.advance().unwrap(); // → Plan
        al.advance().unwrap(); // → Implement
        al.advance().unwrap(); // → Verify

        // Record a clean verification
        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        });

        let next = al.advance().unwrap(); // → Done (clean)
        assert_eq!(next, LoopPhase::Done);
    }

    #[test]
    fn test_loop_advance_verify_with_issues_goes_to_revalidate() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.advance().unwrap(); // → Plan
        al.advance().unwrap(); // → Implement
        al.advance().unwrap(); // → Verify

        // Record verification with an issue
        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![Issue::new(1, "A bug", IssueSeverity::Important, "main.rs")],
            timestamp: Utc::now().to_rfc3339(),
        });

        let next = al.advance().unwrap(); // → ReValidate
        assert_eq!(next, LoopPhase::ReValidate);
    }

    #[test]
    fn test_loop_advance_revalidate_confirmed_goes_to_fix() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.advance().unwrap(); // → Plan
        al.advance().unwrap(); // → Implement
        al.advance().unwrap(); // → Verify

        // Add an issue and confirm it
        let mut issue = Issue::new(1, "A bug", IssueSeverity::Important, "main.rs");
        issue.set_verdict(IssueVerdict::Confirmed, "Reproduced");
        al.add_issue(issue);

        al.set_phase(LoopPhase::ReValidate);
        let next = al.advance().unwrap(); // → Fix
        assert_eq!(next, LoopPhase::Fix);
    }

    #[test]
    fn test_loop_advance_revalidate_false_positive_goes_to_done() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();

        // Add a false-positive issue
        let mut issue = Issue::new(1, "False alarm", IssueSeverity::Nit, "main.rs");
        issue.set_verdict(IssueVerdict::FalsePositive, "Internal function");
        al.add_issue(issue);

        al.set_phase(LoopPhase::ReValidate);
        let next = al.advance().unwrap(); // → Done
        assert_eq!(next, LoopPhase::Done);
    }

    #[test]
    fn test_loop_advance_fix_goes_to_verify_with_increment() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        assert_eq!(al.iteration, 1);

        al.set_phase(LoopPhase::Fix);
        let next = al.advance().unwrap(); // → Verify, iteration++
        assert_eq!(next, LoopPhase::Verify);
        assert_eq!(al.iteration, 2);
    }

    #[test]
    fn test_loop_advance_done_is_error() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();
        al.set_phase(LoopPhase::Done);
        assert!(al.advance().is_err());
    }

    #[test]
    fn test_loop_max_iterations_exceeded() {
        let mut al = AutonomousLoop::new("Test").with_max_iterations(2);
        al.start().unwrap();
        al.iteration = 2;

        al.set_phase(LoopPhase::Fix);
        assert!(al.advance().is_err());
        assert!(al.emergency_stopped);
    }

    #[test]
    fn test_loop_set_phase() {
        let mut al = AutonomousLoop::new("Test");
        al.set_phase(LoopPhase::Verify);
        assert_eq!(al.phase, LoopPhase::Verify);
    }

    // ── Task management tests ──────────────────────────────────────

    #[test]
    fn test_add_and_get_task() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Create module"));
        al.add_task(LoopTask::new("T2", "Add tests"));

        assert_eq!(al.tasks.len(), 2);
        assert_eq!(al.get_task("T1").unwrap().description, "Create module");
        assert_eq!(al.get_task("T2").unwrap().description, "Add tests");
        assert!(al.get_task("T3").is_none());
    }

    #[test]
    fn test_get_task_mut() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Create module"));

        al.get_task_mut("T1")
            .unwrap()
            .complete(Some("abc".to_string()));
        assert_eq!(
            al.get_task("T1").unwrap().commit_hash,
            Some("abc".to_string())
        );
    }

    // ── Batch computation tests ────────────────────────────────────

    #[test]
    fn test_compute_batches_empty() {
        let mut al = AutonomousLoop::new("Test");
        al.compute_batches().unwrap();
        assert!(al.batches.is_empty());
    }

    #[test]
    fn test_compute_batches_single_task() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Do thing"));
        al.compute_batches().unwrap();

        assert_eq!(al.batches.len(), 1);
        assert_eq!(al.batches[0].tasks.len(), 1);
        assert!(!al.batches[0].has_conflicts);
    }

    #[test]
    fn test_compute_batches_parallel() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Task 1"));
        al.add_task(LoopTask::new("T2", "Task 2"));
        al.compute_batches().unwrap();

        // Both in batch 0 (no dependencies)
        assert_eq!(al.batches.len(), 1);
        assert_eq!(al.batches[0].tasks.len(), 2);
    }

    #[test]
    fn test_compute_batches_sequential() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Foundation"));
        al.add_task(LoopTask::new("T2", "Build on foundation").depends_on("T1"));
        al.add_task(LoopTask::new("T3", "Final layer").depends_on("T2"));

        al.compute_batches().unwrap();

        assert_eq!(al.batches.len(), 3);
        assert_eq!(al.batches[0].tasks[0].id, "T1");
        assert_eq!(al.batches[1].tasks[0].id, "T2");
        assert_eq!(al.batches[2].tasks[0].id, "T3");
    }

    #[test]
    fn test_compute_batches_mixed() {
        let mut al = AutonomousLoop::new("Test");
        // T1, T2 independent → batch 0
        al.add_task(LoopTask::new("T1", "Independent 1"));
        al.add_task(LoopTask::new("T2", "Independent 2"));
        // T3 depends on T1, T4 depends on T2 → batch 1
        al.add_task(LoopTask::new("T3", "After T1").depends_on("T1"));
        al.add_task(LoopTask::new("T4", "After T2").depends_on("T2"));
        // T5 depends on T3, T4 → batch 2
        al.add_task(
            LoopTask::new("T5", "After T3 and T4")
                .depends_on("T3")
                .depends_on("T4"),
        );

        al.compute_batches().unwrap();

        assert_eq!(al.batches.len(), 3);
        assert_eq!(al.batches[0].tasks.len(), 2);
        assert_eq!(al.batches[1].tasks.len(), 2);
        assert_eq!(al.batches[2].tasks.len(), 1);
    }

    #[test]
    fn test_compute_batches_file_conflicts() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Touch lib").touches("src/lib.rs"));
        al.add_task(LoopTask::new("T2", "Also touch lib").touches("src/lib.rs"));

        al.compute_batches().unwrap();

        // Both in batch 0 but flagged as conflicting
        assert_eq!(al.batches.len(), 1);
        assert!(al.batches[0].has_conflicts);
    }

    #[test]
    fn test_compute_batches_circular_dependency() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Circular 1").depends_on("T2"));
        al.add_task(LoopTask::new("T2", "Circular 2").depends_on("T1"));

        let result = al.compute_batches();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("circular dependency"));
    }

    #[test]
    fn test_next_pending_batch() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Task 1"));
        al.add_task(LoopTask::new("T2", "Task 2").depends_on("T1"));
        al.compute_batches().unwrap();

        assert!(al.next_pending_batch().is_some());
        assert_eq!(al.next_pending_batch().unwrap().index, 0);

        al.batches[0].status = TaskStatus::Done;
        assert!(al.next_pending_batch().is_some());
        assert_eq!(al.next_pending_batch().unwrap().index, 1);

        al.batches[1].status = TaskStatus::Done;
        assert!(al.next_pending_batch().is_none());
    }

    #[test]
    fn test_completed_batch_count() {
        let mut al = AutonomousLoop::new("Test");
        al.add_task(LoopTask::new("T1", "Task 1"));
        al.add_task(LoopTask::new("T2", "Task 2").depends_on("T1"));
        al.compute_batches().unwrap();

        assert_eq!(al.completed_batch_count(), 0);
        assert_eq!(al.total_batch_count(), 2);

        al.batches[0].status = TaskStatus::Done;
        assert_eq!(al.completed_batch_count(), 1);
    }

    // ── Issue management tests ─────────────────────────────────────

    #[test]
    fn test_add_issue() {
        let mut al = AutonomousLoop::new("Test");
        al.add_issue(Issue::new(1, "Bug", IssueSeverity::Important, "main.rs"));

        assert_eq!(al.issues.len(), 1);
        assert_eq!(al.issues[0].description, "Bug");
    }

    #[test]
    fn test_confirmed_issues() {
        let mut al = AutonomousLoop::new("Test");

        // Add a confirmed issue
        let mut confirmed = Issue::new(1, "Real bug", IssueSeverity::Important, "main.rs");
        confirmed.set_verdict(IssueVerdict::Confirmed, "Reproduced");
        al.add_issue(confirmed);

        // Add a false positive
        let mut fp = Issue::new(2, "False alarm", IssueSeverity::Nit, "lib.rs");
        fp.set_verdict(IssueVerdict::FalsePositive, "Internal function");
        al.add_issue(fp);

        // Add a fixed issue
        let mut fixed = Issue::new(3, "Already fixed", IssueSeverity::Minor, "util.rs");
        fixed.set_verdict(IssueVerdict::Confirmed, "Was real");
        fixed.mark_fixed(None);
        al.add_issue(fixed);

        assert_eq!(al.confirmed_issues().len(), 1);
        assert_eq!(al.confirmed_issues()[0].description, "Real bug");
    }

    #[test]
    fn test_issues_by_verdict() {
        let mut al = AutonomousLoop::new("Test");

        let mut i1 = Issue::new(1, "A", IssueSeverity::Minor, "a");
        i1.set_verdict(IssueVerdict::Confirmed, "Real");
        al.add_issue(i1);

        let mut i2 = Issue::new(2, "B", IssueSeverity::Nit, "b");
        i2.set_verdict(IssueVerdict::FalsePositive, "Fake");
        al.add_issue(i2);

        let mut i3 = Issue::new(3, "C", IssueSeverity::Minor, "c");
        i3.set_verdict(IssueVerdict::Confirmed, "Real");
        al.add_issue(i3);

        assert_eq!(al.issues_by_verdict(IssueVerdict::Confirmed), 2);
        assert_eq!(al.issues_by_verdict(IssueVerdict::FalsePositive), 1);
        assert_eq!(al.issues_by_verdict(IssueVerdict::Deferred), 0);
    }

    #[test]
    fn test_fixed_issue_count() {
        let mut al = AutonomousLoop::new("Test");

        let mut i1 = Issue::new(1, "A", IssueSeverity::Minor, "a");
        i1.mark_fixed(Some("abc".to_string()));
        al.add_issue(i1);

        al.add_issue(Issue::new(2, "B", IssueSeverity::Minor, "b"));

        assert_eq!(al.fixed_issue_count(), 1);
    }

    // ── Verification tests ─────────────────────────────────────────

    #[test]
    fn test_record_verification() {
        let mut al = AutonomousLoop::new("Test");
        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![Issue::new(1, "Bug", IssueSeverity::Minor, "main.rs")],
            timestamp: Utc::now().to_rfc3339(),
        });

        assert!(al.last_verification.is_some());
        assert_eq!(al.issues.len(), 1);
    }

    #[test]
    fn test_is_clean() {
        let mut al = AutonomousLoop::new("Test");
        assert!(!al.is_clean()); // No verification yet

        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        });
        assert!(al.is_clean());
    }

    #[test]
    fn test_is_dirty_with_issue() {
        let mut al = AutonomousLoop::new("Test");
        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![Issue::new(1, "Bug", IssueSeverity::Minor, "main.rs")],
            timestamp: Utc::now().to_rfc3339(),
        });
        assert!(!al.is_clean());
    }

    // ── Git integration tests ──────────────────────────────────────

    #[test]
    fn test_record_commit() {
        let mut al = AutonomousLoop::new("Test");
        al.record_commit("deadbeef");
        assert_eq!(al.last_commit, Some("deadbeef".to_string()));
    }

    #[test]
    fn test_set_git_clean() {
        let mut al = AutonomousLoop::new("Test");
        al.set_git_clean(false);
        assert!(!al.git_clean);
        al.set_git_clean(true);
        assert!(al.git_clean);
    }

    // ── Status and diagnostics tests ───────────────────────────────

    #[test]
    fn test_status_snapshot() {
        let mut al = AutonomousLoop::new("Build auth system");
        al.start().unwrap();
        al.add_task(LoopTask::new("T1", "Create module"));
        al.compute_batches().unwrap();

        let status = al.status();
        assert_eq!(status.task, "Build auth system");
        assert_eq!(status.iteration, 1);
        assert_eq!(status.phase, LoopPhase::Design);
        assert_eq!(status.batches.len(), 1);
        assert!(status.git_clean);
    }

    #[test]
    fn test_status_render() {
        let mut al = AutonomousLoop::new("Test task");
        al.start().unwrap();
        al.add_task(LoopTask::new("T1", "Foundation"));
        al.add_task(LoopTask::new("T2", "Build on it").depends_on("T1"));
        al.compute_batches().unwrap();
        al.record_commit("abc1234");

        let status = al.status();
        let rendered = status.render();

        assert!(rendered.contains("AUTONOMOUS LOOP STATUS"));
        assert!(rendered.contains("Test task"));
        assert!(rendered.contains("DESIGN"));
        assert!(rendered.contains("T1"));
        assert!(rendered.contains("abc1234"));
    }

    #[test]
    fn test_diagnostic() {
        let mut al = AutonomousLoop::new("Test task");
        al.start().unwrap();
        al.emergency_stop("Hit max iterations");

        let diag = al.diagnostic();
        assert!(diag.contains("AUTONOMOUS LOOP DIAGNOSTIC"));
        assert!(diag.contains("Test task"));
        assert!(diag.contains("Hit max iterations"));
    }

    #[test]
    fn test_diagnostic_with_issues() {
        let mut al = AutonomousLoop::new("Test");
        al.start().unwrap();

        let mut issue = Issue::new(
            1,
            "Critical build failure",
            IssueSeverity::Critical,
            "build.rs:1",
        );
        issue.set_verdict(IssueVerdict::Confirmed, "Build won't compile");
        issue.set_verdict(IssueVerdict::Confirmed, "Still broken");
        al.add_issue(issue);

        let diag = al.diagnostic();
        assert!(diag.contains("Critical build failure"));
        assert!(diag.contains("build.rs:1"));
    }

    // ── Skill prompt test ──────────────────────────────────────────

    #[test]
    fn test_skill_instructions() {
        let prompt = AutonomousLoopSkill::skill_instructions();
        assert!(prompt.contains("Autonomous Development Loop"));
        assert!(prompt.contains("DESIGN"));
        assert!(prompt.contains("PLAN"));
        assert!(prompt.contains("IMPLEMENT"));
        assert!(prompt.contains("VERIFY"));
        assert!(prompt.contains("RE-VALIDATE"));
        assert!(prompt.contains("FIX"));
        assert!(prompt.contains("DONE"));
        assert!(prompt.contains("8"));
        assert!(prompt.contains("Emergency Stop"));
        assert!(prompt.contains("Anti-Rationalization"));
        assert!(prompt.contains("Red Flags"));
    }

    // ── Full loop integration test ─────────────────────────────────

    #[test]
    fn test_full_loop_happy_path() {
        let mut al = AutonomousLoop::new("Implement caching").with_max_iterations(3);
        al.start().unwrap();
        assert_eq!(al.iteration, 1);
        assert_eq!(al.phase, LoopPhase::Design);

        // Design → Plan
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Plan);

        // Add tasks
        al.add_task(LoopTask::new("T1", "Create cache module").touches("src/cache.rs"));
        al.add_task(
            LoopTask::new("T2", "Add tests")
                .depends_on("T1")
                .touches("tests/cache_test.rs"),
        );
        al.compute_batches().unwrap();
        assert_eq!(al.total_batch_count(), 2);

        // Plan → Implement
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Implement);

        // Implement → Verify
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Verify);

        // Record clean verification
        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        });
        assert!(al.is_clean());

        // Verify → Done
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Done);
    }

    #[test]
    fn test_full_loop_with_fix_cycle() {
        let mut al = AutonomousLoop::new("Fix bugs").with_max_iterations(4);
        al.start().unwrap();

        // Fast forward to Verify
        al.set_phase(LoopPhase::Verify);

        // Record verification with issue
        al.record_verification(VerificationResult {
            build_passed: false,
            tests_passed: false,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![
                Issue::new(1, "Build fails", IssueSeverity::Critical, "main.rs:10")
                    .with_evidence("undefined variable"),
            ],
            timestamp: Utc::now().to_rfc3339(),
        });
        assert!(!al.is_clean());

        // Verify → ReValidate
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::ReValidate);

        // Confirm the issue
        al.issues[0].set_verdict(IssueVerdict::Confirmed, "Build output reproduced");

        // ReValidate → Fix
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Fix);

        // Fix the issue
        al.issues[0].mark_fixed(Some("fix123".to_string()));
        al.record_commit("fix123");

        // Fix → Verify (iteration 2)
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Verify);
        assert_eq!(al.iteration, 2);

        // Now record clean verification
        al.record_verification(VerificationResult {
            build_passed: true,
            tests_passed: true,
            type_check_passed: true,
            lint_passed: true,
            issues: vec![],
            timestamp: Utc::now().to_rfc3339(),
        });

        // Verify → Done
        al.advance().unwrap();
        assert_eq!(al.phase, LoopPhase::Done);
    }

    // ── Serialization tests ────────────────────────────────────────

    #[test]
    fn test_loop_serde_roundtrip() {
        let mut al = AutonomousLoop::new("Serialize test");
        al.start().unwrap();
        al.add_task(LoopTask::new("T1", "Do work").touches("src/main.rs"));
        al.compute_batches().unwrap();
        al.add_issue(Issue::new(1, "Bug", IssueSeverity::Important, "main.rs"));
        al.record_commit("abc123");

        let json = serde_json::to_string_pretty(&al).unwrap();
        let parsed: AutonomousLoop = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.task, al.task);
        assert_eq!(parsed.iteration, al.iteration);
        assert_eq!(parsed.phase, al.phase);
        assert_eq!(parsed.tasks.len(), 1);
        assert_eq!(parsed.batches.len(), 1);
        assert_eq!(parsed.issues.len(), 1);
        assert_eq!(parsed.last_commit, Some("abc123".to_string()));
    }

    #[test]
    fn test_status_serde_roundtrip() {
        let al = AutonomousLoop::new("Status test");
        let status = al.status();

        let json = serde_json::to_string(&status).unwrap();
        let parsed: LoopStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.task, status.task);
        assert_eq!(parsed.iteration, status.iteration);
        assert_eq!(parsed.phase, status.phase);
    }

    #[test]
    fn test_verification_result_serde_roundtrip() {
        let result = VerificationResult {
            build_passed: true,
            tests_passed: false,
            type_check_passed: true,
            lint_passed: false,
            issues: vec![Issue::new(
                1,
                "Test fail",
                IssueSeverity::Important,
                "test.rs",
            )],
            timestamp: Utc::now().to_rfc3339(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: VerificationResult = serde_json::from_str(&json).unwrap();

        assert!(parsed.build_passed);
        assert!(!parsed.tests_passed);
        assert_eq!(parsed.issues.len(), 1);
    }

    #[test]
    fn test_issue_serde_roundtrip() {
        let mut issue = Issue::new(1, "Bug", IssueSeverity::Critical, "main.rs:10")
            .with_evidence("error: undefined")
            .reproducible(true)
            .fix_approach("Add variable declaration");
        issue.set_verdict(IssueVerdict::Confirmed, "Reproduced on main");
        issue.mark_fixed(Some("fix456".to_string()));

        let json = serde_json::to_string(&issue).unwrap();
        let parsed: Issue = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.number, 1);
        assert_eq!(parsed.severity, IssueSeverity::Critical);
        assert!(parsed.reproducible);
        assert_eq!(parsed.verdict, Some(IssueVerdict::Confirmed));
        assert!(parsed.fixed);
        assert_eq!(parsed.fix_commit, Some("fix456".to_string()));
    }

    #[test]
    fn test_loop_task_serde_roundtrip() {
        let task = LoopTask::new("T1", "Create module")
            .touches("src/mod.rs")
            .depends_on("T0")
            .verify_with("cargo test");

        let json = serde_json::to_string(&task).unwrap();
        let parsed: LoopTask = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "T1");
        assert_eq!(parsed.touches_files.len(), 1);
        assert_eq!(parsed.depends_on, vec!["T0"]);
        assert_eq!(parsed.verification, "cargo test");
    }
}
