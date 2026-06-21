//! Conventional-commit tool.
//!
//! Produces atomic, conventional-commits-formatted commits from the working
//! tree. The deterministic core (scope extraction, validation, Kahn
//! topological ordering, message formatting) needs no LLM and is unit-tested;
//! the [`CommitTool`] wraps it with optional LLM analysis via
//! [`oxi_ai::high_level::complete`].
//!
//! Ported from omp's `commit/` subsystem (~3,000 lines), keeping the
//! deterministic heuristics verbatim and replacing the agentic pipeline with a
//! single LLM analysis call plus a deterministic fallback.

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;

// ═══════════════════════════════════════════════════════════════════════════
// Core types
// ═══════════════════════════════════════════════════════════════════════════

/// Conventional commit type, per the Conventional Commits specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitType {
    /// A new feature.
    Feat,
    /// A bug fix.
    Fix,
    /// Documentation only changes.
    Docs,
    /// Changes that do not affect the meaning of the code (white-space,
    /// formatting, missing semi-colons, etc).
    Style,
    /// A code change that neither fixes a bug nor adds a feature.
    Refactor,
    /// A code change that improves performance.
    Perf,
    /// Adding missing tests or correcting existing ones.
    Test,
    /// Changes that affect the build system or external dependencies.
    Build,
    /// Changes to CI configuration files and scripts.
    Ci,
    /// Other changes that don't modify `src` or `test` files.
    Chore,
    /// Reverts a previous commit.
    Revert,
}

impl CommitType {
    /// Lowercase identifier used in commit headers (e.g. `"feat"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Feat => "feat",
            Self::Fix => "fix",
            Self::Docs => "docs",
            Self::Style => "style",
            Self::Refactor => "refactor",
            Self::Perf => "perf",
            Self::Test => "test",
            Self::Build => "build",
            Self::Ci => "ci",
            Self::Chore => "chore",
            Self::Revert => "revert",
        }
    }

    /// Parse a commit type from its lowercase identifier.
    ///
    /// Returns `None` for unknown identifiers.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "feat" => Some(Self::Feat),
            "fix" => Some(Self::Fix),
            "docs" => Some(Self::Docs),
            "style" => Some(Self::Style),
            "refactor" => Some(Self::Refactor),
            "perf" => Some(Self::Perf),
            "test" => Some(Self::Test),
            "build" => Some(Self::Build),
            "ci" => Some(Self::Ci),
            "chore" => Some(Self::Chore),
            "revert" => Some(Self::Revert),
            _ => None,
        }
    }
}

impl std::fmt::Display for CommitType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Keep-a-Changelog category for a single detail line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangelogCategory {
    /// New features for users.
    Added,
    /// Changes to existing functionality.
    Changed,
    /// Soon-to-be removed features.
    Deprecated,
    /// Removed features.
    Removed,
    /// Bug fixes.
    Fixed,
    /// Vulnerability fixes.
    Security,
    /// Internal changes invisible to users.
    Internal,
}

/// A single bullet point inside a conventional commit body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConventionalDetail {
    /// Detail text (conventionally ≤120 chars, ending with a period).
    pub text: String,
    /// Optional Keep-a-Changelog category driving changelog generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog_category: Option<ChangelogCategory>,
    /// Whether this detail is user-visible (versus internal).
    #[serde(default = "default_true")]
    pub user_visible: bool,
}

/// Default value for [`ConventionalDetail::user_visible`].
fn default_true() -> bool {
    true
}

/// Result of conventional-commit analysis — the heart of every generated
/// commit message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConventionalAnalysis {
    /// Conventional commit type (`feat`, `fix`, …).
    #[serde(rename = "type")]
    pub commit_type: CommitType,
    /// Optional scope (≤2 path segments, lowercase).
    pub scope: String,
    /// Bullet-point details for the commit body.
    pub details: Vec<ConventionalDetail>,
    /// Issue references (e.g. `["#42"]`).
    #[serde(default)]
    pub issue_refs: Vec<String>,
}

/// One atomic commit group produced by split-commit planning.
#[derive(Debug, Clone)]
pub struct CommitGroup {
    /// Stable group identifier.
    pub id: String,
    /// Files included in this commit.
    pub files: Vec<String>,
    /// Conventional analysis for this group.
    pub analysis: ConventionalAnalysis,
    /// Summary line.
    pub summary: String,
    /// IDs of groups that must be committed before this one.
    pub dependencies: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// numstat + scope extraction (deterministic, no LLM)
// ═══════════════════════════════════════════════════════════════════════════

/// One row of `git diff --numstat` output.
#[derive(Debug, Clone)]
pub struct NumstatEntry {
    /// Changed file path (as reported by git).
    pub path: String,
    /// Lines added.
    pub additions: usize,
    /// Lines deleted.
    pub deletions: usize,
}

/// A scope candidate derived from path analysis, ranked by weighted churn.
#[derive(Debug, Clone)]
pub struct ScopeCandidate {
    /// Candidate scope name (1–2 path segments).
    pub name: String,
    /// Weighted line churn (2-segment names boosted ×1.2, 1-segment ×0.8).
    pub weight: f64,
    /// Number of path segments in the scope name.
    pub segments: usize,
}

/// Lock files and other generated artifacts excluded from scope analysis.
///
/// Ported from omp's `commit/utils/exclusions.ts` — these churn heavily but
/// carry no semantic signal, so they would otherwise dominate scope ranking.
const EXCLUDED_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "shrinkwrap.yaml",
    "bun.lock",
    "bun.lockb",
    "deno.lock",
    "composer.lock",
    "Gemfile.lock",
    "poetry.lock",
    "Pipfile.lock",
    "pdm.lock",
    "uv.lock",
    "go.sum",
    "flake.lock",
    "pubspec.lock",
    "Podfile.lock",
    "Packages.resolved",
    "mix.lock",
    "packages.lock.json",
];

/// Suffixes marking a file as a generated lock artifact.
const EXCLUDED_SUFFIXES: &[&str] = &[
    ".lock.yml",
    ".lock.yaml",
    "-lock.yml",
    "-lock.yaml",
    "config.yml.lock",
    "config.yaml.lock",
    "settings.yml.lock",
    "settings.yaml.lock",
];

