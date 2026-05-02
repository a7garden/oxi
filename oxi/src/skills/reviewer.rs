//! Reviewer skill for oxi
//!
//! Code review specialist that performs structured, thorough reviews of
//! code changes. The reviewer:
//!
//! 1. **Contextualizes** — reads the change scope, affected files, project conventions
//! 2. **Reviews per-axis** — correctness, readability, performance, security, testing
//! 3. **Classifies findings** — critical / important / minor / nit with evidence
//! 4. **Filters false positives** — cross-examines every finding before reporting
//! 5. **Outputs** a structured review document with actionable recommendations
//!
//! The module provides:
//! - [`ReviewSession`] — state machine tracking the review lifecycle
//! - [`ReviewFinding`] / [`FindingSeverity`] — structured findings with evidence
//! - [`ReviewAxis`] — per-axis review categories
//! - [`ReviewReport`] — complete review document rendered to Markdown
//! - [`ReviewerSkill`] — skill prompt generator for LLM-driven reviews

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Phase ──────────────────────────────────────────────────────────────

/// The phase a review session is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPhase {
    /// Reading context: scope, affected files, conventions.
    Context,
    /// Per-axis deep review of each changed area.
    DeepReview,
    /// Cross-examining findings to filter false positives.
    CrossExamine,
    /// Producing the final review document.
    Produce,
    /// Review complete.
    Done,
}

impl Default for ReviewPhase {
    fn default() -> Self {
        ReviewPhase::Context
    }
}

impl fmt::Display for ReviewPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReviewPhase::Context => write!(f, "Context"),
            ReviewPhase::DeepReview => write!(f, "Deep Review"),
            ReviewPhase::CrossExamine => write!(f, "Cross-Examine"),
            ReviewPhase::Produce => write!(f, "Produce"),
            ReviewPhase::Done => write!(f, "Done"),
        }
    }
}

// ── Review axes ────────────────────────────────────────────────────────

/// A review axis — a category of concern to review against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAxis {
    /// Does the code do what it's supposed to do?
    Correctness,
    /// Is the code readable and maintainable?
    Readability,
    /// Are there performance issues?
    Performance,
    /// Are there security vulnerabilities?
    Security,
    /// Are edge cases and error paths handled?
    ErrorHandling,
    /// Is the code adequately tested?
    Testing,
    /// Does the code follow project conventions?
    Conventions,
    /// Are there concerns at the architecture level?
    Architecture,
}

impl fmt::Display for ReviewAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReviewAxis::Correctness => write!(f, "Correctness"),
            ReviewAxis::Readability => write!(f, "Readability"),
            ReviewAxis::Performance => write!(f, "Performance"),
            ReviewAxis::Security => write!(f, "Security"),
            ReviewAxis::ErrorHandling => write!(f, "Error Handling"),
            ReviewAxis::Testing => write!(f, "Testing"),
            ReviewAxis::Conventions => write!(f, "Conventions"),
            ReviewAxis::Architecture => write!(f, "Architecture"),
        }
    }
}

/// All review axes in a standard order.
impl ReviewAxis {
    /// Return all review axes in priority order.
    pub fn all() -> &'static [ReviewAxis] {
        &[
            ReviewAxis::Correctness,
            ReviewAxis::ErrorHandling,
            ReviewAxis::Security,
            ReviewAxis::Testing,
            ReviewAxis::Performance,
            ReviewAxis::Readability,
            ReviewAxis::Conventions,
            ReviewAxis::Architecture,
        ]
    }
}

// ── Findings ───────────────────────────────────────────────────────────

/// Severity of a review finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// Formatting, naming preference. Optional.
    Nit,
    /// Style inconsistency, minor polish. Should fix if trivial.
    Minor,
    /// Will cause problems under real conditions. Must fix.
    Important,
    /// Incorrect behavior, data loss risk, or security vulnerability. Must fix.
    Critical,
}

impl fmt::Display for FindingSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FindingSeverity::Nit => write!(f, "nit"),
            FindingSeverity::Minor => write!(f, "minor"),
            FindingSeverity::Important => write!(f, "important"),
            FindingSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// Verdict after cross-examining a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingVerdict {
    /// Genuine issue — must be fixed.
    Confirmed,
    /// Not actually a problem.
    FalsePositive,
    /// Real issue but out of scope for this review.
    Deferred,
}

impl fmt::Display for FindingVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FindingVerdict::Confirmed => write!(f, "confirmed"),
            FindingVerdict::FalsePositive => write!(f, "false positive"),
            FindingVerdict::Deferred => write!(f, "deferred"),
        }
    }
}

/// Evidence pointing to specific code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeEvidence {
    /// File path (relative to project root).
    pub file: String,
    /// Optional line number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// Code snippet or excerpt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// What this evidence demonstrates.
    pub observation: String,
}

impl fmt::Display for CodeEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "`{}:{}` — {}", self.file, line, self.observation),
            None => write!(f, "`{}` — {}", self.file, self.observation),
        }
    }
}

