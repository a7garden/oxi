//! Super-review skill for oxi
//!
//! Performs deep, honest system analysis that goes beyond superficial
//! pass/fail checklists. The skill:
//!
//! 1. **Understands the project first** — reads manifests, docs, structure
//!    to grasp the system's purpose before judging anything
//! 2. **Evaluates each domain against the system's purpose** — architecture
//!    isn't judged in isolation; it's judged by how well it serves the system
//! 3. **Produces honest assessments with evidence** — every claim cites
//!    specific code, every severity rating matches reality
//! 4. **Finds cross-cutting patterns** — connects dots across domains to
//!    surface issues invisible to per-domain analysis
//!
//! The module provides:
//! - A [`SuperReviewSession`] that tracks the review lifecycle
//! - Domain-specific analysis types ([`DomainAssessment`], [`Evidence`], etc.)
//! - A markdown renderer that produces the structured review document
//! - Project context gathering (manifest reading, directory walking)
//! - A skill prompt generator for the LLM-driven review workflow

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Types ──────────────────────────────────────────────────────────────

/// The phase a super-review session is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Reading project context to understand the system.
    Understand,
    /// Identifying relevant review domains.
    MapDomains,
    /// Deep analysis of each domain.
    Analyze,
    /// Looking for patterns that span domains.
    CrossCutting,
    /// Writing the review document.
    Produce,
    /// Review complete.
    Done,
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Understand
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Understand => write!(f, "Understand"),
            Phase::MapDomains => write!(f, "Map Domains"),
            Phase::Analyze => write!(f, "Analyze"),
            Phase::CrossCutting => write!(f, "Cross-Cutting"),
            Phase::Produce => write!(f, "Produce"),
            Phase::Done => write!(f, "Done"),
        }
    }
}

/// Assessment rating for a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    /// Purposeful, well-executed, couldn't reasonably be better.
    Excellent,
    /// Solid work, minor improvements possible.
    Good,
    /// Works, but has clear room for improvement.
    Adequate,
    /// Problems that will cause issues under real conditions.
    Concerning,
    /// Fundamental problems that undermine the system's purpose.
    Poor,
    /// Broken, dangerous, or actively harmful.
    Critical,
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rating::Excellent => write!(f, "Excellent"),
            Rating::Good => write!(f, "Good"),
            Rating::Adequate => write!(f, "Adequate"),
            Rating::Concerning => write!(f, "Concerning"),
            Rating::Poor => write!(f, "Poor"),
            Rating::Critical => write!(f, "Critical"),
        }
    }
}

/// Severity of a finding (concern or issue).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Style, naming, minor polish.
    Nit,
    /// Clear improvement opportunity but not causing problems today.
    Minor,
    /// Will cause problems under real conditions.
    Important,
    /// Causes incorrect behavior, data loss risk, or security vulnerability.
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Nit => write!(f, "nit"),
            Severity::Minor => write!(f, "minor"),
            Severity::Important => write!(f, "important"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}

/// A piece of evidence pointing to specific code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// File path (relative to project root).
    pub file: String,
    /// Optional line range (start, end).
    pub lines: Option<(usize, usize)>,
    /// What this evidence demonstrates.
    pub observation: String,
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.lines {
            Some((start, end)) => write!(f, "`{}:{}-{}` — {}", self.file, start, end, self.observation),
            None => write!(f, "`{}` — {}", self.file, self.observation),
        }
    }
}

/// A single finding within a domain assessment (strength or concern).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// What was found.
    pub description: String,
    /// Severity (for concerns) or relevance (for strengths).
    pub severity: Severity,
    /// Supporting evidence.
    pub evidence: Vec<Evidence>,
}

/// Assessment of a single review domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainAssessment {
    /// Domain name (e.g., "Error Handling", "Architecture").
    pub domain: String,
    /// Why this domain matters for THIS system.
    pub relevance: String,
    /// Overall rating.
    pub rating: Rating,
    /// Detailed analysis text.
    pub analysis: String,
    /// Things the system does well in this domain.
    pub strengths: Vec<Finding>,
    /// Problems or concerns in this domain.
    pub concerns: Vec<Finding>,
    /// Direct evidence citations.
    pub evidence: Vec<Evidence>,
}

/// A cross-cutting finding that spans multiple domains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossCuttingFinding {
    /// Title of the finding.
    pub title: String,
    /// Description of the pattern or issue.
    pub description: String,
    /// Which domains this connects.
    pub connected_domains: Vec<String>,
    /// Evidence from multiple domains.
    pub evidence: Vec<Evidence>,
    /// How important is this?
    pub severity: Severity,
}

/// A ranked recommendation from the review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    /// What to do.
    pub action: String,
    /// Why it matters.
    pub rationale: String,
    /// Estimated effort (e.g., "2 hours", "1 sprint").
    pub effort: String,
    /// Which domains or findings this addresses.
    pub addresses: Vec<String>,
    /// Priority ordering (lower = higher priority).
    pub priority: u32,
}

/// The system-level understanding captured during Phase 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemUnderstanding {
    /// What problem does this system solve?
    pub purpose: String,
    /// Who are the primary users?
    pub users: String,
    /// What promises does the system make?
    pub core_contract: String,
    /// What workflows MUST work?
    pub critical_paths: Vec<String>,
    /// Hard constraints (performance, security, compatibility).
    pub constraints: Vec<String>,
    /// Free-form context notes.
    pub notes: String,
}