/// Returns `true` if `path` is a lock file or other generated artifact that
/// should be excluded from conventional-commit analysis.
pub fn is_excluded_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    EXCLUDED_FILES
        .iter()
        .any(|name| lower.ends_with(&name.to_ascii_lowercase()))
        || EXCLUDED_SUFFIXES
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

/// Directory segments too generic to count as a distinct change root.
const PLACEHOLDER_DIRS: &[&str] = &["src", "lib", "bin", "app", "cmd", "internal", "main"];

/// Extract a 1–2 segment directory component from a file path.
///
/// The final path segment (the filename) is never part of the component —
/// scopes are directory groupings. For a bare filename with no directory the
/// extension is stripped, so `README.md` yields `README`. For deeper paths
/// the first two directory segments are taken: `src/auth/login.rs` yields
/// `src/auth`, while `src/main.rs` yields `src`.
fn extract_path_component(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return String::new();
    }
    // Directory segments exclude the final (filename) segment.
    let dirs = &segments[..segments.len() - 1];
    if dirs.is_empty() {
        // Bare filename: use the stem without extension.
        return segments[0]
            .split('.')
            .next()
            .unwrap_or(segments[0])
            .to_string();
    }
    let take = dirs.len().min(2);
    dirs[..take].join("/")
}

/// Deterministically rank scope candidates from numstat by weighted line churn.
///
/// A port of omp's `analysis/scope.ts` that needs no LLM: each changed path is
/// mapped to a 1–2 segment component, churn is summed per component,
/// 2-segment components are boosted (×1.2) and single-segment ones dampened
/// (×0.8), then the list is sorted by descending weight.
pub fn extract_scope_candidates(numstat: &[NumstatEntry]) -> Vec<ScopeCandidate> {
    let mut components: HashMap<String, usize> = HashMap::new();
    for entry in numstat {
        if is_excluded_file(&entry.path) {
            continue;
        }
        let component = extract_path_component(&entry.path);
        if component.is_empty() {
            continue;
        }
        *components.entry(component).or_default() += entry.additions + entry.deletions;
    }

    let mut candidates: Vec<ScopeCandidate> = components
        .into_iter()
        .map(|(name, lines)| {
            let segments = name.split('/').count();
            ScopeCandidate {
                name,
                weight: lines as f64,
                segments,
            }
        })
        .collect();

    for candidate in &mut candidates {
        candidate.weight *= if candidate.segments >= 2 { 1.2 } else { 0.8 };
    }

    candidates.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
}

/// Returns `true` when the change set is too broad for a single scope.
///
/// Triggers when the top candidate holds <60% of total churn, or when there
/// are ≥3 distinct non-placeholder roots. Used to decide whether to collapse
/// the scope or keep a broad marker.
pub fn is_wide_change(numstat: &[NumstatEntry]) -> bool {
    let candidates = extract_scope_candidates(numstat);
    if candidates.is_empty() {
        return false;
    }
    let total: f64 = candidates.iter().map(|c| c.weight).sum();
    let top_share = if total > 0.0 {
        candidates[0].weight / total
    } else {
        0.0
    };
    let distinct_roots = candidates
        .iter()
        .filter(|c| {
            let root = c.name.split('/').next().unwrap_or("");
            !PLACEHOLDER_DIRS.contains(&root)
        })
        .count();
    top_share < 0.6 || distinct_roots >= 3
}

/// Parse `git diff --numstat` output into [`NumstatEntry`] rows.
///
/// Each line is `<additions>\t<deletions>\t<path>`. Binary files report `-`
/// for the counts, which parse to `0`.
pub fn parse_numstat(output: &str) -> Vec<NumstatEntry> {
    output.lines().filter_map(parse_numstat_line).collect()
}