/// A single review finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// Short description of the finding.
    pub description: String,
    /// Which review axis this belongs to.
    pub axis: ReviewAxis,
    /// Severity of the finding.
    pub severity: FindingSeverity,
    /// Supporting evidence from the code.
    pub evidence: Vec<CodeEvidence>,
    /// Suggested fix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Cross-examination verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<FindingVerdict>,
    /// Reason for the verdict (especially for false positives).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict_reason: Option<String>,
}

impl ReviewFinding {
    /// Create a new finding.
    pub fn new(
        description: impl Into<String>,
        axis: ReviewAxis,
        severity: FindingSeverity,
    ) -> Self {
        Self {
            description: description.into(),
            axis,
            severity,
            evidence: Vec::new(),
            suggestion: None,
            verdict: None,
            verdict_reason: None,
        }
    }

    /// Add evidence to this finding.
    pub fn with_evidence(mut self, evidence: CodeEvidence) -> Self {
        self.evidence.push(evidence);
        self
    }

    /// Set the suggested fix.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set the cross-examination verdict.
    pub fn set_verdict(&mut self, verdict: FindingVerdict, reason: impl Into<String>) {
        self.verdict = Some(verdict);
        self.verdict_reason = Some(reason.into());
    }

    /// Whether this finding is confirmed (not a false positive or deferred).
    pub fn is_confirmed(&self) -> bool {
        matches!(self.verdict, Some(FindingVerdict::Confirmed))
    }

    /// Whether this finding has been cross-examined.
    pub fn is_examined(&self) -> bool {
        self.verdict.is_some()
    }
}

// ── Review scope ───────────────────────────────────────────────────────

/// The scope of a code review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewScope {
    /// Description of what's being reviewed.
    pub description: String,
    /// Files included in the review.
    pub files: Vec<String>,
    /// Optional commit range (e.g., "abc123..def456").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_range: Option<String>,
    /// Optional branch name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

// ── Review report ──────────────────────────────────────────────────────

/// The complete review report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    /// Report metadata.
    pub meta: ReviewMeta,
    /// What was reviewed.
    pub scope: ReviewScope,
    /// Per-axis summaries.
    pub axis_summaries: Vec<AxisSummary>,
    /// All findings (both confirmed and false positives).
    pub findings: Vec<ReviewFinding>,
    /// Overall assessment.
    pub overall: OverallAssessment,
    /// Actionable recommendations (confirmed findings to fix).
    pub recommendations: Vec<Recommendation>,
    /// False positives discarded during cross-examination.
    pub discarded: Vec<DiscardedFinding>,
}

/// Metadata for the review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewMeta {
    /// ISO date string.
    pub date: String,
    /// Reviewer identifier.
    pub reviewer: String,
}

/// Summary of findings per review axis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisSummary {
    /// Which axis.
    pub axis: ReviewAxis,
    /// Overall impression for this axis.
    pub impression: AxisImpression,
    /// Number of findings (confirmed only).
    pub finding_count: usize,
    /// Brief summary text.
    pub summary: String,
}

/// Overall impression for an axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisImpression {
    /// No issues found.
    Clean,
    /// Minor issues only.
    Acceptable,
    /// Has issues that should be addressed.
    NeedsWork,
    /// Has critical issues that must be fixed.
    Concerning,
}

impl fmt::Display for AxisImpression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AxisImpression::Clean => write!(f, "✅ Clean"),
            AxisImpression::Acceptable => write!(f, "🟢 Acceptable"),
            AxisImpression::NeedsWork => write!(f, "🟡 Needs Work"),
            AxisImpression::Concerning => write!(f, "🔴 Concerning"),
        }
    }
}

/// Overall assessment of the code under review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallAssessment {
    /// One-line summary.
    pub summary: String,
    /// Whether the code is ready to merge/ship.
    pub ready_to_ship: bool,
    /// Total confirmed findings by severity.
    pub critical_count: usize,
    pub important_count: usize,
    pub minor_count: usize,
    pub nit_count: usize,
}

/// An actionable recommendation from the review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    /// What to do.
    pub action: String,
    /// Why it matters.
    pub rationale: String,
    /// Severity of the underlying finding.
    pub severity: FindingSeverity,
    /// Which files are affected.
    pub files: Vec<String>,
    /// Priority (lower = higher priority).
    pub priority: u32,
}

/// A discarded (false positive or deferred) finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscardedFinding {
    /// Original description.
    pub description: String,
    /// Why it was discarded.
    pub reason: String,
    /// Whether it was a false positive or deferred.
    pub discard_type: FindingVerdict,
}