/// Project context gathered from the filesystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    /// Project name from manifest.
    pub name: String,
    /// Version from manifest.
    pub version: String,
    /// Language / ecosystem.
    pub language: String,
    /// Top-level directory structure.
    pub directory_structure: String,
    /// Key files read and their summaries.
    pub key_files: Vec<(String, String)>,
    /// Dependencies found.
    pub dependencies: Vec<String>,
    /// Has tests directory.
    pub has_tests: bool,
    /// Has CI configuration.
    pub has_ci: bool,
    /// Has documentation directory.
    pub has_docs: bool,
}

/// The complete super-review report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperReviewReport {
    /// Report metadata.
    pub meta: ReportMeta,
    /// System understanding from Phase 1.
    pub system_understanding: SystemUnderstanding,
    /// Project context from Phase 1.
    pub project_context: ProjectContext,
    /// Domains selected and their analyses.
    pub domains: Vec<DomainAssessment>,
    /// Cross-cutting findings.
    pub cross_cutting: Vec<CrossCuttingFinding>,
    /// What's genuinely working well.
    pub working_well: Vec<String>,
    /// What needs attention, ranked.
    pub needs_attention: Vec<String>,
    /// What's at risk.
    pub at_risk: Vec<String>,
    /// The bottom-line verdict.
    pub verdict: String,
    /// Ordered recommendations.
    pub recommendations: Vec<Recommendation>,
}

/// Metadata for a review report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportMeta {
    /// ISO date string (YYYY-MM-DD).
    pub date: String,
    /// URL-friendly slug.
    pub slug: String,
    /// What was reviewed.
    pub scope: String,
    /// Reviewer identifier.
    pub reviewer: String,
}

// ── Session ──────────────────────────────────────────────────────────

/// A super-review session that tracks the full review lifecycle.
///
/// The session progresses through phases: Understand → MapDomains →
/// Analyze → CrossCutting → Produce → Done. Each phase builds on the
/// previous one, and the final report is rendered from the accumulated
/// data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperReviewSession {
    /// Current phase.
    pub phase: Phase,
    /// Optional project root directory.
    pub project_root: Option<PathBuf>,
    /// Optional scope description (what's being reviewed).
    pub scope: Option<String>,
    /// System understanding (populated in Phase 1).
    pub system_understanding: Option<SystemUnderstanding>,
    /// Project context (populated in Phase 1).
    pub project_context: Option<ProjectContext>,
    /// Domain assessments (populated in Phase 3).
    pub domains: Vec<DomainAssessment>,
    /// Cross-cutting findings (populated in Phase 4).
    pub cross_cutting: Vec<CrossCuttingFinding>,
    /// Working well (populated in Phase 5).
    pub working_well: Vec<String>,
    /// Needs attention (populated in Phase 5).
    pub needs_attention: Vec<String>,
    /// At risk (populated in Phase 5).
    pub at_risk: Vec<String>,
    /// Verdict (populated in Phase 5).
    pub verdict: Option<String>,
    /// Recommendations (populated in Phase 5).
    pub recommendations: Vec<Recommendation>,
}

impl SuperReviewSession {
    /// Create a new super-review session.
    pub fn new() -> Self {
        Self {
            phase: Phase::Understand,
            project_root: None,
            scope: None,
            system_understanding: None,
            project_context: None,
            domains: Vec::new(),
            cross_cutting: Vec::new(),
            working_well: Vec::new(),
            needs_attention: Vec::new(),
            at_risk: Vec::new(),
            verdict: None,
            recommendations: Vec::new(),
        }
    }

    /// Set the project root directory.
    pub fn with_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    /// Set the review scope.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Advance to the next phase.
    ///
    /// Returns an error if trying to advance past `Done`.
    pub fn advance(&mut self) -> Result<()> {
        let next = match self.phase {
            Phase::Understand => Phase::MapDomains,
            Phase::MapDomains => Phase::Analyze,
            Phase::Analyze => Phase::CrossCutting,
            Phase::CrossCutting => Phase::Produce,
            Phase::Produce => Phase::Done,
            Phase::Done => bail!("Cannot advance past Done"),
        };
        self.phase = next;
        Ok(())
    }

    /// Set the phase directly.
    pub fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    /// Set the system understanding (Phase 1).
    pub fn set_system_understanding(&mut self, understanding: SystemUnderstanding) {
        self.system_understanding = Some(understanding);
    }

    /// Set the project context (Phase 1).
    pub fn set_project_context(&mut self, context: ProjectContext) {
        self.project_context = Some(context);
    }

    /// Add a domain assessment (Phase 3).
    pub fn add_domain(&mut self, assessment: DomainAssessment) {
        self.domains.push(assessment);
    }

    /// Add a cross-cutting finding (Phase 4).
    pub fn add_cross_cutting(&mut self, finding: CrossCuttingFinding) {
        self.cross_cutting.push(finding);
    }

    /// Set the working-well items (Phase 5).
    pub fn set_working_well(&mut self, items: Vec<String>) {
        self.working_well = items;
    }

    /// Set the needs-attention items (Phase 5).
    pub fn set_needs_attention(&mut self, items: Vec<String>) {
        self.needs_attention = items;
    }

    /// Set the at-risk items (Phase 5).
    pub fn set_at_risk(&mut self, items: Vec<String>) {
        self.at_risk = items;
    }

    /// Set the verdict (Phase 5).
    pub fn set_verdict(&mut self, verdict: impl Into<String>) {
        self.verdict = Some(verdict.into());
    }

    /// Add a recommendation (Phase 5).
    pub fn add_recommendation(&mut self, rec: Recommendation) {
        self.recommendations.push(rec);
    }

    /// Sort recommendations by priority.
    pub fn sort_recommendations(&mut self) {
        self.recommendations.sort_by_key(|r| r.priority);
    }