fn parse_numstat_line(line: &str) -> Option<NumstatEntry> {
    let mut parts = line.splitn(3, '\t');
    let additions_raw = parts.next()?;
    let deletions_raw = parts.next()?;
    let path = parts.next()?;
    if path.is_empty() {
        return None;
    }
    let additions = additions_raw.parse::<usize>().unwrap_or(0);
    let deletions = deletions_raw.parse::<usize>().unwrap_or(0);
    Some(NumstatEntry {
        path: path.to_string(),
        additions,
        deletions,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Message formatting
// ═══════════════════════════════════════════════════════════════════════════

/// Format a conventional commit message from an analysis and summary line.
///
/// Produces `type(scope): summary` (or `type: summary` when the scope is
/// empty) followed by a blank line and `- detail` bullets, then an optional
/// `Refs` footer.
pub fn format_commit_message(analysis: &ConventionalAnalysis, summary: &str) -> String {
    let header = if analysis.scope.is_empty() {
        format!("{}: {}", analysis.commit_type, summary)
    } else {
        format!("{}({}): {}", analysis.commit_type, analysis.scope, summary)
    };

    let mut message = header;
    if !analysis.details.is_empty() {
        message.push_str("\n\n");
        message.push_str(
            &analysis
                .details
                .iter()
                .map(|d| format!("- {}", d.text))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    if !analysis.issue_refs.is_empty() {
        message.push_str("\n\n");
        message.push_str(
            &analysis
                .issue_refs
                .iter()
                .map(|r| format!("Refs {}", r))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    message
}

// ═══════════════════════════════════════════════════════════════════════════
// Validation
// ═══════════════════════════════════════════════════════════════════════════

/// Validate a commit summary line.
///
/// Returns a list of human-readable error strings (empty when valid).
pub fn validate_summary(summary: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if summary.trim().is_empty() {
        errors.push("Summary must not be empty".to_string());
    }
    if summary.chars().count() > 72 {
        errors.push("Summary exceeds 72 characters".to_string());
    }
    if summary.ends_with('.') {
        errors.push("Summary must not end with a period".to_string());
    }
    if summary.contains('\n') {
        errors.push("Summary must be a single line".to_string());
    }
    errors
}

/// Validate a commit scope.
///
/// Returns a list of human-readable error strings (empty when valid). An empty
/// scope is always valid.
pub fn validate_scope(scope: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if scope.is_empty() {
        return errors;
    }
    if scope.split('/').count() > 2 {
        errors.push("Scope has more than 2 segments".to_string());
    }
    if scope != scope.to_ascii_lowercase() {
        errors.push("Scope must be lowercase".to_string());
    }
    if !is_valid_scope_chars(scope) {
        errors.push("Scope contains invalid characters (allowed: a-z 0-9 - _ /)".to_string());
    }
    errors
}

/// Check that each `/`-separated segment matches `^[a-z0-9][a-z0-9_-]*$`.
fn is_valid_scope_chars(scope: &str) -> bool {
    for segment in scope.split('/') {
        if segment.is_empty() {
            return false;
        }
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return false;
        }
        if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            return false;
        }
    }
    true
}

/// Best-effort normalisation of a summary so it satisfies [`validate_summary`].
///
/// Trims, collapses to the first line, strips trailing periods, and truncates
/// to 72 characters at the last whole-word boundary.
pub fn normalize_summary(summary: &str) -> String {
    let first_line = summary.lines().next().unwrap_or("").trim();
    let mut s = first_line.trim_end_matches('.').trim().to_string();
    if s.chars().count() > 72 {
        let truncated: String = s.chars().take(72).collect();
        s = match truncated.rfind(' ') {
            Some(idx) => truncated[..idx]
                .trim_end_matches(|c: char| !c.is_alphanumeric())
                .to_string(),
            None => truncated,
        };
    }
    s
}

// ═══════════════════════════════════════════════════════════════════════════
// Topological ordering (Kahn's algorithm)
// ═══════════════════════════════════════════════════════════════════════════

/// Reorder `groups` in dependency order using Kahn's algorithm.
///
/// Each group's [`CommitGroup::dependencies`] lists IDs that must be committed
/// first. The slice is sorted in place so every dependency precedes its
/// dependents.
///
/// # Errors
///
/// Returns an error string when:
/// - a dependency references an unknown group id,
/// - a group depends on itself, or
/// - a dependency cycle is detected.
pub fn compute_dependency_order(groups: &mut [CommitGroup]) -> Result<(), String> {
    let n = groups.len();
    let id_to_index: HashMap<&str, usize> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.as_str(), i))
        .collect();

    let mut in_degree = vec![0usize; n];
    let mut edges: Vec<HashSet<usize>> = vec![HashSet::new(); n];

    for (idx, group) in groups.iter().enumerate() {
        for dep in &group.dependencies {
            let Some(&dep_idx) = id_to_index.get(dep.as_str()) else {
                return Err(format!(
                    "Unknown dependency '{}' referenced by group '{}'",
                    dep, group.id
                ));
            };
            if dep_idx == idx {
                return Err(format!("Group '{}' depends on itself", group.id));
            }
            if edges[dep_idx].insert(idx) {
                in_degree[idx] += 1;
            }
        }
    }

    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(current) = queue.pop_front() {
        order.push(current);
        let dependents: Vec<usize> = edges[current].iter().copied().collect();
        for next in dependents {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.push_back(next);
            }
        }
    }

    if order.len() != n {
        let cycle: Vec<String> = (0..n)
            .filter(|i| !order.contains(i))
            .map(|i| groups[i].id.clone())
            .collect();
        return Err(format!(
            "Dependency cycle detected among: {}",
            cycle.join(", ")
        ));
    }

    // Rank each group by its position in the topological order, then sort.
    let rank_by_id: HashMap<String, usize> = order
        .iter()
        .enumerate()
        .map(|(rank, &idx)| (groups[idx].id.clone(), rank))
        .collect();
    groups.sort_by_key(|g| rank_by_id.get(&g.id).copied().unwrap_or(usize::MAX));

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// LLM analysis
// ═══════════════════════════════════════════════════════════════════════════

/// System prompt steering the model toward a single structured analysis call.
const ANALYSIS_SYSTEM: &str = "\
You are a conventional-commits analysis engine. Given a git diff and ranked \
scope candidates, call the create_conventional_analysis tool exactly once with \
a conventional commit plan. Rules:\n\
- type: one of feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert.\n\
- scope: lowercase, at most two /-separated segments; pick the most relevant scope candidate when possible, or empty string.\n\
- summary: imperative mood, <=72 chars, no trailing period, single line.\n\
- details: one bullet per logical change, each <=120 chars ending with a period.\n\
- issueRefs: issue/PR references like #123, or omit.";

/// JSON schema for the `create_conventional_analysis` tool.
fn analysis_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": {
                "type": "string",
                "enum": ["feat","fix","docs","style","refactor","perf","test","build","ci","chore","revert"]
            },
            "scope": {
                "type": "string",
                "description": "Lowercase scope, at most two /-separated segments, or empty"
            },
            "summary": {
                "type": "string",
                "maxLength": 72,
                "description": "Imperative one-line summary, no trailing period"
            },
            "details": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "maxLength": 120},
                        "changelogCategory": {
                            "type": "string",
                            "enum": ["added","changed","deprecated","removed","fixed","security","internal"]
                        },
                        "userVisible": {"type": "boolean"}
                    },
                    "required": ["text"]
                }
            },
            "issueRefs": {
                "type": "array",
                "items": {"type": "string"}
            }
        },
        "required": ["type", "scope", "summary", "details"]
    })
}

/// Internal shape of the LLM response — analysis plus a one-line summary.
#[derive(Debug, Deserialize)]
struct LlmAnalysis {
    #[serde(rename = "type")]
    commit_type: CommitType,
    #[serde(default)]
    scope: String,
    summary: String,
    #[serde(default)]
    details: Vec<ConventionalDetail>,
    #[serde(default)]
    issue_refs: Vec<String>,
}