impl ReviewReport {
    /// Render the review as Markdown.
    pub fn render_markdown(&self) -> String {
        let mut md = String::with_capacity(4096);

        // Title
        md.push_str(&format!(
            "# Code Review: {}\n\n",
            self.scope.description
        ));
        md.push_str(&format!("> Date: {} | Reviewer: {}\n", self.meta.date, self.meta.reviewer));

        if let Some(ref range) = self.scope.commit_range {
            md.push_str(&format!("> Commits: {}\n", range));
        }
        if let Some(ref branch) = self.scope.branch {
            md.push_str(&format!("> Branch: {}\n", branch));
        }
        md.push('\n');

        // Overall
        md.push_str("## Overall Assessment\n\n");
        let ship_status = if self.overall.ready_to_ship {
            "✅ Ready to ship"
        } else {
            "❌ Not ready to ship"
        };
        md.push_str(&format!("**{}** — {}\n\n", ship_status, self.overall.summary));
        md.push_str(&format!(
            "- Critical: {} | Important: {} | Minor: {} | Nit: {}\n\n",
            self.overall.critical_count,
            self.overall.important_count,
            self.overall.minor_count,
            self.overall.nit_count,
        ));

        // Axis summaries
        if !self.axis_summaries.is_empty() {
            md.push_str("## Axis Summary\n\n");
            md.push_str("| Axis | Impression | Findings | Summary |\n");
            md.push_str("|------|-----------|----------|----------|\n");
            for summary in &self.axis_summaries {
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    summary.axis, summary.impression, summary.finding_count, summary.summary
                ));
            }
            md.push('\n');
        }

        // Files reviewed
        if !self.scope.files.is_empty() {
            md.push_str("## Files Reviewed\n\n");
            for file in &self.scope.files {
                md.push_str(&format!("- `{}`\n", file));
            }
            md.push('\n');
        }

        // Confirmed findings
        let confirmed: Vec<&ReviewFinding> = self
            .findings
            .iter()
            .filter(|f| f.is_confirmed())
            .collect();

        if !confirmed.is_empty() {
            md.push_str("## Findings\n\n");
            for (i, finding) in confirmed.iter().enumerate() {
                md.push_str(&format!(
                    "### {}. [{}] {} ({})\n\n",
                    i + 1,
                    finding.severity,
                    finding.description,
                    finding.axis,
                ));

                if !finding.evidence.is_empty() {
                    md.push_str("**Evidence:**\n");
                    for ev in &finding.evidence {
                        md.push_str(&format!("- {}\n", ev));
                    }
                    md.push('\n');
                }

                if let Some(ref suggestion) = finding.suggestion {
                    md.push_str(&format!("**Suggestion:** {}\n\n", suggestion));
                }
            }
        }

        // Recommendations
        if !self.recommendations.is_empty() {
            md.push_str("## Recommendations\n\n");
            for (i, rec) in self.recommendations.iter().enumerate() {
                md.push_str(&format!(
                    "{}. **[{}] {}**\n   {}\n",
                    i + 1,
                    rec.severity,
                    rec.action,
                    rec.rationale,
                ));
                if !rec.files.is_empty() {
                    md.push_str(&format!("   Files: {}\n", rec.files.join(", ")));
                }
            }
            md.push('\n');
        }

        // Discarded findings
        if !self.discarded.is_empty() {
            md.push_str("## Discarded Findings\n\n");
            for discarded in &self.discarded {
                let icon = match discarded.discard_type {
                    FindingVerdict::FalsePositive => "❌",
                    FindingVerdict::Deferred => "⏸️",
                    FindingVerdict::Confirmed => "✅",
                };
                md.push_str(&format!(
                    "- {} {}: {}\n",
                    icon, discarded.description, discarded.reason
                ));
            }
            md.push('\n');
        }

        md
    }

    /// Write the review to a file.
    pub fn write_to_file(&self, dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;

        let date = &self.meta.date;
        let slug = slugify(&self.scope.description);
        let filename = format!("{}-{}.md", date, slug);
        let path = dir.join(&filename);

        let content = self.render_markdown();
        fs::write(&path, &content)
            .with_context(|| format!("Failed to write review to {}", path.display()))?;

        Ok(path)
    }
}

// ── Review session ─────────────────────────────────────────────────────

/// A review session that tracks the review lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSession {
    /// Current phase.
    pub phase: ReviewPhase,
    /// What's being reviewed.
    pub scope: Option<ReviewScope>,
    /// Optional project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// Findings accumulated during review.
    pub findings: Vec<ReviewFinding>,
    /// Axis summaries (set during produce phase).
    pub axis_summaries: Vec<AxisSummary>,
    /// The finalized report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ReviewReport>,
}

impl ReviewSession {
    /// Create a new review session.
    pub fn new() -> Self {
        Self {
            phase: ReviewPhase::Context,
            scope: None,
            project_root: None,
            findings: Vec::new(),
            axis_summaries: Vec::new(),
            report: None,
        }
    }