    /// Number of domain assessments.
    pub fn domain_count(&self) -> usize {
        self.domains.len()
    }

    /// Number of cross-cutting findings.
    pub fn cross_cutting_count(&self) -> usize {
        self.cross_cutting.len()
    }

    /// Gather project context from the filesystem.
    ///
    /// Reads the project manifest, directory structure, and key files.
    /// Requires `project_root` to be set.
    pub fn gather_project_context(&self, max_bytes: usize) -> Result<ProjectContext> {
        let root = self
            .project_root
            .as_ref()
            .context("Project root not set — call with_project_root() first")?;

        let mut name = String::from("unknown");
        let mut version = String::from("0.0.0");
        let mut language = String::from("unknown");
        let mut key_files = Vec::new();
        let mut dependencies = Vec::new();
        let has_tests;
        let has_ci;
        let mut has_docs = false;

        // Detect Rust project
        let cargo_toml = root.join("Cargo.toml");
        if cargo_toml.exists() {
            language = "Rust".to_string();
            if let Ok(content) = fs::read_to_string(&cargo_toml) {
                name = extract_toml_value(&content, "name").unwrap_or_else(|| "unknown".to_string());
                version = extract_toml_value(&content, "version").unwrap_or_else(|| "0.0.0".to_string());
                key_files.push(("Cargo.toml".to_string(), summarize_cargo_toml(&content)));
                Self::extract_deps_from_cargo(&content, &mut dependencies);
            }
        }

        // Detect Node.js project
        let package_json = root.join("package.json");
        if package_json.exists() && language == "unknown" {
            language = "JavaScript/TypeScript".to_string();
            if let Ok(content) = fs::read_to_string(&package_json) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    name = json
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    version = json
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0.0.0")
                        .to_string();
                    key_files.push(("package.json".to_string(), summarize_package_json(&content)));
                }
            }
        }

        // Read README if present
        for readme_name in &["README.md", "README.txt", "README"] {
            let readme_path = root.join(readme_name);
            if readme_path.exists() {
                if let Ok(content) = fs::read_to_string(&readme_path) {
                    let summary = truncate_str(&content, 500);
                    key_files.push((readme_name.to_string(), summary));
                }
                break;
            }
        }

        // Read design docs
        let docs_dir = root.join("docs");
        if docs_dir.is_dir() {
            has_docs = true;
            if let Ok(entries) = fs::read_dir(&docs_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "md") {
                        let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        if let Ok(content) = fs::read_to_string(&path) {
                            key_files.push((
                                format!("docs/{}", file_name),
                                truncate_str(&content, 300),
                            ));
                        }
                    }
                }
            }
        }

        // Read AGENTS.md if present
        let agents_md = root.join("AGENTS.md");
        if agents_md.exists() {
            if let Ok(content) = fs::read_to_string(&agents_md) {
                key_files.push(("AGENTS.md".to_string(), truncate_str(&content, 500)));
            }
        }

        // Check for tests and CI
        has_tests = root.join("tests").is_dir()
            || root.join("src").join("test").is_dir()
            || root.join("test").is_dir();
        has_ci = root.join(".github").join("workflows").is_dir()
            || root.join(".gitlab-ci.yml").exists()
            || root.join("Jenkinsfile").exists();

        // Build directory structure
        let directory_structure = Self::build_directory_tree(root, 0, 3, max_bytes / 4);

        // Truncate key_files to fit budget
        let mut total_bytes = directory_structure.len();
        key_files.retain(|(_, content)| {
            total_bytes += content.len();
            total_bytes < max_bytes
        });

        Ok(ProjectContext {
            name,
            version,
            language,
            directory_structure,
            key_files,
            dependencies,
            has_tests,
            has_ci,
            has_docs,
        })
    }

    /// Build a text representation of the directory tree.
    fn build_directory_tree(dir: &Path, depth: usize, max_depth: usize, max_bytes: usize) -> String {
        if depth > max_depth {
            return "  ".repeat(depth) + "...\n";
        }

        let mut tree = String::new();
        let indent = "  ".repeat(depth);

        let Ok(entries) = fs::read_dir(dir) else {
            return tree;
        };

        let mut entries: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Skip hidden dirs and noise
                !name.starts_with('.')
                    && name != "target"
                    && name != "node_modules"
                    && name != "__pycache__"
                    && name != "dist"
                    && name != "build"
                    && name != "vendor"
                    && name != "coverage"
                    && name != ".git"
            })
            .collect();

        entries.sort_by_key(|e| {
            let is_dir = e.path().is_dir();
            // Directories first, then files
            (!is_dir, e.file_name().to_string_lossy().to_string())
        });

        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();

            if path.is_dir() {
                tree.push_str(&format!("{}{}/\n", indent, name));
                let subtree = Self::build_directory_tree(&path, depth + 1, max_depth, max_bytes);
                if tree.len() + subtree.len() > max_bytes {
                    tree.push_str(&format!("{}  ...\n", indent));
                    break;
                }
                tree.push_str(&subtree);
            } else {
                tree.push_str(&format!("{}{}\n", indent, name));
            }

            if tree.len() > max_bytes {
                tree.push_str(&format!("{}...\n", indent));
                break;
            }
        }

        tree
    }

    /// Extract dependency names from a Cargo.toml.
    fn extract_deps_from_cargo(content: &str, deps: &mut Vec<String>) {
        let mut in_deps = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[dependencies]" || trimmed == "[dev-dependencies]" {
                in_deps = true;
                continue;
            }
            if trimmed.starts_with('[') {
                in_deps = false;
                continue;
            }
            if in_deps {
                if let Some((name, _)) = trimmed.split_once('=') {
                    deps.push(name.trim().to_string());
                } else if trimmed.ends_with(",\"workspace\"") || trimmed.contains("workspace = true") {
                    // Workspace dependency — extract name from key
                    if let Some((name, _)) = trimmed.split_once('=') {
                        deps.push(format!("{} (workspace)", name.trim()));
                    }
                }
            }
        }
    }

    /// Render the review as a markdown string.
    pub fn render_markdown(&self) -> Result<String> {
        let system = self.system_understanding.as_ref()
            .context("System understanding not set — complete Phase 1 first")?;
        let context = self.project_context.as_ref()
            .context("Project context not set — complete Phase 1 first")?;

        if self.domains.is_empty() {
            bail!("No domain assessments — complete Phase 3 first");
        }

        let mut md = String::with_capacity(8192);

        // Title and metadata
        let scope = self.scope.as_deref().unwrap_or(&context.name);
        let date = Utc::now().format("%Y-%m-%d").to_string();
        md.push_str(&format!("# Super-Review: {}\n\n", context.name));
        md.push_str(&format!("> Date: {}\n", date));
        md.push_str(&format!("> Scope: {}\n", scope));
        md.push_str("> Reviewer: oxi super-review\n\n");

        // System Understanding
        md.push_str("## System Understanding\n\n");
        md.push_str(&system.purpose);
        md.push_str("\n\n");
        if !system.users.is_empty() {
            md.push_str(&format!("**Users:** {}\n\n", system.users));
        }
        if !system.core_contract.is_empty() {
            md.push_str(&format!("**Core Contract:** {}\n\n", system.core_contract));
        }
        if !system.critical_paths.is_empty() {
            md.push_str("**Critical Paths:**\n");
            for path in &system.critical_paths {
                md.push_str(&format!("- {}\n", path));
            }
            md.push('\n');
        }
        if !system.constraints.is_empty() {
            md.push_str("**Constraints:**\n");
            for constraint in &system.constraints {
                md.push_str(&format!("- {}\n", constraint));
            }
            md.push('\n');
        }
        if !system.notes.is_empty() {
            md.push_str(&system.notes);
            md.push_str("\n\n");
        }

        // Domain Assessments
        md.push_str("## Domain Assessments\n\n");
        for domain in &self.domains {
            md.push_str(&format!("### {} — {}\n\n", domain.domain, domain.rating));
            md.push_str(&domain.analysis);
            md.push_str("\n\n");

            if !domain.strengths.is_empty() {
                md.push_str("**Strengths:**\n");
                for strength in &domain.strengths {
                    md.push_str(&format!("- {}\n", strength.description));
                    for ev in &strength.evidence {
                        md.push_str(&format!("  - {}\n", ev));
                    }
                }
                md.push('\n');
            }

            if !domain.concerns.is_empty() {
                md.push_str("**Concerns:**\n");
                for concern in &domain.concerns {
                    md.push_str(&format!("- [{}] {}\n", concern.severity, concern.description));
                    for ev in &concern.evidence {
                        md.push_str(&format!("  - {}\n", ev));
                    }
                }
                md.push('\n');
            }

            if !domain.evidence.is_empty() {
                md.push_str("**Evidence:**\n");
                for ev in &domain.evidence {
                    md.push_str(&format!("- {}\n", ev));
                }
                md.push_str("\n---\n\n");
            }
        }

        // Cross-Cutting Findings
        if !self.cross_cutting.is_empty() {
            md.push_str("## Cross-Cutting Findings\n\n");
            for finding in &self.cross_cutting {
                md.push_str(&format!("### {}\n\n", finding.title));
                md.push_str(&finding.description);
                md.push_str("\n\n");
                md.push_str(&format!(
                    "**Connected domains:** {}\n",
                    finding.connected_domains.join(", ")
                ));
                md.push_str(&format!("**Severity:** {}\n\n", finding.severity));
                if !finding.evidence.is_empty() {
                    md.push_str("**Evidence:**\n");
                    for ev in &finding.evidence {
                        md.push_str(&format!("- {}\n", ev));
                    }
                    md.push('\n');
                }
            }
        }

        // System-Level Assessment
        md.push_str("## System-Level Assessment\n\n");

        if !self.working_well.is_empty() {
            md.push_str("### What's Working Well\n\n");
            for item in &self.working_well {
                md.push_str(&format!("- {}\n", item));
            }
            md.push('\n');
        }

        if !self.needs_attention.is_empty() {
            md.push_str("### What Needs Attention\n\n");
            for (i, item) in self.needs_attention.iter().enumerate() {
                md.push_str(&format!("{}. {}\n", i + 1, item));
            }
            md.push('\n');
        }

        if !self.at_risk.is_empty() {
            md.push_str("### What's At Risk\n\n");
            for item in &self.at_risk {
                md.push_str(&format!("- {}\n", item));
            }
            md.push('\n');
        }

        // Verdict
        if let Some(ref verdict) = self.verdict {
            md.push_str("## Verdict\n\n");
            md.push_str(verdict);
            md.push_str("\n\n");
        }

        // Recommendations
        if !self.recommendations.is_empty() {
            md.push_str("## Recommendations\n\n");
            for (i, rec) in self.recommendations.iter().enumerate() {
                md.push_str(&format!(
                    "{}. **{}** (effort: {})\n   {}\n",
                    i + 1,
                    rec.action,
                    rec.effort,
                    rec.rationale,
                ));
                if !rec.addresses.is_empty() {
                    md.push_str(&format!(
                        "   Addresses: {}\n",
                        rec.addresses.join(", ")
                    ));
                }
            }
            md.push('\n');
        }

        Ok(md)
    }

    /// Write the review document to disk.
    ///
    /// Creates the output directory if needed. Returns the path to the written file.
    pub fn write_document(&self, explicit_path: Option<&Path>) -> Result<PathBuf> {
        if self.system_understanding.is_none() {
            bail!("System understanding not set — complete Phase 1 first");
        }
        if self.domains.is_empty() {
            bail!("No domain assessments — complete Phase 3 first");
        }

        let md = self.render_markdown()?;

        let path = if let Some(explicit) = explicit_path {
            explicit.to_path_buf()
        } else {
            let root = self.project_root.as_ref()
                .context("Project root not set and no explicit path given")?;
            let date = Utc::now().format("%Y-%m-%d").to_string();
            let scope_slug = self.scope.as_deref().unwrap_or("review");
            let slug = slugify(scope_slug);
            let dir = root.join("docs").join("review");
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
            dir.join(format!("{}-{}.md", date, slug))
        };

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        fs::write(&path, &md)
            .with_context(|| format!("Failed to write review to {}", path.display()))?;

        tracing::info!("Super-review written to {}", path.display());
        Ok(path)
    }
}