/// Ask the configured model for a conventional analysis of the diff.
///
/// Returns `(analysis, summary)`. Falls back to text-JSON parsing when the
/// model emits JSON instead of a tool call.
async fn generate_analysis(
    model: &oxi_ai::Model,
    diff: &str,
    candidates: &[ScopeCandidate],
    extra_context: Option<&str>,
) -> Result<(ConventionalAnalysis, String), String> {
    let scope_hint = if candidates.is_empty() {
        "(none — derive from the diff)".to_string()
    } else {
        candidates
            .iter()
            .take(5)
            .map(|c| format!("- {} (weight {:.0})", c.name, c.weight))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut user =
        format!("Ranked scope candidates (by churn):\n{scope_hint}\n\n--- diff ---\n{diff}");
    if let Some(ctx) = extra_context {
        user.push_str(&format!("\n\n--- additional context ---\n{ctx}"));
    }

    let mut context = oxi_ai::Context::new().with_system_prompt(ANALYSIS_SYSTEM);
    context.add_message(oxi_ai::Message::User(oxi_ai::UserMessage::new(user)));
    context.set_tools(vec![oxi_ai::Tool::new(
        "create_conventional_analysis",
        "Emit a conventional-commit analysis for the given diff.",
        analysis_tool_schema(),
    )]);

    let options = oxi_ai::StreamOptions {
        max_tokens: Some(2400),
        temperature: Some(0.2),
        ..Default::default()
    };

    let response = oxi_ai::complete(model, &context, Some(options))
        .await
        .map_err(|e| format!("LLM analysis failed: {e}"))?;

    parse_analysis_response(&response)
}

/// Extract `(analysis, summary)` from the model response.
///
/// Prefers a `create_conventional_analysis` tool call; falls back to the first
/// JSON object in any text block.
fn parse_analysis_response(
    msg: &oxi_ai::AssistantMessage,
) -> Result<(ConventionalAnalysis, String), String> {
    for block in &msg.content {
        if let oxi_ai::ContentBlock::ToolCall(call) = block
            && call.name == "create_conventional_analysis"
        {
            let plan: LlmAnalysis = serde_json::from_value(call.arguments.clone())
                .map_err(|e| format!("Invalid analysis tool arguments: {e}"))?;
            return Ok(split_plan(plan));
        }
    }

    let text = msg.text_content();
    if let Some(raw) = extract_json_object(&text) {
        let plan: LlmAnalysis =
            serde_json::from_str(&raw).map_err(|e| format!("Invalid analysis JSON: {e}"))?;
        return Ok(split_plan(plan));
    }

    Err("LLM did not return a conventional analysis".to_string())
}

fn split_plan(plan: LlmAnalysis) -> (ConventionalAnalysis, String) {
    let analysis = ConventionalAnalysis {
        commit_type: plan.commit_type,
        scope: plan.scope,
        details: plan.details,
        issue_refs: plan.issue_refs,
    };
    (analysis, plan.summary)
}

/// Extract the first balanced JSON object from `text`.
fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &byte) in bytes.iter().enumerate().skip(start) {
        let c = byte as char;
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(text[start..=i].to_string());
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Deterministic fallback (no LLM)
// ═══════════════════════════════════════════════════════════════════════════

/// Deterministically infer a [`ConventionalAnalysis`] when no LLM is available.
fn deterministic_analysis(
    entries: &[NumstatEntry],
    candidates: &[ScopeCandidate],
) -> ConventionalAnalysis {
    let commit_type = infer_commit_type(entries);
    let scope = candidates
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let details = deterministic_details(entries);
    ConventionalAnalysis {
        commit_type,
        scope,
        details,
        issue_refs: Vec::new(),
    }
}

/// Derive a sensible imperative summary from the commit type and scope.
fn deterministic_summary(commit_type: CommitType, scope: &str) -> String {
    let verb = match commit_type {
        CommitType::Feat => "Add",
        CommitType::Fix => "Fix",
        CommitType::Docs => "Document",
        CommitType::Refactor => "Refactor",
        CommitType::Test => "Add tests for",
        CommitType::Perf => "Optimize",
        CommitType::Build => "Update build config for",
        CommitType::Ci => "Update CI for",
        CommitType::Style => "Format",
        CommitType::Revert => "Revert",
        CommitType::Chore => "Update",
    };
    let target = if scope.is_empty() {
        "the project"
    } else {
        scope
    };
    normalize_summary(&format!("{verb} {target}"))
}

fn infer_commit_type(entries: &[NumstatEntry]) -> CommitType {
    let paths: Vec<&str> = entries
        .iter()
        .filter(|e| !is_excluded_file(&e.path))
        .map(|e| e.path.as_str())
        .collect();
    if paths.is_empty() {
        return CommitType::Chore;
    }
    if paths.iter().all(|p| is_doc_file(p)) {
        return CommitType::Docs;
    }
    if paths.iter().all(|p| is_test_file(p)) {
        return CommitType::Test;
    }
    if paths.iter().all(|p| is_ci_file(p)) {
        return CommitType::Ci;
    }
    if paths.iter().all(|p| is_build_file(p)) {
        return CommitType::Build;
    }
    CommitType::Chore
}

fn deterministic_details(entries: &[NumstatEntry]) -> Vec<ConventionalDetail> {
    entries
        .iter()
        .filter(|e| !is_excluded_file(&e.path))
        .take(6)
        .map(|e| ConventionalDetail {
            text: format!("Update {}.", short_path(&e.path)),
            changelog_category: None,
            user_visible: true,
        })
        .collect()
}

fn short_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(_, base)| base.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn is_doc_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.starts_with("docs/")
        || lower.contains("/docs/")
        || lower == "readme.md"
        || lower == "changelog.md"
        || lower == "license"
        || lower == "license.md"
}

fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("_test.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".test.js")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.js")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.ends_with("_test.go")
        || lower.ends_with("test.py")
        || lower.ends_with("_test.py")
}

fn is_ci_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/")
        || lower.starts_with("ci/")
        || lower.contains("/.gitlab-ci")
        || lower == ".gitlab-ci.yml"
        || lower == "dockerfile"
        || lower.ends_with("/dockerfile")
}

fn is_build_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("cargo.toml")
        || lower.ends_with("package.json")
        || lower.ends_with("tsconfig.json")
        || lower.ends_with("go.mod")
        || lower.ends_with("go.sum")
        || lower == "makefile"
        || lower == "justfile"
        || lower.ends_with("dockerfile")
        || lower.ends_with(".cmake")
}

// ═══════════════════════════════════════════════════════════════════════════
// Changelog (best-effort Keep-a-Changelog append)
// ═══════════════════════════════════════════════════════════════════════════

/// Title-case heading for a changelog category.
fn category_title(cat: ChangelogCategory) -> &'static str {
    match cat {
        ChangelogCategory::Added => "Added",
        ChangelogCategory::Changed => "Changed",
        ChangelogCategory::Deprecated => "Deprecated",
        ChangelogCategory::Removed => "Removed",
        ChangelogCategory::Fixed => "Fixed",
        ChangelogCategory::Security => "Security",
        ChangelogCategory::Internal => "Internal",
    }
}