    /// Set the review scope.
    pub fn with_scope(mut self, scope: ReviewScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Set the project root.
    pub fn with_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    /// Advance to the next phase.
    pub fn advance(&mut self) -> Result<()> {
        let next = match self.phase {
            ReviewPhase::Context => ReviewPhase::DeepReview,
            ReviewPhase::DeepReview => ReviewPhase::CrossExamine,
            ReviewPhase::CrossExamine => ReviewPhase::Produce,
            ReviewPhase::Produce => ReviewPhase::Done,
            ReviewPhase::Done => bail!("Cannot advance past Done"),
        };
        self.phase = next;
        Ok(())
    }

    /// Set phase directly.
    pub fn set_phase(&mut self, phase: ReviewPhase) {
        self.phase = phase;
    }

    /// Add a finding.
    pub fn add_finding(&mut self, finding: ReviewFinding) {
        self.findings.push(finding);
    }

    /// Number of findings.
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }

    /// Get confirmed findings only.
    pub fn confirmed_findings(&self) -> Vec<&ReviewFinding> {
        self.findings.iter().filter(|f| f.is_confirmed()).collect()
    }

    /// Get findings by axis.
    pub fn findings_by_axis(&self, axis: ReviewAxis) -> Vec<&ReviewFinding> {
        self.findings.iter().filter(|f| f.axis == axis).collect()
    }

    /// Get findings by severity.
    pub fn findings_by_severity(&self, severity: FindingSeverity) -> Vec<&ReviewFinding> {
        self.findings.iter().filter(|f| f.severity == severity).collect()
    }

    /// Cross-examine a specific finding by index.
    ///
    /// Sets the verdict based on the analysis.
    pub fn cross_examine(&mut self, index: usize, verdict: FindingVerdict, reason: impl Into<String>) -> Result<()> {
        let finding = self.findings.get_mut(index)
            .with_context(|| format!("No finding at index {}", index))?;
        finding.set_verdict(verdict, reason);
        Ok(())
    }

    /// Cross-examine all unexamined findings.
    ///
    /// Returns the number of findings that were already examined.
    pub fn cross_examine_all(&mut self, verdict: FindingVerdict, reason: impl Into<String>) -> usize {
        let reason_str = reason.into();
        let mut already_examined = 0;
        for finding in &mut self.findings {
            if finding.is_examined() {
                already_examined += 1;
            } else {
                finding.set_verdict(verdict, reason_str.clone());
            }
        }
        already_examined
    }

    /// Produce the final report from accumulated findings.
    pub fn produce_report(&mut self) -> Result<()> {
        let scope = self.scope.clone()
            .context("Review scope not set — call with_scope() first")?;

        // Count confirmed findings by severity
        let confirmed: Vec<&ReviewFinding> = self.confirmed_findings();
        let critical_count = confirmed.iter().filter(|f| f.severity == FindingSeverity::Critical).count();
        let important_count = confirmed.iter().filter(|f| f.severity == FindingSeverity::Important).count();
        let minor_count = confirmed.iter().filter(|f| f.severity == FindingSeverity::Minor).count();
        let nit_count = confirmed.iter().filter(|f| f.severity == FindingSeverity::Nit).count();

        let ready_to_ship = critical_count == 0 && important_count == 0;

        // Build axis summaries
        let mut axis_summaries = Vec::new();
        for axis in ReviewAxis::all() {
            let axis_findings: Vec<&&ReviewFinding> = confirmed.iter().filter(|f| f.axis == *axis).collect();
            let count = axis_findings.len();

            let has_critical = axis_findings.iter().any(|f| f.severity == FindingSeverity::Critical);
            let has_important = axis_findings.iter().any(|f| f.severity == FindingSeverity::Important);

            let impression = if count == 0 {
                AxisImpression::Clean
            } else if has_critical {
                AxisImpression::Concerning
            } else if has_important {
                AxisImpression::NeedsWork
            } else {
                AxisImpression::Acceptable
            };

            let summary = if count == 0 {
                "No issues found.".to_string()
            } else {
                format!("{} finding(s).", count)
            };

            axis_summaries.push(AxisSummary {
                axis: *axis,
                impression,
                finding_count: count,
                summary,
            });
        }
        self.axis_summaries = axis_summaries.clone();

        // Build recommendations from confirmed findings
        let mut recommendations: Vec<Recommendation> = confirmed
            .iter()
            .enumerate()
            .map(|(i, f)| Recommendation {
                action: f.suggestion.clone().unwrap_or_else(|| f.description.clone()),
                rationale: f.description.clone(),
                severity: f.severity,
                files: f.evidence.iter().map(|e| e.file.clone()).collect(),
                priority: i as u32,
            })
            .collect();

        // Sort by severity (critical first)
        recommendations.sort_by(|a, b| b.severity.cmp(&a.severity));
        for (i, rec) in recommendations.iter_mut().enumerate() {
            rec.priority = i as u32;
        }

        // Build discarded list
        let discarded: Vec<DiscardedFinding> = self.findings.iter()
            .filter(|f| matches!(f.verdict, Some(FindingVerdict::FalsePositive) | Some(FindingVerdict::Deferred)))
            .map(|f| DiscardedFinding {
                description: f.description.clone(),
                reason: f.verdict_reason.clone().unwrap_or_default(),
                discard_type: f.verdict.unwrap_or(FindingVerdict::FalsePositive),
            })
            .collect();

        let summary = if ready_to_ship {
            format!("Code is ready to ship with {} minor finding(s).", minor_count + nit_count)
        } else {
            format!("{} critical and {} important finding(s) must be addressed before shipping.", critical_count, important_count)
        };

        let report = ReviewReport {
            meta: ReviewMeta {
                date: Utc::now().format("%Y-%m-%d").to_string(),
                reviewer: "oxi reviewer".to_string(),
            },
            scope,
            axis_summaries,
            findings: self.findings.clone(),
            overall: OverallAssessment {
                summary,
                ready_to_ship,
                critical_count,
                important_count,
                minor_count,
                nit_count,
            },
            recommendations,
            discarded,
        };

        self.report = Some(report);
        Ok(())
    }