impl Default for SuperReviewSession {
    fn default() -> Self {
        Self::new()
    }
}

// ── Skill Prompt ─────────────────────────────────────────────────────

/// Generate the system-prompt fragment for the super-review skill.
///
/// This is appended to the base system prompt when the super-review
/// skill is active, telling the LLM how to conduct the deep analysis.
pub fn super_review_skill_prompt() -> String {
    include_str!("super_review_prompt.md").to_string()
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert a string into a filesystem-safe slug.
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

/// Extract a value from a simple TOML `key = "value"` line.
fn extract_toml_value(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{} = \"", key);
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
        // Also handle `key = { ... name = "..." ... }` patterns (for workspace inheritance)
        if trimmed.starts_with(&format!("{} = {{", key)) {
            // Look for name within the braces
            let name_prefix = "name = \"";
            if let Some(idx) = trimmed.find(name_prefix) {
                let after = &trimmed[idx + name_prefix.len()..];
                if let Some(end) = after.find('"') {
                    return Some(after[..end].to_string());
                }
            }
        }
    }
    None
}

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        // Try to break at a word boundary
        if let Some(pos) = s[..max_len].rfind(' ') {
            if pos > max_len / 2 {
                end = pos;
            }
        }
        format!("{}...", &s[..end])
    }
}