/// Best-effort Keep-a-Changelog update.
///
/// Appends user-visible details carrying a [`ChangelogCategory`] under the
/// matching `### <Category>` sub-heading inside the `## [Unreleased]` section,
/// creating missing sub-headings as needed.
///
/// Returns `Ok(true)` if the file was modified, `Ok(false)` when there is
/// nothing to do (no file, no `[Unreleased]` section, or no categorised
/// details), and `Err` only on I/O failure. The commit is never rolled back
/// on failure — the caller treats this as a non-fatal warning.
fn update_changelog(root: &Path, analysis: &ConventionalAnalysis) -> std::io::Result<bool> {
    let by_category: Vec<(ChangelogCategory, String)> = analysis
        .details
        .iter()
        .filter(|d| d.user_visible)
        .filter_map(|d| {
            d.changelog_category
                .map(|cat| (cat, d.text.trim_end_matches('.').to_string()))
        })
        .collect();
    if by_category.is_empty() {
        return Ok(false);
    }

    let path = root.join("CHANGELOG.md");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };

    let marker = "[Unreleased]";
    let Some(marker_idx) = content.find(marker) else {
        return Ok(false);
    };
    let line_end = content[marker_idx..]
        .find('\n')
        .map(|n| marker_idx + n)
        .unwrap_or(content.len());
    let section_end = content[line_end..]
        .find("\n## ")
        .map(|n| line_end + n)
        .unwrap_or(content.len());

    let section = &content[line_end..section_end];
    let mut new_section = section.to_string();
    for (cat, text) in &by_category {
        let heading = format!("### {}\n", category_title(*cat));
        if let Some(hpos) = new_section.find(&heading) {
            let insert_at = hpos + heading.len();
            new_section.insert_str(insert_at, &format!("- {text}\n"));
        } else {
            if !new_section.is_empty() && !new_section.ends_with('\n') {
                new_section.push('\n');
            }
            new_section.push_str(&format!("\n### {}\n- {text}\n", category_title(*cat)));
        }
    }

    let mut new_content = String::with_capacity(content.len() + new_section.len());
    new_content.push_str(&content[..line_end]);
    new_content.push_str(&new_section);
    new_content.push_str(&content[section_end..]);
    std::fs::write(&path, new_content)?;
    Ok(true)
}

// ═══════════════════════════════════════════════════════════════════════════
// Git operations
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal git wrapper around [`std::process::Command`] for the commit tool.
struct GitOps {
    /// Working directory for git invocations.
    cwd: PathBuf,
}