    /// Write the report to disk.
    pub fn write_report(&self, explicit_path: Option<&Path>) -> Result<PathBuf> {
        let report = self.report.as_ref()
            .context("Report not produced — call produce_report() first")?;

        if let Some(path) = explicit_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            let content = report.render_markdown();
            fs::write(path, &content)
                .with_context(|| format!("Failed to write review to {}", path.display()))?;
            Ok(path.to_path_buf())
        } else {
            let root = self.project_root.as_deref()
                .context("No project root and no explicit path")?;
            let review_dir = root.join("docs").join("review");
            report.write_to_file(&review_dir)
        }
    }
}

impl Default for ReviewSession {
    fn default() -> Self {
        Self::new()
    }
}

// ── Skill prompt ───────────────────────────────────────────────────────

/// The reviewer skill struct.
pub struct ReviewerSkill;

impl ReviewerSkill {
    /// Create a new reviewer skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Generate the system-prompt fragment for the reviewer skill.
    pub fn skill_prompt() -> String {
        r#"# Reviewer Skill

You are running the **reviewer** skill. Your job is to perform a thorough,
structured code review that catches real problems without generating false
positives.

## Workflow

### Phase 1: Context

1. Identify the scope of changes (files changed, diff summary).
2. Read project conventions (AGENTS.md, CONTRIBUTING.md, linting config).
3. Understand what the changes are trying to accomplish.
4. Note the project's actual conventions — don't impose external standards.

### Phase 2: Deep Review

Review each changed file against these axes, in priority order:

1. **Correctness** — Does the code do what it claims? Logic errors?
2. **Error Handling** — Are edge cases covered? Errors propagated?
3. **Security** — Input validation, injection risks, secrets?
4. **Testing** — Are new behaviors tested? Edge cases covered?
5. **Performance** — N+1 queries? Unnecessary allocations? Hot paths?
6. **Readability** — Clear naming? Obvious control flow?
7. **Conventions** — Follows project style? Consistent patterns?
8. **Architecture** — Clean boundaries? Good abstractions?

For each finding, record:
- **Description** — What's wrong
- **Severity** — critical / important / minor / nit
- **Evidence** — Exact file, line, and code
- **Suggestion** — How to fix it

### Phase 3: Cross-Examine

For EVERY finding, ask:
- Does this violate a project convention, or just a generic best practice?
- Is this actually in scope for this change?
- Would a staff engineer on this project flag this?
- Is the "correct" version actually better in this context?
- Does this affect actual behavior, or is it theoretical?

Discard false positives. Defer out-of-scope items. Only keep genuine findings.

### Phase 4: Produce

1. Generate the review report with confirmed findings.
2. Determine if code is ready to ship.
3. Write to `docs/review/YYYY-MM-DD-<slug>.md`.

## Rules

- Every finding MUST cite specific code (file + line + observation).
- Never flag something without explaining WHY it's a problem.
- Severity must match reality — don't inflate nits to critical.
- If the project intentionally uses a pattern, don't flag it as wrong.
- It's better to miss a minor issue than to waste time on false positives.
- The goal is to help the author, not to prove you found the most issues.
"#
        .to_string()
    }
}