/// Summarize a Cargo.toml for the project context.
fn summarize_cargo_toml(content: &str) -> String {
    let mut summary = String::new();
    if let Some(name) = extract_toml_value(content, "name") {
        summary.push_str(&format!("Rust crate: {}", name));
    }
    if let Some(version) = extract_toml_value(content, "version") {
        summary.push_str(&format!(" v{}", version));
    }
    if let Some(edition) = extract_toml_value(content, "edition") {
        summary.push_str(&format!(" (edition {})", edition));
    }
    if summary.is_empty() {
        summary = "Rust project".to_string();
    }
    summary
}

/// Summarize a package.json for the project context.
fn summarize_package_json(content: &str) -> String {
    let mut summary = String::from("Node.js project");
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(desc) = json.get("description").and_then(|v| v.as_str()) {
            summary.push_str(&format!(": {}", desc));
        }
    }
    summary
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── Phase & Rating display ──────────────────────────────────

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", Phase::Understand), "Understand");
        assert_eq!(format!("{}", Phase::MapDomains), "Map Domains");
        assert_eq!(format!("{}", Phase::Analyze), "Analyze");
        assert_eq!(format!("{}", Phase::CrossCutting), "Cross-Cutting");
        assert_eq!(format!("{}", Phase::Produce), "Produce");
        assert_eq!(format!("{}", Phase::Done), "Done");
    }

    #[test]
    fn test_rating_display() {
        assert_eq!(format!("{}", Rating::Excellent), "Excellent");
        assert_eq!(format!("{}", Rating::Good), "Good");
        assert_eq!(format!("{}", Rating::Adequate), "Adequate");
        assert_eq!(format!("{}", Rating::Concerning), "Concerning");
        assert_eq!(format!("{}", Rating::Poor), "Poor");
        assert_eq!(format!("{}", Rating::Critical), "Critical");
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(format!("{}", Severity::Nit), "nit");
        assert_eq!(format!("{}", Severity::Minor), "minor");
        assert_eq!(format!("{}", Severity::Important), "important");
        assert_eq!(format!("{}", Severity::Critical), "critical");
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::Important);
        assert!(Severity::Important > Severity::Minor);
        assert!(Severity::Minor > Severity::Nit);
    }

    // ── Session lifecycle ──────────────────────────────────────

    #[test]
    fn test_session_new() {
        let session = SuperReviewSession::new();
        assert_eq!(session.phase, Phase::Understand);
        assert!(session.project_root.is_none());
        assert!(session.domains.is_empty());
        assert!(session.cross_cutting.is_empty());
        assert!(session.recommendations.is_empty());
    }

    #[test]
    fn test_session_with_project_root() {
        let session = SuperReviewSession::new().with_project_root("/tmp/project");
        assert_eq!(session.project_root, Some(PathBuf::from("/tmp/project")));
    }

    #[test]
    fn test_session_with_scope() {
        let session = SuperReviewSession::new().with_scope("API layer");
        assert_eq!(session.scope, Some("API layer".to_string()));
    }

    #[test]
    fn test_phase_advance() {
        let mut session = SuperReviewSession::new();
        assert_eq!(session.phase, Phase::Understand);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::MapDomains);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Analyze);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::CrossCutting);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Produce);

        session.advance().unwrap();
        assert_eq!(session.phase, Phase::Done);

        // Cannot advance past Done
        assert!(session.advance().is_err());
    }

    #[test]
    fn test_set_phase() {
        let mut session = SuperReviewSession::new();
        session.set_phase(Phase::Analyze);
        assert_eq!(session.phase, Phase::Analyze);
    }

    // ── Domain assessment ──────────────────────────────────────

    #[test]
    fn test_add_domain() {
        let mut session = SuperReviewSession::new();
        session.add_domain(DomainAssessment {
            domain: "Error Handling".to_string(),
            relevance: "Critical for CLI tool reliability".to_string(),
            rating: Rating::Good,
            analysis: "Errors are propagated with context".to_string(),
            strengths: vec![Finding {
                description: "Uses anyhow::Context consistently".to_string(),
                severity: Severity::Nit,
                evidence: vec![Evidence {
                    file: "src/main.rs".to_string(),
                    lines: Some((42, 50)),
                    observation: "All file reads use .context()".to_string(),
                }],
            }],
            concerns: vec![Finding {
                description: "Some unwrap() calls in non-test code".to_string(),
                severity: Severity::Minor,
                evidence: vec![Evidence {
                    file: "src/session.rs".to_string(),
                    lines: Some((100, 100)),
                    observation: "Direct unwrap() on Option".to_string(),
                }],
            }],
            evidence: vec![],
        });

        assert_eq!(session.domain_count(), 1);
        assert_eq!(session.domains[0].domain, "Error Handling");
        assert_eq!(session.domains[0].rating, Rating::Good);
    }

    #[test]
    fn test_add_cross_cutting() {
        let mut session = SuperReviewSession::new();
        session.add_cross_cutting(CrossCuttingFinding {
            title: "Error handling ignores module boundaries".to_string(),
            description: "Architecture defines clean boundaries but error handling leaks internals".to_string(),
            connected_domains: vec!["Architecture".to_string(), "Error Handling".to_string()],
            evidence: vec![],
            severity: Severity::Important,
        });
        assert_eq!(session.cross_cutting_count(), 1);
    }

    #[test]
    fn test_recommendations() {
        let mut session = SuperReviewSession::new();
        session.add_recommendation(Recommendation {
            action: "Add input validation layer".to_string(),
            rationale: "User inputs reach business logic without validation".to_string(),
            effort: "4 hours".to_string(),
            addresses: vec!["Security".to_string(), "Correctness".to_string()],
            priority: 1,
        });
        session.add_recommendation(Recommendation {
            action: "Document public API".to_string(),
            rationale: "Public functions lack doc comments".to_string(),
            effort: "1 sprint".to_string(),
            addresses: vec!["Documentation".to_string()],
            priority: 3,
        });
        session.add_recommendation(Recommendation {
            action: "Add integration tests".to_string(),
            rationale: "Only unit tests exist".to_string(),
            effort: "2 days".to_string(),
            addresses: vec!["Testing".to_string()],
            priority: 2,
        });

        session.sort_recommendations();
        assert_eq!(session.recommendations[0].action, "Add input validation layer");
        assert_eq!(session.recommendations[1].action, "Add integration tests");
        assert_eq!(session.recommendations[2].action, "Document public API");
    }

    // ── Project context gathering ──────────────────────────────

    #[test]
    fn test_gather_project_context_no_root() {
        let session = SuperReviewSession::new();
        assert!(session.gather_project_context(10000).is_err());
    }

    #[test]
    fn test_gather_project_context_rust() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let tests = tmp.path().join("tests");
        let docs = tmp.path().join("docs");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tests).unwrap();
        fs::create_dir_all(&docs).unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "test-project"