impl GitOps {
    /// Create a git operator rooted at `cwd`.
    fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.cwd)
            .output()
            .map_err(|e| format!("Failed to run git {}: {e}", args.join(" ")))?;
        if !output.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn numstat(&self) -> Result<Vec<NumstatEntry>, String> {
        let output = self.run(&["diff", "--numstat", "HEAD"])?;
        Ok(parse_numstat(&output))
    }

    fn diff(&self) -> Result<String, String> {
        self.run(&["diff", "HEAD"])
    }

    fn stage_all(&self) -> Result<(), String> {
        self.run(&["add", "-A"])?;
        Ok(())
    }

    fn commit(&self, message: &str) -> Result<(), String> {
        self.run(&["commit", "-m", message])?;
        Ok(())
    }

    fn push(&self) -> Result<(), String> {
        self.run(&["push"])?;
        Ok(())
    }

    fn head_short(&self) -> Result<String, String> {
        let output = self.run(&["rev-parse", "--short", "HEAD"])?;
        Ok(output.trim().to_string())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CommitTool
// ═══════════════════════════════════════════════════════════════════════════

/// Parsed parameters for the commit tool.
#[derive(Debug, Default)]
struct CommitArgs {
    /// Preview without committing.
    dry_run: bool,
    /// Push after committing.
    push: bool,
    /// Skip the changelog update.
    no_changelog: bool,
    /// Extra context passed to the LLM analysis.
    context: Option<String>,
}

fn parse_args(params: &Value) -> Result<CommitArgs, String> {
    Ok(CommitArgs {
        dry_run: params["dry_run"].as_bool().unwrap_or(false),
        push: params["push"].as_bool().unwrap_or(false),
        no_changelog: params["no_changelog"].as_bool().unwrap_or(false),
        context: params["context"].as_str().map(String::from),
    })
}

/// Agent tool that produces conventional commits from the working tree.
///
/// Collects the git diff, deterministically extracts scope candidates, asks a
/// configured LLM for a conventional analysis (falling back to deterministic
/// heuristics when no model is set), validates and formats the message, then
/// either commits or previews (`dry_run`).
pub struct CommitTool {
    /// LLM model for conventional analysis. `None` selects deterministic mode.
    model: Option<oxi_ai::Model>,
}

impl CommitTool {
    /// Create a commit tool backed by the given LLM model.
    ///
    /// This is the constructor bootstrap uses to inject a resolved model.
    pub fn new(model: oxi_ai::Model) -> Self {
        Self { model: Some(model) }
    }

    /// Create a commit tool with no LLM model (deterministic-only mode).
    ///
    /// Used by the default built-in registry; bootstrap can replace it with a
    /// [`CommitTool::new`] instance once a model is resolved.
    pub fn unconfigured() -> Self {
        Self { model: None }
    }
}

#[async_trait]
impl AgentTool for CommitTool {
    fn name(&self) -> &str {
        "commit"
    }

    fn label(&self) -> &str {
        "Conventional Commit"
    }

    fn essential(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Analyze working-tree changes, extract a conventional commit scope, \
         generate a conventional commit message, and commit (or preview with \
         dry_run). Optionally update CHANGELOG.md and push."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "dry_run": {
                    "type": "boolean",
                    "description": "Preview the commit message without committing",
                    "default": false
                },
                "push": {
                    "type": "boolean",
                    "description": "Push after committing",
                    "default": false
                },
                "no_changelog": {
                    "type": "boolean",
                    "description": "Skip the CHANGELOG.md update",
                    "default": false
                },
                "context": {
                    "type": "string",
                    "description": "Optional extra context to guide the analysis"
                }
            }
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args = parse_args(&params)?;
        let cwd = ctx.root().to_path_buf();
        let git = GitOps::new(cwd.clone());

        // 1. Collect changes.
        let numstat = git.numstat()?;
        let filtered: Vec<NumstatEntry> = numstat
            .iter()
            .filter(|e| !is_excluded_file(&e.path))
            .cloned()
            .collect();
        if filtered.is_empty() {
            return Ok(AgentToolResult::success("No changes to commit."));
        }

        // 2. Deterministic scope extraction.
        let candidates = extract_scope_candidates(&numstat);

        // 3. Analysis: LLM when configured, deterministic fallback otherwise.
        let (mut analysis, mut summary) = match self.model.as_ref() {
            Some(model) => {
                let diff = git.diff()?;
                match generate_analysis(model, &diff, &candidates, args.context.as_deref()).await {
                    Ok(plan) => plan,
                    Err(e) => {
                        let det = deterministic_analysis(&filtered, &candidates);
                        let det_summary = deterministic_summary(det.commit_type, &det.scope);
                        tracing::warn!(
                            "commit tool: LLM analysis failed ({e}), using deterministic fallback"
                        );
                        (det, det_summary)
                    }
                }
            }
            None => {
                let det = deterministic_analysis(&filtered, &candidates);
                let det_summary = deterministic_summary(det.commit_type, &det.scope);
                (det, det_summary)
            }
        };

        // 4. Normalise + validate.
        summary = normalize_summary(&summary);
        analysis.scope = analysis.scope.trim().to_string();
        let validation = {
            let mut v = validate_summary(&summary);
            v.extend(validate_scope(&analysis.scope));
            v
        };

        // 5. Format message.
        let message = format_commit_message(&analysis, &summary);

        if args.dry_run {
            let mut output = String::new();
            if !validation.is_empty() {
                output.push_str("⚠ Validation warnings:\n");
                output.push_str(&validation.join("\n"));
                output.push_str("\n\n");
            }
            output.push_str("Dry run — would commit:\n\n");
            output.push_str(&message);
            return Ok(AgentToolResult::success(output).with_metadata(json!({
                "dry_run": true,
                "scope": analysis.scope,
                "type": analysis.commit_type.as_str(),
            })));
        }

        // Validation is advisory for real commits but surfaced if present.
        if !validation.is_empty() {
            tracing::warn!(
                "commit tool: validation warnings: {}",
                validation.join("; ")
            );
        }

        // 6. Stage + commit.
        git.stage_all()?;
        git.commit(&message)?;
        let hash = git.head_short().unwrap_or_else(|_| "unknown".to_string());

        // 7. Changelog (best-effort, non-fatal).
        if !args.no_changelog
            && let Err(e) = update_changelog(&cwd, &analysis)
        {
            tracing::warn!("commit tool: changelog update failed: {e}");
        }

        // 8. Push (optional).
        if args.push {
            git.push()?;
        }

        Ok(
            AgentToolResult::success(format!("Committed {hash}:\n\n{message}")).with_metadata(
                json!({
                    "hash": hash,
                    "scope": analysis.scope,
                    "type": analysis.commit_type.as_str(),
                }),
            ),
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, additions: usize, deletions: usize) -> NumstatEntry {
        NumstatEntry {
            path: path.to_string(),
            additions,
            deletions,
        }
    }

    // ── scope extraction ───────────────────────────────────────────────

    #[test]
    fn scope_extraction_single_component() {
        let numstat = vec![
            entry("src/auth/login.rs", 50, 10),
            entry("src/auth/logout.rs", 20, 5),
        ];
        let candidates = extract_scope_candidates(&numstat);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "src/auth");
        assert_eq!(candidates[0].segments, 2);
    }

    #[test]
    fn scope_extraction_ranks_by_churn() {
        let numstat = vec![
            entry("src/big/module.rs", 200, 50),
            entry("src/tiny/util.rs", 5, 1),
            entry("docs/readme.md", 3, 0),
        ];
        let candidates = extract_scope_candidates(&numstat);
        assert!(!candidates.is_empty());
        // The high-churn component should rank first.
        assert_eq!(candidates[0].name, "src/big");
    }

    #[test]
    fn scope_extraction_excludes_lock_files() {
        let numstat = vec![
            entry("Cargo.lock", 5000, 100),
            entry("src/main.rs", 10, 2),
            entry("package-lock.json", 9000, 0),
            entry("pnpm-lock.yaml", 300, 10),
            entry("go.sum", 800, 5),
        ];
        let candidates = extract_scope_candidates(&numstat);
        // Only src/main.rs survives; lock files are excluded.
        assert!(
            candidates
                .iter()
                .all(|c| !c.name.contains("lock") && !c.name.contains("sum"))
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "src");
    }

    #[test]
    fn scope_extraction_single_segment_boost() {
        let numstat = vec![entry("README.md", 10, 0)];
        let candidates = extract_scope_candidates(&numstat);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "README");
        // Single-segment weight is dampened (×0.8).
        assert!((candidates[0].weight - 8.0).abs() < 0.001);
    }

    #[test]
    fn wide_change_detection_many_roots() {
        let numstat = vec![
            entry("auth/login.rs", 30, 0),
            entry("billing/invoice.rs", 30, 0),
            entry("reports/export.rs", 30, 0),
        ];
        // Three distinct roots (auth, billing, reports) each holding 1/3 → wide.
        assert!(is_wide_change(&numstat));
    }

    #[test]
    fn wide_change_false_for_single_scope() {
        let numstat = vec![
            entry("src/auth/login.rs", 100, 10),
            entry("src/auth/session.rs", 20, 5),
        ];
        assert!(!is_wide_change(&numstat));
    }

    // ── numstat parsing ────────────────────────────────────────────────

    #[test]
    fn parse_numstat_basic() {
        let output = "10\t2\tsrc/main.rs\n3\t0\tdocs/readme.md\n";
        let entries = parse_numstat(output);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[0].additions, 10);
        assert_eq!(entries[0].deletions, 2);
        assert_eq!(entries[1].path, "docs/readme.md");
    }

    #[test]
    fn parse_numstat_binary_file() {
        let output = "-\t-\tassets/logo.png\n";
        let entries = parse_numstat(output);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "assets/logo.png");
        assert_eq!(entries[0].additions, 0);
        assert_eq!(entries[0].deletions, 0);
    }

    #[test]
    fn parse_numstat_skips_blank() {
        let output = "\n10\t2\tsrc/main.rs\n\n";
        let entries = parse_numstat(output);
        assert_eq!(entries.len(), 1);
    }

    // ── message format ─────────────────────────────────────────────────

    fn feat_auth_analysis() -> ConventionalAnalysis {
        ConventionalAnalysis {
            commit_type: CommitType::Feat,
            scope: "auth".to_string(),
            details: vec![ConventionalDetail {
                text: "Add OAuth2 login flow.".to_string(),
                changelog_category: Some(ChangelogCategory::Added),
                user_visible: true,
            }],
            issue_refs: vec!["#42".to_string()],
        }
    }

    #[test]
    fn message_format_with_scope_and_refs() {
        let analysis = feat_auth_analysis();
        let msg = format_commit_message(&analysis, "Add OAuth2 login");
        assert!(msg.starts_with("feat(auth): Add OAuth2 login"));
        assert!(msg.contains("- Add OAuth2 login flow."));
        assert!(msg.contains("Refs #42"));
    }

    #[test]
    fn message_format_without_scope() {
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Fix,
            scope: String::new(),
            details: vec![ConventionalDetail {
                text: "Correct off-by-one.".to_string(),
                changelog_category: None,
                user_visible: true,
            }],
            issue_refs: Vec::new(),
        };
        let msg = format_commit_message(&analysis, "Fix crash");
        assert!(msg.starts_with("fix: Fix crash\n\n- Correct off-by-one."));
        assert!(!msg.contains("Refs"));
    }

    #[test]
    fn message_format_empty_details() {
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Chore,
            scope: "deps".to_string(),
            details: Vec::new(),
            issue_refs: Vec::new(),
        };
        let msg = format_commit_message(&analysis, "Bump deps");
        assert_eq!(msg, "chore(deps): Bump deps");
    }

    #[test]
    fn message_format_multiple_refs() {
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Fix,
            scope: String::new(),
            details: Vec::new(),
            issue_refs: vec!["#1".to_string(), "#2".to_string()],
        };
        let msg = format_commit_message(&analysis, "Fix things");
        assert!(msg.contains("Refs #1\nRefs #2"));
    }

    // ── validation ─────────────────────────────────────────────────────

    #[test]
    fn validation_rejects_long_summary() {
        let long = "x".repeat(73);
        let errors = validate_summary(&long);
        assert!(errors.iter().any(|e| e.contains("72 characters")));
    }

    #[test]
    fn validation_accepts_max_length_summary() {
        let exact = "x".repeat(72);
        let errors = validate_summary(&exact);
        assert!(!errors.iter().any(|e| e.contains("72 characters")));
    }

    #[test]
    fn validation_rejects_trailing_period() {
        let errors = validate_summary("Add feature.");
        assert!(errors.iter().any(|e| e.contains("period")));
    }

    #[test]
    fn validation_rejects_multiline_summary() {
        let errors = validate_summary("line one\nline two");
        assert!(errors.iter().any(|e| e.contains("single line")));
    }

    #[test]
    fn validation_rejects_empty_summary() {
        let errors = validate_summary("   ");
        assert!(errors.iter().any(|e| e.contains("empty")));
    }

    #[test]
    fn validation_rejects_uppercase_scope() {
        let errors = validate_scope("Auth");
        assert!(errors.iter().any(|e| e.contains("lowercase")));
    }

    #[test]
    fn validation_rejects_three_segment_scope() {
        let errors = validate_scope("a/b/c");
        assert!(errors.iter().any(|e| e.contains("2 segments")));
    }

    #[test]
    fn validation_rejects_invalid_scope_chars() {
        let errors = validate_scope("auth config");
        assert!(errors.iter().any(|e| e.contains("invalid characters")));
    }

    #[test]
    fn validation_accepts_empty_scope() {
        assert!(validate_scope("").is_empty());
    }

    #[test]
    fn validation_accepts_two_segment_scope() {
        assert!(validate_scope("oxi-agent/auth").is_empty());
    }

    #[test]
    fn normalize_summary_strips_period_and_truncates() {
        assert_eq!(normalize_summary("Add feature."), "Add feature");
        let long = format!("{}.", "x".repeat(80));
        let normalized = normalize_summary(&long);
        assert!(normalized.chars().count() <= 72);
        assert!(!normalized.ends_with('.'));
    }

    #[test]
    fn normalize_summary_collapses_to_single_line() {
        assert_eq!(normalize_summary("first\nsecond"), "first");
    }

    // ── topological sort ───────────────────────────────────────────────

    fn group(id: &str, deps: &[&str]) -> CommitGroup {
        CommitGroup {
            id: id.to_string(),
            files: Vec::new(),
            analysis: ConventionalAnalysis {
                commit_type: CommitType::Feat,
                scope: String::new(),
                details: Vec::new(),
                issue_refs: Vec::new(),
            },
            summary: String::new(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn topo_sort_no_cycle() {
        let mut groups = vec![group("a", &[]), group("b", &["a"]), group("c", &["b"])];
        compute_dependency_order(&mut groups).expect("no cycle");
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn topo_sort_cycle_detected() {
        let mut groups = vec![group("a", &["b"]), group("b", &["a"])];
        let result = compute_dependency_order(&mut groups);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("cycle"));
    }

    #[test]
    fn topo_sort_unknown_dependency() {
        let mut groups = vec![group("a", &["nonexistent"])];
        let result = compute_dependency_order(&mut groups);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown dependency"));
    }

    #[test]
    fn topo_sort_self_dependency() {
        let mut groups = vec![group("a", &["a"])];
        let result = compute_dependency_order(&mut groups);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("itself"));
    }

    #[test]
    fn topo_sort_independent_groups_preserved() {
        let mut groups = vec![group("x", &[]), group("y", &[]), group("z", &[])];
        compute_dependency_order(&mut groups).expect("ok");
        // No dependencies → stable order preserved.
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["x", "y", "z"]);
    }

    #[test]
    fn topo_sort_diamond() {
        // d depends on b and c; b and c depend on a.
        let mut groups = vec![
            group("d", &["b", "c"]),
            group("c", &["a"]),
            group("b", &["a"]),
            group("a", &[]),
        ];
        compute_dependency_order(&mut groups).expect("no cycle");
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids[0], "a");
        assert_eq!(ids[3], "d");
        // b and c come after a and before d.
        let b_pos = ids.iter().position(|&i| i == "b").unwrap();
        let c_pos = ids.iter().position(|&i| i == "c").unwrap();
        assert!(b_pos > 0 && b_pos < 3);
        assert!(c_pos > 0 && c_pos < 3);
    }

    #[test]
    fn topo_sort_dedupes_repeated_dependency() {
        // Declaring the same dependency twice must not corrupt in-degree.
        let mut groups = vec![group("b", &["a", "a"]), group("a", &[])];
        compute_dependency_order(&mut groups).expect("no cycle");
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    // ── exclusions ─────────────────────────────────────────────────────

    #[test]
    fn excludes_common_lock_files() {
        assert!(is_excluded_file("Cargo.lock"));
        assert!(is_excluded_file("crates/foo/Cargo.lock"));
        assert!(is_excluded_file("package-lock.json"));
        assert!(is_excluded_file("yarn.lock"));
        assert!(is_excluded_file("pnpm-lock.yaml"));
        assert!(is_excluded_file("go.sum"));
        assert!(is_excluded_file("uv.lock"));
        assert!(is_excluded_file("flake.lock"));
        assert!(is_excluded_file("app/config.yaml.lock"));
    }

    #[test]
    fn does_not_exclude_source_files() {
        assert!(!is_excluded_file("src/main.rs"));
        assert!(!is_excluded_file("lib/index.ts"));
        assert!(!is_excluded_file("Cargo.toml"));
        assert!(!is_excluded_file("README.md"));
    }

    // ── CommitType ─────────────────────────────────────────────────────

    #[test]
    fn commit_type_roundtrip() {
        for id in [
            "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
            "revert",
        ] {
            let ty = CommitType::from_id(id).unwrap_or_else(|| panic!("unknown type {id}"));
            assert_eq!(ty.as_str(), id);
            assert_eq!(ty.to_string(), id);
        }
        assert!(CommitType::from_id("unknown").is_none());
    }

    // ── deterministic fallback ─────────────────────────────────────────

    #[test]
    fn deterministic_analysis_docs() {
        let entries = vec![entry("docs/guide.md", 20, 5)];
        let candidates = extract_scope_candidates(&entries);
        let analysis = deterministic_analysis(&entries, &candidates);
        assert_eq!(analysis.commit_type, CommitType::Docs);
        assert_eq!(analysis.scope, "docs");
        assert!(!analysis.details.is_empty());
    }

    #[test]
    fn deterministic_analysis_tests() {
        let entries = vec![entry("src/auth_test.rs", 40, 2)];
        let candidates = extract_scope_candidates(&entries);
        let analysis = deterministic_analysis(&entries, &candidates);
        assert_eq!(analysis.commit_type, CommitType::Test);
    }

    #[test]
    fn deterministic_summary_is_valid() {
        let summary = deterministic_summary(CommitType::Feat, "auth");
        assert!(validate_summary(&summary).is_empty());
        assert!(summary.contains("Add"));
    }

    // ── JSON extraction ────────────────────────────────────────────────

    #[test]
    fn extract_json_object_from_fence() {
        let text = "Here is the plan:\n```json\n{\"type\":\"fix\",\"scope\":\"a\"}\n```\n";
        let extracted = extract_json_object(text).expect("found json");
        assert!(extracted.contains("\"type\":\"fix\""));
    }

    #[test]
    fn extract_json_object_nested() {
        let text = "{\"a\":{\"b\":1},\"c\":2}";
        let extracted = extract_json_object(text).expect("found json");
        assert_eq!(extracted, text);
    }

    #[test]
    fn extract_json_object_with_brace_in_string() {
        let text = "{\"text\":\"has } brace\"}";
        let extracted = extract_json_object(text).expect("found json");
        assert_eq!(extracted, text);
    }

    // ── changelog ──────────────────────────────────────────────────────

    #[test]
    fn update_changelog_appends_under_unreleased() {
        let dir = tempfile::tempdir().expect("tempdir");
        let changelog = dir.path().join("CHANGELOG.md");
        std::fs::write(
            &changelog,
            "# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2024-01-01\n\n- initial\n",
        )
        .expect("write");
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Feat,
            scope: String::new(),
            details: vec![ConventionalDetail {
                text: "Add OAuth2 login.".to_string(),
                changelog_category: Some(ChangelogCategory::Added),
                user_visible: true,
            }],
            issue_refs: Vec::new(),
        };
        let modified = update_changelog(dir.path(), &analysis).expect("ok");
        assert!(modified);
        let content = std::fs::read_to_string(&changelog).expect("read");
        let unreleased_start = content.find("## [Unreleased]").unwrap();
        let v1_start = content.find("## [1.0.0]").unwrap();
        let unreleased = &content[unreleased_start..v1_start];
        assert!(unreleased.contains("### Added"));
        assert!(unreleased.contains("- Add OAuth2 login"));
    }

    #[test]
    fn update_changelog_skips_without_unreleased() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [1.0.0]\n",
        )
        .unwrap();
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Feat,
            scope: String::new(),
            details: vec![ConventionalDetail {
                text: "Add.".to_string(),
                changelog_category: Some(ChangelogCategory::Added),
                user_visible: true,
            }],
            issue_refs: Vec::new(),
        };
        let modified = update_changelog(dir.path(), &analysis).expect("ok");
        assert!(!modified);
    }

    #[test]
    fn update_changelog_no_file_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let analysis = ConventionalAnalysis {
            commit_type: CommitType::Feat,
            scope: String::new(),
            details: vec![ConventionalDetail {
                text: "Add.".to_string(),
                changelog_category: Some(ChangelogCategory::Added),
                user_visible: true,
            }],
            issue_refs: Vec::new(),
        };
        let modified = update_changelog(dir.path(), &analysis).expect("ok");
        assert!(!modified);
    }

    // ── parse_args ─────────────────────────────────────────────────────

    #[test]
    fn parse_args_defaults() {
        let args = parse_args(&json!({})).expect("ok");
        assert!(!args.dry_run);
        assert!(!args.push);
        assert!(!args.no_changelog);
        assert!(args.context.is_none());
    }

    #[test]
    fn parse_args_all_set() {
        let args = parse_args(
            &json!({"dry_run": true, "push": true, "no_changelog": true, "context": "ctx"}),
        )
        .expect("ok");
        assert!(args.dry_run);
        assert!(args.push);
        assert!(args.no_changelog);
        assert_eq!(args.context.as_deref(), Some("ctx"));
    }
}