impl Default for ReviewerSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ReviewerSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReviewerSkill").finish()
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

    fn sample_finding(axis: ReviewAxis, severity: FindingSeverity) -> ReviewFinding {
        ReviewFinding::new(
            format!("{:?} issue", axis),
            axis,
            severity,
        )
        .with_evidence(CodeEvidence {
            file: "src/main.rs".to_string(),
            line: Some(42),
            snippet: Some("unwrap()".to_string()),
            observation: "Direct unwrap on Option".to_string(),
        })
        .with_suggestion("Use ok_or_else()? instead")
    }

    #[test]
    fn test_session_new() {
        let session = ReviewSession::new();
        assert_eq!(session.phase, ReviewPhase::Context);
        assert!(session.scope.is_none());
        assert!(session.findings.is_empty());
    }

    #[test]
    fn test_phase_advance() {
        let mut session = ReviewSession::new();
        assert_eq!(session.phase, ReviewPhase::Context);

        session.advance().unwrap();
        assert_eq!(session.phase, ReviewPhase::DeepReview);

        session.advance().unwrap();
        assert_eq!(session.phase, ReviewPhase::CrossExamine);

        session.advance().unwrap();
        assert_eq!(session.phase, ReviewPhase::Produce);

        session.advance().unwrap();
        assert_eq!(session.phase, ReviewPhase::Done);

        assert!(session.advance().is_err());
    }

    #[test]
    fn test_set_phase() {
        let mut session = ReviewSession::new();
        session.set_phase(ReviewPhase::CrossExamine);
        assert_eq!(session.phase, ReviewPhase::CrossExamine);
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", ReviewPhase::Context), "Context");
        assert_eq!(format!("{}", ReviewPhase::DeepReview), "Deep Review");
        assert_eq!(format!("{}", ReviewPhase::CrossExamine), "Cross-Examine");
        assert_eq!(format!("{}", ReviewPhase::Produce), "Produce");
        assert_eq!(format!("{}", ReviewPhase::Done), "Done");
    }

    #[test]
    fn test_review_axis_display() {
        assert_eq!(format!("{}", ReviewAxis::Correctness), "Correctness");
        assert_eq!(format!("{}", ReviewAxis::Security), "Security");
        assert_eq!(format!("{}", ReviewAxis::Testing), "Testing");
    }

    #[test]
    fn test_review_axis_all() {
        let all = ReviewAxis::all();
        assert_eq!(all.len(), 8);
        assert_eq!(all[0], ReviewAxis::Correctness);
    }

    #[test]
    fn test_severity_display_and_ordering() {
        assert_eq!(format!("{}", FindingSeverity::Critical), "critical");
        assert_eq!(format!("{}", FindingSeverity::Important), "important");
        assert_eq!(format!("{}", FindingSeverity::Minor), "minor");
        assert_eq!(format!("{}", FindingSeverity::Nit), "nit");

        assert!(FindingSeverity::Critical > FindingSeverity::Important);
        assert!(FindingSeverity::Important > FindingSeverity::Minor);
        assert!(FindingSeverity::Minor > FindingSeverity::Nit);
    }

    #[test]
    fn test_verdict_display() {
        assert_eq!(format!("{}", FindingVerdict::Confirmed), "confirmed");
        assert_eq!(format!("{}", FindingVerdict::FalsePositive), "false positive");
        assert_eq!(format!("{}", FindingVerdict::Deferred), "deferred");
    }

    #[test]
    fn test_axis_impression_display() {
        assert!(format!("{}", AxisImpression::Clean).contains("Clean"));
        assert!(format!("{}", AxisImpression::Acceptable).contains("Acceptable"));
        assert!(format!("{}", AxisImpression::NeedsWork).contains("Needs Work"));
        assert!(format!("{}", AxisImpression::Concerning).contains("Concerning"));
    }

    #[test]
    fn test_code_evidence_display() {
        let ev = CodeEvidence {
            file: "src/main.rs".to_string(),
            line: Some(42),
            snippet: None,
            observation: "Direct unwrap".to_string(),
        };
        assert_eq!(format!("{}", ev), "`src/main.rs:42` — Direct unwrap");

        let ev_no_line = CodeEvidence {
            file: "src/lib.rs".to_string(),
            line: None,
            snippet: None,
            observation: "Module issue".to_string(),
        };
        assert_eq!(format!("{}", ev_no_line), "`src/lib.rs` — Module issue");
    }

    #[test]
    fn test_finding_creation() {
        let f = ReviewFinding::new("Missing error check", ReviewAxis::ErrorHandling, FindingSeverity::Important);
        assert_eq!(f.description, "Missing error check");
        assert_eq!(f.axis, ReviewAxis::ErrorHandling);
        assert_eq!(f.severity, FindingSeverity::Important);
        assert!(f.evidence.is_empty());
        assert!(f.suggestion.is_none());
        assert!(f.verdict.is_none());
        assert!(!f.is_examined());
        assert!(!f.is_confirmed());
    }

    #[test]
    fn test_finding_with_evidence_and_suggestion() {
        let f = sample_finding(ReviewAxis::Security, FindingSeverity::Critical);
        assert_eq!(f.evidence.len(), 1);
        assert_eq!(f.evidence[0].file, "src/main.rs");
        assert!(f.suggestion.is_some());
    }

    #[test]
    fn test_finding_set_verdict() {
        let mut f = sample_finding(ReviewAxis::Correctness, FindingSeverity::Important);
        f.set_verdict(FindingVerdict::Confirmed, "Reproduces consistently");
        assert!(f.is_examined());
        assert!(f.is_confirmed());
        assert_eq!(f.verdict_reason, Some("Reproduces consistently".to_string()));
    }

    #[test]
    fn test_finding_false_positive() {
        let mut f = sample_finding(ReviewAxis::Conventions, FindingSeverity::Nit);
        f.set_verdict(FindingVerdict::FalsePositive, "Project convention differs");
        assert!(f.is_examined());
        assert!(!f.is_confirmed());
    }

    #[test]
    fn test_add_findings() {
        let mut session = ReviewSession::new();
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Critical));
        session.add_finding(sample_finding(ReviewAxis::Security, FindingSeverity::Important));
        session.add_finding(sample_finding(ReviewAxis::Readability, FindingSeverity::Nit));

        assert_eq!(session.finding_count(), 3);
    }

    #[test]
    fn test_findings_by_axis() {
        let mut session = ReviewSession::new();
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Important));
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Minor));
        session.add_finding(sample_finding(ReviewAxis::Security, FindingSeverity::Critical));

        assert_eq!(session.findings_by_axis(ReviewAxis::Correctness).len(), 2);
        assert_eq!(session.findings_by_axis(ReviewAxis::Security).len(), 1);
        assert_eq!(session.findings_by_axis(ReviewAxis::Testing).len(), 0);
    }

    #[test]
    fn test_findings_by_severity() {
        let mut session = ReviewSession::new();
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Critical));
        session.add_finding(sample_finding(ReviewAxis::Security, FindingSeverity::Critical));
        session.add_finding(sample_finding(ReviewAxis::Readability, FindingSeverity::Minor));

        assert_eq!(session.findings_by_severity(FindingSeverity::Critical).len(), 2);
        assert_eq!(session.findings_by_severity(FindingSeverity::Minor).len(), 1);
    }

    #[test]
    fn test_cross_examine_single() {
        let mut session = ReviewSession::new();
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Important));

        session.cross_examine(0, FindingVerdict::Confirmed, "Real bug").unwrap();
        assert!(session.findings[0].is_confirmed());

        // Invalid index
        assert!(session.cross_examine(5, FindingVerdict::FalsePositive, "test").is_err());
    }

    #[test]
    fn test_cross_examine_all() {
        let mut session = ReviewSession::new();
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Important));
        session.add_finding(sample_finding(ReviewAxis::Security, FindingSeverity::Critical));

        // Mark first as already examined
        session.findings[0].set_verdict(FindingVerdict::Confirmed, "Already done");

        let already = session.cross_examine_all(FindingVerdict::FalsePositive, "Generic reason");
        assert_eq!(already, 1);
        assert!(session.findings[0].is_confirmed());
        assert!(!session.findings[1].is_confirmed()); // false positive
    }

    #[test]
    fn test_produce_report_no_scope() {
        let mut session = ReviewSession::new();
        assert!(session.produce_report().is_err());
    }

    #[test]
    fn test_produce_report_ready_to_ship() {
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test review".to_string(),
                files: vec!["src/main.rs".to_string()],
                commit_range: None,
                branch: None,
            });

        // No findings → ready to ship
        session.produce_report().unwrap();

        let report = session.report.as_ref().unwrap();
        assert!(report.overall.ready_to_ship);
        assert_eq!(report.overall.critical_count, 0);
        assert_eq!(report.overall.important_count, 0);
    }

    #[test]
    fn test_produce_report_not_ready() {
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test".to_string(),
                files: vec!["src/main.rs".to_string()],
                commit_range: None,
                branch: None,
            });

        let mut f = sample_finding(ReviewAxis::Correctness, FindingSeverity::Critical);
        f.set_verdict(FindingVerdict::Confirmed, "Real bug");
        session.add_finding(f);

        let mut f2 = sample_finding(ReviewAxis::Security, FindingSeverity::Important);
        f2.set_verdict(FindingVerdict::Confirmed, "SQL injection");
        session.add_finding(f2);

        // Add a false positive
        let mut f3 = sample_finding(ReviewAxis::Conventions, FindingSeverity::Nit);
        f3.set_verdict(FindingVerdict::FalsePositive, "Project convention");
        session.add_finding(f3);

        session.produce_report().unwrap();

        let report = session.report.as_ref().unwrap();
        assert!(!report.overall.ready_to_ship);
        assert_eq!(report.overall.critical_count, 1);
        assert_eq!(report.overall.important_count, 1);
        assert_eq!(report.recommendations.len(), 2);
        assert_eq!(report.discarded.len(), 1);
        assert_eq!(report.discarded[0].discard_type, FindingVerdict::FalsePositive);
    }

    #[test]
    fn test_render_markdown() {
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Auth feature".to_string(),
                files: vec!["src/auth.rs".to_string(), "src/middleware.rs".to_string()],
                commit_range: Some("abc..def".to_string()),
                branch: Some("feat/auth".to_string()),
            });

        let mut f = ReviewFinding::new(
            "SQL injection in login query",
            ReviewAxis::Security,
            FindingSeverity::Critical,
        )
        .with_evidence(CodeEvidence {
            file: "src/auth.rs".to_string(),
            line: Some(45),
            snippet: Some("format!(\"SELECT * FROM users WHERE name = '{}'\", name)".to_string()),
            observation: "String interpolation in SQL query".to_string(),
        })
        .with_suggestion("Use parameterized query");
        f.set_verdict(FindingVerdict::Confirmed, "Direct injection risk");

        session.add_finding(f);
        session.produce_report().unwrap();

        let md = session.report.as_ref().unwrap().render_markdown();
        assert!(md.contains("# Code Review: Auth feature"));
        assert!(md.contains("Not ready to ship"));
        assert!(md.contains("## Axis Summary"));
        assert!(md.contains("## Files Reviewed"));
        assert!(md.contains("`src/auth.rs`"));
        assert!(md.contains("## Findings"));
        assert!(md.contains("SQL injection"));
        assert!(md.contains("parameterized query"));
        assert!(md.contains("Commits: abc..def"));
        assert!(md.contains("Branch: feat/auth"));
    }

    #[test]
    fn test_write_report() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test Review".to_string(),
                files: vec![],
                commit_range: None,
                branch: None,
            })
            .with_project_root(tmp.path());

        session.produce_report().unwrap();
        let path = session.write_report(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("docs/review"));
    }

    #[test]
    fn test_write_report_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test".to_string(),
                files: vec![],
                commit_range: None,
                branch: None,
            });

        session.produce_report().unwrap();
        let explicit = tmp.path().join("review.md");
        let path = session.write_report(Some(&explicit)).unwrap();
        assert_eq!(path, explicit);
        assert!(path.exists());
    }

    #[test]
    fn test_write_report_not_produced() {
        let session = ReviewSession::new();
        assert!(session.write_report(None).is_err());
    }

    #[test]
    fn test_session_serialization_roundtrip() {
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test".to_string(),
                files: vec!["src/main.rs".to_string()],
                commit_range: None,
                branch: None,
            });
        session.add_finding(sample_finding(ReviewAxis::Correctness, FindingSeverity::Important));
        session.set_phase(ReviewPhase::DeepReview);

        let json = serde_json::to_string(&session).unwrap();
        let parsed: ReviewSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.finding_count(), 1);
        assert_eq!(parsed.phase, ReviewPhase::DeepReview);
    }

    #[test]
    fn test_axis_summaries_populated() {
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test".to_string(),
                files: vec![],
                commit_range: None,
                branch: None,
            });

        let mut f = sample_finding(ReviewAxis::Security, FindingSeverity::Critical);
        f.set_verdict(FindingVerdict::Confirmed, "Real");
        session.add_finding(f);

        session.produce_report().unwrap();

        let security_summary = session.axis_summaries.iter()
            .find(|s| s.axis == ReviewAxis::Security)
            .unwrap();
        assert_eq!(security_summary.finding_count, 1);
        assert_eq!(security_summary.impression, AxisImpression::Concerning);

        let correctness_summary = session.axis_summaries.iter()
            .find(|s| s.axis == ReviewAxis::Correctness)
            .unwrap();
        assert_eq!(correctness_summary.finding_count, 0);
        assert_eq!(correctness_summary.impression, AxisImpression::Clean);
    }

    #[test]
    fn test_recommendations_sorted_by_severity() {
        let mut session = ReviewSession::new()
            .with_scope(ReviewScope {
                description: "Test".to_string(),
                files: vec![],
                commit_range: None,
                branch: None,
            });

        // Add in reverse severity order
        let mut f1 = sample_finding(ReviewAxis::Readability, FindingSeverity::Nit);
        f1.set_verdict(FindingVerdict::Confirmed, "Real");
        session.add_finding(f1);

        let mut f2 = sample_finding(ReviewAxis::Correctness, FindingSeverity::Critical);
        f2.set_verdict(FindingVerdict::Confirmed, "Real");
        session.add_finding(f2);

        let mut f3 = sample_finding(ReviewAxis::Performance, FindingSeverity::Important);
        f3.set_verdict(FindingVerdict::Confirmed, "Real");
        session.add_finding(f3);

        session.produce_report().unwrap();
        let report = session.report.as_ref().unwrap();

        // Critical should be first
        assert_eq!(report.recommendations[0].severity, FindingSeverity::Critical);
        assert_eq!(report.recommendations[1].severity, FindingSeverity::Important);
        assert_eq!(report.recommendations[2].severity, FindingSeverity::Nit);
    }

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = ReviewerSkill::skill_prompt();
        assert!(prompt.contains("Reviewer Skill"));
        assert!(prompt.contains("Phase 1: Context"));
        assert!(prompt.contains("Phase 3: Cross-Examine"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Auth Feature Review"), "auth-feature-review");
        assert_eq!(slugify("hello_world"), "hello-world");
    }
}