version = "0.2.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = "1"
anyhow = "1"

[dev-dependencies]
tempfile = "3"
"#,
        )
        .unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("README.md"), "# Test Project\nA test project.").unwrap();

        let session = SuperReviewSession::new().with_project_root(tmp.path());
        let ctx = session.gather_project_context(10000).unwrap();

        assert_eq!(ctx.name, "test-project");
        assert_eq!(ctx.version, "0.2.0");
        assert_eq!(ctx.language, "Rust");
        assert!(ctx.has_tests);
        assert!(ctx.has_docs);
        assert!(ctx.dependencies.iter().any(|d| d == "serde"));
        assert!(ctx.dependencies.iter().any(|d| d == "tokio"));
        assert!(ctx.dependencies.iter().any(|d| d == "anyhow"));
        assert!(ctx.directory_structure.contains("src"));
    }

    #[test]
    fn test_gather_project_context_node() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "test-app", "version": "1.0.0", "description": "A test app", "dependencies": {"express": "^4.18"}}"#,
        )
        .unwrap();

        let session = SuperReviewSession::new().with_project_root(tmp.path());
        let ctx = session.gather_project_context(10000).unwrap();

        assert_eq!(ctx.name, "test-app");
        assert_eq!(ctx.language, "JavaScript/TypeScript");
    }

    #[test]
    fn test_gather_project_context_ci() {
        let tmp = tempfile::tempdir().unwrap();
        let workflows = tmp.path().join(".github").join("workflows");
        fs::create_dir_all(&workflows).unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"ci-test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(workflows.join("ci.yml"), "name: CI\non: push\n").unwrap();

        let session = SuperReviewSession::new().with_project_root(tmp.path());
        let ctx = session.gather_project_context(10000).unwrap();
        assert!(ctx.has_ci);
    }

    // ── Markdown rendering ─────────────────────────────────────

    fn minimal_session() -> SuperReviewSession {
        let mut session = SuperReviewSession::new();
        session.set_system_understanding(SystemUnderstanding {
            purpose: "A CLI tool for managing tasks".to_string(),
            users: "Developers".to_string(),
            core_contract: "Reliable task CRUD via CLI".to_string(),
            critical_paths: vec!["task create".to_string(), "task list".to_string()],
            constraints: vec!["Must work offline".to_string()],
            notes: String::new(),
        });
        session.project_context = Some(ProjectContext {
            name: "task-cli".to_string(),
            version: "1.0.0".to_string(),
            language: "Rust".to_string(),
            directory_structure: "src/\n  main.rs\n".to_string(),
            key_files: vec![],
            dependencies: vec!["clap".to_string()],
            has_tests: true,
            has_ci: false,
            has_docs: false,
        });
        session.add_domain(DomainAssessment {
            domain: "Error Handling".to_string(),
            relevance: "CLI tools must handle errors gracefully".to_string(),
            rating: Rating::Good,
            analysis: "Errors are propagated with context throughout".to_string(),
            strengths: vec![Finding {
                description: "Consistent use of anyhow".to_string(),
                severity: Severity::Nit,
                evidence: vec![],
            }],
            concerns: vec![Finding {
                description: "Some unwrap() in tests only".to_string(),
                severity: Severity::Nit,
                evidence: vec![],
            }],
            evidence: vec![Evidence {
                file: "src/main.rs".to_string(),
                lines: None,
                observation: "All error paths use .context()".to_string(),
            }],
        });
        session
    }

    #[test]
    fn test_render_markdown_minimal() {
        let session = minimal_session();
        let md = session.render_markdown().unwrap();

        assert!(md.contains("# Super-Review: task-cli"));
        assert!(md.contains("## System Understanding"));
        assert!(md.contains("A CLI tool for managing tasks"));
        assert!(md.contains("## Domain Assessments"));
        assert!(md.contains("### Error Handling — Good"));
        assert!(md.contains("Errors are propagated with context"));
        assert!(md.contains("**Strengths:**"));
        assert!(md.contains("**Concerns:**"));
        assert!(md.contains("**Evidence:**"));
        assert!(md.contains("`src/main.rs`"));
    }

    #[test]
    fn test_render_markdown_full() {
        let mut session = minimal_session();

        session.add_cross_cutting(CrossCuttingFinding {
            title: "Architecture and error handling are aligned".to_string(),
            description: "Module boundaries match error propagation boundaries".to_string(),
            connected_domains: vec!["Architecture".to_string(), "Error Handling".to_string()],
            evidence: vec![],
            severity: Severity::Nit,
        });

        session.set_working_well(vec!["Clean module structure".to_string()]);
        session.set_needs_attention(vec!["Add more integration tests".to_string()]);
        session.set_at_risk(vec!["CI is not configured".to_string()]);
        session.set_verdict("Solid foundation with room for improvement in testing and CI.");

        session.add_recommendation(Recommendation {
            action: "Set up CI pipeline".to_string(),
            rationale: "No CI means regressions are caught late".to_string(),
            effort: "2 hours".to_string(),
            addresses: vec!["Build & CI".to_string()],
            priority: 1,
        });

        let md = session.render_markdown().unwrap();

        assert!(md.contains("## Cross-Cutting Findings"));
        assert!(md.contains("Architecture and error handling are aligned"));
        assert!(md.contains("## System-Level Assessment"));
        assert!(md.contains("### What's Working Well"));
        assert!(md.contains("Clean module structure"));
        assert!(md.contains("### What Needs Attention"));
        assert!(md.contains("Add more integration tests"));
        assert!(md.contains("### What's At Risk"));
        assert!(md.contains("CI is not configured"));
        assert!(md.contains("## Verdict"));
        assert!(md.contains("Solid foundation"));
        assert!(md.contains("## Recommendations"));
        assert!(md.contains("Set up CI pipeline"));
    }

    #[test]
    fn test_render_markdown_no_system_understanding() {
        let mut session = SuperReviewSession::new();
        session.add_domain(DomainAssessment {
            domain: "Test".to_string(),
            relevance: "test".to_string(),
            rating: Rating::Adequate,
            analysis: "test".to_string(),
            strengths: vec![],
            concerns: vec![],
            evidence: vec![],
        });
        assert!(session.render_markdown().is_err());
    }

    #[test]
    fn test_render_markdown_no_domains() {
        let mut session = SuperReviewSession::new();
        session.set_system_understanding(SystemUnderstanding {
            purpose: "test".to_string(),
            users: String::new(),
            core_contract: String::new(),
            critical_paths: vec![],
            constraints: vec![],
            notes: String::new(),
        });
        session.project_context = Some(ProjectContext {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            language: "Rust".to_string(),
            directory_structure: String::new(),
            key_files: vec![],
            dependencies: vec![],
            has_tests: false,
            has_ci: false,
            has_docs: false,
        });
        assert!(session.render_markdown().is_err());
    }

    // ── File writing ───────────────────────────────────────────

    #[test]
    fn test_write_document_to_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = minimal_session();
        session.project_root = Some(tmp.path().to_path_buf());

        let path = session.write_document(None).unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("docs/review"));

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Super-Review: task-cli"));
    }

    #[test]
    fn test_write_document_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let session = minimal_session();

        let explicit = tmp.path().join("custom-review.md");
        let path = session.write_document(Some(&explicit)).unwrap();
        assert_eq!(path, explicit);
        assert!(path.exists());
    }

    #[test]
    fn test_write_document_no_understanding() {
        let session = SuperReviewSession::new();
        assert!(session.write_document(None).is_err());
    }

    #[test]
    fn test_write_document_no_domains() {
        let mut session = SuperReviewSession::new();
        session.set_system_understanding(SystemUnderstanding {
            purpose: "test".to_string(),
            users: String::new(),
            core_contract: String::new(),
            critical_paths: vec![],
            constraints: vec![],
            notes: String::new(),
        });
        assert!(session.write_document(None).is_err());
    }

    // ── Helpers ────────────────────────────────────────────────

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("API Layer Review!"), "api-layer-review");
        assert_eq!(slugify("foo_bar baz"), "foo-bar-baz");
        assert_eq!(slugify("  spaces  "), "spaces");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        let result = truncate_str("hello world foo bar", 12);
        assert!(result.len() <= 15); // 12 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_extract_toml_value() {
        let content = r#"[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"
"#;
        assert_eq!(extract_toml_value(content, "name"), Some("my-crate".to_string()));
        assert_eq!(extract_toml_value(content, "version"), Some("0.1.0".to_string()));
        assert_eq!(extract_toml_value(content, "missing"), None);
    }

    #[test]
    fn test_extract_toml_value_with_braces() {
        let content = r#"[package]
name = { workspace = true }
version = "0.1.0"
"#;
        // The braces form won't match the simple pattern, should return None
        assert_eq!(extract_toml_value(content, "name"), None);
        assert_eq!(extract_toml_value(content, "version"), Some("0.1.0".to_string()));
    }

    // ── Evidence display ───────────────────────────────────────

    #[test]
    fn test_evidence_display_with_lines() {
        let ev = Evidence {
            file: "src/main.rs".to_string(),
            lines: Some((42, 50)),
            observation: "Uses context".to_string(),
        };
        assert_eq!(format!("{}", ev), "`src/main.rs:42-50` — Uses context");
    }

    #[test]
    fn test_evidence_display_without_lines() {
        let ev = Evidence {
            file: "src/main.rs".to_string(),
            lines: None,
            observation: "General pattern".to_string(),
        };
        assert_eq!(format!("{}", ev), "`src/main.rs` — General pattern");
    }

    // ── Skill prompt ───────────────────────────────────────────

    #[test]
    fn test_super_review_skill_prompt() {
        let prompt = super_review_skill_prompt();
        assert!(prompt.contains("Super-Review Skill"));
        assert!(prompt.contains("Phase 1: Understand the System"));
        assert!(prompt.contains("Phase 2: Map the Domains"));
        assert!(prompt.contains("Phase 3: Deep Domain Analysis"));
        assert!(prompt.contains("Phase 4: Cross-Cutting Analysis"));
        assert!(prompt.contains("Phase 5: Produce the Review"));
        assert!(prompt.contains("Excellent"));
        assert!(prompt.contains("Critical"));
    }

    // ── Serialization roundtrips ───────────────────────────────

    #[test]
    fn test_session_serde_roundtrip() {
        let mut session = SuperReviewSession::new().with_scope("API layer");
        session.set_phase(Phase::Analyze);
        session.set_system_understanding(SystemUnderstanding {
            purpose: "Test".to_string(),
            users: "Devs".to_string(),
            core_contract: "Works".to_string(),
            critical_paths: vec!["create".to_string()],
            constraints: vec![],
            notes: String::new(),
        });
        session.add_domain(DomainAssessment {
            domain: "Testing".to_string(),
            relevance: "Must be reliable".to_string(),
            rating: Rating::Adequate,
            analysis: "Some tests".to_string(),
            strengths: vec![],
            concerns: vec![],
            evidence: vec![],
        });

        let json = serde_json::to_string(&session).unwrap();
        let parsed: SuperReviewSession = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.phase, Phase::Analyze);
        assert_eq!(parsed.scope, Some("API layer".to_string()));
        assert_eq!(parsed.domains.len(), 1);
        assert!(parsed.system_understanding.is_some());
    }

    #[test]
    fn test_report_types_serde_roundtrip() {
        let evidence = Evidence {
            file: "src/lib.rs".to_string(),
            lines: Some((10, 20)),
            observation: "Pattern observed".to_string(),
        };

        let finding = Finding {
            description: "Test finding".to_string(),
            severity: Severity::Important,
            evidence: vec![evidence.clone()],
        };

        let domain = DomainAssessment {
            domain: "Security".to_string(),
            relevance: "Important".to_string(),
            rating: Rating::Concerning,
            analysis: "Issues found".to_string(),
            strengths: vec![finding.clone()],
            concerns: vec![finding.clone()],
            evidence: vec![evidence],
        };

        let json = serde_json::to_string_pretty(&domain).unwrap();
        let parsed: DomainAssessment = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.domain, "Security");
        assert_eq!(parsed.rating, Rating::Concerning);
        assert_eq!(parsed.strengths.len(), 1);
        assert_eq!(parsed.concerns.len(), 1);
    }

    #[test]
    fn test_recommendation_serde_roundtrip() {
        let rec = Recommendation {
            action: "Fix the thing".to_string(),
            rationale: "It's broken".to_string(),
            effort: "2 hours".to_string(),
            addresses: vec!["Correctness".to_string()],
            priority: 1,
        };

        let json = serde_json::to_string(&rec).unwrap();
        let parsed: Recommendation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.action, "Fix the thing");
        assert_eq!(parsed.priority, 1);
    }
}
