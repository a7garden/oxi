//! Deep-research skill for oxi
//!
//! Produces structured research reports by:
//! 1. Analyzing a codebase or topic
//! 2. Searching the web for information
//! 3. Comparing approaches
//! 4. Writing a research report to `docs/research/YYYY-MM-DD-<slug>.md`
//!
//! This module provides both:
//! - A [`DeepResearchSkill`] struct that can be used programmatically
//! - A skill content generator that produces the system-prompt instructions
//!   for the LLM-driven deep-research workflow

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Research report types ────────────────────────────────────────────

/// Configuration for a deep-research run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchConfig {
    /// Topic or question to research.
    pub topic: String,

    /// Working directory for the project (used to scope codebase analysis).
    pub working_dir: PathBuf,

    /// Output directory for research reports (default: `docs/research/`).
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,

    /// Maximum number of web search queries to execute.
    #[serde(default = "default_max_searches")]
    pub max_searches: usize,

    /// Whether to include codebase analysis in the report.
    #[serde(default = "default_true")]
    pub analyze_codebase: bool,

    /// Optional focus area to narrow the research scope.
    pub focus: Option<String>,
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("docs/research")
}

fn default_max_searches() -> usize {
    5
}

fn default_true() -> bool {
    true
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            topic: String::new(),
            working_dir: std::env::current_dir().unwrap_or_default(),
            output_dir: default_output_dir(),
            max_searches: default_max_searches(),
            analyze_codebase: true,
            focus: None,
        }
    }
}

/// A single web search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The search query used.
    pub query: String,
    /// Title of the result.
    pub title: String,
    /// URL of the result.
    pub url: String,
    /// Snippet or summary.
    pub snippet: String,
    /// Source engine (e.g., "ddg", "bing").
    pub source: String,
}

/// An approach comparison entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproachComparison {
    /// Name of the approach.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Pros of this approach.
    pub pros: Vec<String>,
    /// Cons of this approach.
    pub cons: Vec<String>,
    /// Complexity rating (1-5).
    pub complexity: u8,
    /// Suitability rating for the given topic (1-5).
    pub suitability: u8,
}

/// Codebase analysis findings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebaseAnalysis {
    /// Relevant files found.
    pub files: Vec<String>,
    /// Key patterns identified.
    pub patterns: Vec<String>,
    /// Dependencies relevant to the topic.
    pub dependencies: Vec<String>,
    /// Summary of findings.
    pub summary: String,
}

/// A complete research report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchReport {
    /// Report metadata.
    pub meta: ReportMeta,
    /// The original research topic.
    pub topic: String,
    /// Optional focus area.
    pub focus: Option<String>,
    /// Executive summary.
    pub summary: String,
    /// Background context.
    pub background: String,
    /// Findings from codebase analysis (if performed).
    pub codebase_analysis: Option<CodebaseAnalysis>,
    /// Web search results grouped by query.
    pub search_results: Vec<SearchResult>,
    /// Comparison of approaches.
    pub approaches: Vec<ApproachComparison>,
    /// Recommended approach.
    pub recommendation: String,
    /// Action items / next steps.
    pub next_steps: Vec<String>,
    /// References (URLs, docs).
    pub references: Vec<Reference>,
}

/// Metadata for a research report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportMeta {
    /// ISO date string (YYYY-MM-DD).
    pub date: String,
    /// URL-friendly slug derived from the topic.
    pub slug: String,
    /// Version of the report (incremented on updates).
    pub version: u32,
}

/// A reference / citation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    /// Title of the reference.
    pub title: String,
    /// URL or path.
    pub url: String,
    /// Type of reference.
    #[serde(rename = "type")]
    pub ref_type: ReferenceType,
}

/// Type of reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceType {
    /// Web page or article.
    Web,
    /// Documentation page.
    Documentation,
    /// Source code file.
    Code,
    /// Academic paper.
    Paper,
    /// Other / unknown.
    Other,
}

// ── Slug generation ──────────────────────────────────────────────────

/// Convert a topic string into a URL-friendly slug.
///
/// ```
/// assert_eq!(
///     oxi::skills::deep_research::slugify("What is the best ORM for Rust?"),
///     "what-is-the-best-orm-for-rust"
/// );
/// ```
pub fn slugify(topic: &str) -> String {
    let mut slug = String::with_capacity(topic.len());
    let mut prev_dash = false;

    for ch in topic.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if ch == ' ' || ch == '_' || ch == '-' {
            if !prev_dash && !slug.is_empty() {
                slug.push('-');
                prev_dash = true;
            }
        }
        // Skip other characters (punctuation, etc.)
    }

    // Remove trailing dash
    if slug.ends_with('-') {
        slug.pop();
    }

    slug
}

// ── Deep-research skill ──────────────────────────────────────────────

/// The deep-research skill.
///
/// This struct provides methods to:
/// - Generate a filename for a research report
/// - Render a report as markdown
/// - Write a report to disk
/// - Produce the skill instructions that get injected into the LLM system prompt
///
/// The actual research (web searches, codebase analysis) is orchestrated by
/// the LLM agent using its tools (bash, read, web_search). This skill provides
/// the structure, templates, and I/O.
pub struct DeepResearchSkill;

impl DeepResearchSkill {
    /// Create a new deep-research skill instance.
    pub fn new() -> Self {
        Self
    }

    /// Generate the output filename for a research report.
    ///
    /// Format: `YYYY-MM-DD-<slug>.md`
    pub fn report_filename(topic: &str) -> String {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let slug = slugify(topic);
        format!("{}-{}.md", date, slug)
    }

    /// Generate the full output path for a report.
    pub fn report_path(config: &ResearchConfig) -> PathBuf {
        config.output_dir.join(Self::report_filename(&config.topic))
    }

    /// Render a [`ResearchReport`] as a markdown string.
    pub fn render_markdown(report: &ResearchReport) -> String {
        let mut md = String::with_capacity(4096);

        // Title and metadata
        md.push_str(&format!("# {}\n\n", report.topic));
        md.push_str(&format!(
            "> Date: {} | Version: {}\n",
            report.meta.date, report.meta.version
        ));
        if let Some(ref focus) = report.focus {
            md.push_str(&format!("> Focus: {}\n", focus));
        }
        md.push('\n');

        // Executive summary
        md.push_str("## Summary\n\n");
        md.push_str(&report.summary);
        md.push_str("\n\n");

        // Background
        md.push_str("## Background\n\n");
        md.push_str(&report.background);
        md.push_str("\n\n");

        // Codebase analysis
        if let Some(ref analysis) = report.codebase_analysis {
            md.push_str("## Codebase Analysis\n\n");
            md.push_str(&analysis.summary);
            md.push_str("\n\n");

            if !analysis.files.is_empty() {
                md.push_str("### Relevant Files\n\n");
                for file in &analysis.files {
                    md.push_str(&format!("- `{}`\n", file));
                }
                md.push('\n');
            }

            if !analysis.patterns.is_empty() {
                md.push_str("### Patterns\n\n");
                for pattern in &analysis.patterns {
                    md.push_str(&format!("- {}\n", pattern));
                }
                md.push('\n');
            }

            if !analysis.dependencies.is_empty() {
                md.push_str("### Dependencies\n\n");
                for dep in &analysis.dependencies {
                    md.push_str(&format!("- {}\n", dep));
                }
                md.push('\n');
            }
        }

        // Search findings
        if !report.search_results.is_empty() {
            md.push_str("## Research Findings\n\n");
            let mut i = 1;
            for result in &report.search_results {
                md.push_str(&format!(
                    "### {}. {} [{}]({})\n\n{}\n\nSource: {}\n\n",
                    i, result.title, result.query, result.url, result.snippet, result.source
                ));
                i += 1;
            }
        }

        // Approach comparison
        if !report.approaches.is_empty() {
            md.push_str("## Approach Comparison\n\n");
            md.push_str("| Approach | Complexity | Suitability | Pros | Cons |\n");
            md.push_str("|----------|-----------|-------------|------|------|\n");
            for approach in &report.approaches {
                let pros = approach.pros.join(", ");
                let cons = approach.cons.join(", ");
                md.push_str(&format!(
                    "| {} | {}/5 | {}/5 | {} | {} |\n",
                    approach.name, approach.complexity, approach.suitability, pros, cons,
                ));
            }
            md.push('\n');

            // Detailed breakdowns
            for approach in &report.approaches {
                md.push_str(&format!("### {}\n\n", approach.name));
                md.push_str(&format!("{}\n\n", approach.description));
                md.push_str("**Pros:**\n");
                for pro in &approach.pros {
                    md.push_str(&format!("- {}\n", pro));
                }
                md.push_str("\n**Cons:**\n");
                for con in &approach.cons {
                    md.push_str(&format!("- {}\n", con));
                }
                md.push_str(&format!(
                    "\nComplexity: {}/5 | Suitability: {}/5\n\n",
                    approach.complexity, approach.suitability
                ));
            }
        }

        // Recommendation
        md.push_str("## Recommendation\n\n");
        md.push_str(&report.recommendation);
        md.push_str("\n\n");

        // Next steps
        if !report.next_steps.is_empty() {
            md.push_str("## Next Steps\n\n");
            for (i, step) in report.next_steps.iter().enumerate() {
                md.push_str(&format!("{}. {}\n", i + 1, step));
            }
            md.push('\n');
        }

        // References
        if !report.references.is_empty() {
            md.push_str("## References\n\n");
            for reference in &report.references {
                let type_label = match reference.ref_type {
                    ReferenceType::Web => "🌐",
                    ReferenceType::Documentation => "📖",
                    ReferenceType::Code => "💻",
                    ReferenceType::Paper => "📄",
                    ReferenceType::Other => "🔗",
                };
                md.push_str(&format!(
                    "- {} [{}]({})\n",
                    type_label, reference.title, reference.url
                ));
            }
            md.push('\n');
        }

        md
    }

    /// Write a research report to disk.
    ///
    /// Creates the output directory if it doesn't exist.
    /// Returns the path to the written file.
    pub fn write_report(config: &ResearchConfig, report: &ResearchReport) -> Result<PathBuf> {
        let output_dir = if config.output_dir.is_absolute() {
            config.output_dir.clone()
        } else {
            config.working_dir.join(&config.output_dir)
        };

        // Create output directory
        fs::create_dir_all(&output_dir).with_context(|| {
            format!(
                "Failed to create output directory: {}",
                output_dir.display()
            )
        })?;

        let filename = Self::report_filename(&config.topic);
        let path = output_dir.join(&filename);

        let markdown = Self::render_markdown(report);

        fs::write(&path, &markdown)
            .with_context(|| format!("Failed to write report to {}", path.display()))?;

        tracing::info!("Research report written to {}", path.display());
        Ok(path)
    }

    /// Generate the skill instructions to be injected into the system prompt
    /// when the deep-research skill is active.
    ///
    /// This tells the LLM how to conduct deep research using the tools
    /// available to it (bash, read, write, web_search).
    pub fn skill_instructions() -> String {
        include_str!("deep_research_prompt.md").to_string()
    }

    /// Analyze a project directory to find files relevant to a topic.
    ///
    /// This performs a lightweight static analysis: scanning filenames,
    /// reading key config files (Cargo.toml, package.json), and identifying
    /// patterns. It does NOT use AI — it's a heuristic pre-pass to give
    /// the researcher context before the LLM-driven phase.
    pub fn analyze_project(dir: &Path, topic_keywords: &[&str]) -> Result<CodebaseAnalysis> {
        let mut files = Vec::new();
        let mut patterns = Vec::new();
        let mut dependencies = Vec::new();

        // Walk the directory tree (depth-limited)
        Self::walk_dir(dir, "", 0, 4, topic_keywords, &mut files, &mut patterns)?;

        // Read dependency files
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(content) = fs::read_to_string(&cargo_toml) {
                Self::extract_cargo_deps(&content, topic_keywords, &mut dependencies);
                patterns.push("Rust project (Cargo)".to_string());
            }
        }

        let package_json = dir.join("package.json");
        if package_json.exists() {
            if let Ok(content) = fs::read_to_string(&package_json) {
                Self::extract_npm_deps(&content, topic_keywords, &mut dependencies);
                patterns.push("Node.js project (npm/yarn)".to_string());
            }
        }

        // Check for common patterns
        if dir.join("src").is_dir() {
            patterns.push("Standard src/ layout".to_string());
        }
        if dir.join("tests").is_dir() {
            patterns.push("Has tests/ directory".to_string());
        }
        if dir.join(".github").is_dir() {
            patterns.push("GitHub Actions CI".to_string());
        }
        if dir.join("Dockerfile").exists() {
            patterns.push("Dockerized".to_string());
        }

        let file_count = files.len();
        let summary = format!(
            "Found {} relevant file(s) across the project. {} pattern(s) and {} related dep(s) identified.",
            file_count,
            patterns.len(),
            dependencies.len()
        );

        Ok(CodebaseAnalysis {
            files,
            patterns,
            dependencies,
            summary,
        })
    }

    /// Recursively walk a directory, collecting relevant files.
    fn walk_dir(
        dir: &Path,
        prefix: &str,
        depth: usize,
        max_depth: usize,
        keywords: &[&str],
        files: &mut Vec<String>,
        patterns: &mut Vec<String>,
    ) -> Result<()> {
        if depth > max_depth {
            return Ok(());
        }

        let entries = fs::read_dir(dir)
            .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden dirs and common noise
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "__pycache__"
                || name == "dist"
                || name == "build"
                || name == ".git"
                || name == "vendor"
                || name == "coverage"
            {
                continue;
            }

            let path = entry.path();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };

            if path.is_dir() {
                Self::walk_dir(&path, &rel, depth + 1, max_depth, keywords, files, patterns)?;
            } else {
                // Check if the filename or extension is relevant
                let name_lower = name.to_lowercase();

                // Always include config files
                let is_config = matches!(
                    name_lower.as_str(),
                    "cargo.toml"
                        | "package.json"
                        | "tsconfig.json"
                        | "pyproject.toml"
                        | "go.mod"
                        | "makefile"
                        | "dockerfile"
                        | "docker-compose.yml"
                        | "docker-compose.yaml"
                        | ".env.example"
                        | "readme.md"
                        | "license"
                );

                // Check keyword relevance
                let keyword_match = keywords.iter().any(|kw| {
                    let kw_lower = kw.to_lowercase();
                    // Match keyword in filename
                    name_lower.contains(&kw_lower)
                    // Also check common source extensions
                    || Self::is_source_file(&name_lower) && !keywords.is_empty()
                });

                if is_config || keyword_match || keywords.is_empty() {
                    files.push(rel);
                }
            }
        }

        Ok(())
    }

    fn is_source_file(name: &str) -> bool {
        name.ends_with(".rs")
            || name.ends_with(".ts")
            || name.ends_with(".js")
            || name.ends_with(".py")
            || name.ends_with(".go")
            || name.ends_with(".java")
            || name.ends_with(".tsx")
            || name.ends_with(".jsx")
    }

    /// Extract relevant dependencies from a Cargo.toml file.
    fn extract_cargo_deps(content: &str, keywords: &[&str], deps: &mut Vec<String>) {
        let in_deps = content
            .lines()
            .skip_while(|line| {
                line.trim() != "[dependencies]" && line.trim() != "[dev-dependencies]"
            })
            .take_while(|line| {
                !line.starts_with('[')
                    || line.trim() == "[dependencies]"
                    || line.trim() == "[dev-dependencies]"
            });

        for line in in_deps {
            let line = line.trim();
            if line.starts_with('[') || line.is_empty() {
                continue;
            }
            if let Some((name, _rest)) = line.split_once('=') {
                let name = name.trim();
                if keywords.is_empty() {
                    deps.push(format!("{} (Rust crate)", name));
                } else {
                    let name_lower = name.to_lowercase();
                    let relevant = keywords
                        .iter()
                        .any(|kw| name_lower.contains(&kw.to_lowercase()));
                    if relevant {
                        deps.push(format!("{} (Rust crate)", name));
                    }
                }
            }
        }
    }

    /// Extract relevant dependencies from a package.json file.
    fn extract_npm_deps(content: &str, keywords: &[&str], deps: &mut Vec<String>) {
        // Simple heuristic parse — avoid pulling in a full JSON parser dependency
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
            for section in &["dependencies", "devDependencies"] {
                if let Some(obj) = json.get(section).and_then(|v| v.as_object()) {
                    for name in obj.keys() {
                        if keywords.is_empty() {
                            deps.push(format!("{} (npm)", name));
                        } else {
                            let name_lower = name.to_lowercase();
                            let relevant = keywords
                                .iter()
                                .any(|kw| name_lower.contains(&kw.to_lowercase()));
                            if relevant {
                                deps.push(format!("{} (npm)", name));
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Default for DeepResearchSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DeepResearchSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeepResearchSkill").finish()
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_slugify_simple() {
        assert_eq!(
            slugify("What is the best ORM for Rust?"),
            "what-is-the-best-orm-for-rust"
        );
    }

    #[test]
    fn test_slugify_with_special_chars() {
        assert_eq!(
            slugify("React vs. Vue: A Comparison (2024)"),
            "react-vs-vue-a-comparison-2024"
        );
    }

    #[test]
    fn test_slugify_with_underscores() {
        assert_eq!(slugify("my_important_topic"), "my-important-topic");
    }

    #[test]
    fn test_slugify_consecutive_spaces() {
        assert_eq!(slugify("hello   world"), "hello-world");
    }

    #[test]
    fn test_slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn test_slugify_only_special() {
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn test_report_filename() {
        let filename = DeepResearchSkill::report_filename("Best database for Rust");
        let date = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(filename, format!("{}-best-database-for-rust.md", date));
    }

    #[test]
    fn test_report_path() {
        let config = ResearchConfig {
            topic: "Test Topic".to_string(),
            output_dir: PathBuf::from("docs/research"),
            ..Default::default()
        };
        let path = DeepResearchSkill::report_path(&config);
        assert!(path.to_string_lossy().contains("docs/research"));
        assert!(path.to_string_lossy().ends_with(".md"));
    }

    #[test]
    fn test_render_markdown_minimal() {
        let report = ResearchReport {
            meta: ReportMeta {
                date: "2024-01-15".to_string(),
                slug: "test-topic".to_string(),
                version: 1,
            },
            topic: "Test Topic".to_string(),
            focus: None,
            summary: "This is a test summary.".to_string(),
            background: "Some background info.".to_string(),
            codebase_analysis: None,
            search_results: vec![],
            approaches: vec![],
            recommendation: "Do the thing.".to_string(),
            next_steps: vec![],
            references: vec![],
        };

        let md = DeepResearchSkill::render_markdown(&report);
        assert!(md.contains("# Test Topic"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("This is a test summary."));
        assert!(md.contains("## Background"));
        assert!(md.contains("## Recommendation"));
        assert!(md.contains("Do the thing."));
    }

    #[test]
    fn test_render_markdown_full() {
        let report = ResearchReport {
            meta: ReportMeta {
                date: "2024-03-01".to_string(),
                slug: "auth-strategies".to_string(),
                version: 2,
            },
            topic: "Authentication Strategies".to_string(),
            focus: Some("JWT vs Session-based".to_string()),
            summary: "Comparison of auth strategies.".to_string(),
            background: "Web apps need auth.".to_string(),
            codebase_analysis: Some(CodebaseAnalysis {
                files: vec!["src/auth.rs".to_string(), "src/middleware.rs".to_string()],
                patterns: vec!["Middleware pattern".to_string()],
                dependencies: vec!["jsonwebtoken (Rust crate)".to_string()],
                summary: "Found auth-related files.".to_string(),
            }),
            search_results: vec![SearchResult {
                query: "JWT vs session auth".to_string(),
                title: "JWT vs Session Authentication".to_string(),
                url: "https://example.com/jwt-vs-session".to_string(),
                snippet: "A comparison of authentication methods.".to_string(),
                source: "ddg".to_string(),
            }],
            approaches: vec![ApproachComparison {
                name: "JWT".to_string(),
                description: "Stateless token-based auth.".to_string(),
                pros: vec!["Stateless".to_string(), "Scalable".to_string()],
                cons: vec!["Token revocation is hard".to_string()],
                complexity: 3,
                suitability: 4,
            }],
            recommendation: "Use JWT for this project.".to_string(),
            next_steps: vec!["Implement JWT middleware".to_string()],
            references: vec![Reference {
                title: "JWT RFC".to_string(),
                url: "https://tools.ietf.org/html/rfc7519".to_string(),
                ref_type: ReferenceType::Documentation,
            }],
        };

        let md = DeepResearchSkill::render_markdown(&report);
        assert!(md.contains("# Authentication Strategies"));
        assert!(md.contains("> Focus: JWT vs Session-based"));
        assert!(md.contains("## Codebase Analysis"));
        assert!(md.contains("`src/auth.rs`"));
        assert!(md.contains("## Research Findings"));
        assert!(md.contains("## Approach Comparison"));
        assert!(md.contains("| JWT |"));
        assert!(md.contains("## Next Steps"));
        assert!(md.contains("1. Implement JWT middleware"));
        assert!(md.contains("## References"));
        assert!(md.contains("JWT RFC"));
    }

    #[test]
    fn test_write_report_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config = ResearchConfig {
            topic: "Test Report".to_string(),
            working_dir: tmp.path().to_path_buf(),
            output_dir: PathBuf::from("docs/research"),
            ..Default::default()
        };

        let report = ResearchReport {
            meta: ReportMeta {
                date: "2024-01-01".to_string(),
                slug: "test-report".to_string(),
                version: 1,
            },
            topic: "Test Report".to_string(),
            focus: None,
            summary: "Test summary.".to_string(),
            background: "Test background.".to_string(),
            codebase_analysis: None,
            search_results: vec![],
            approaches: vec![],
            recommendation: "Test recommendation.".to_string(),
            next_steps: vec![],
            references: vec![],
        };

        let path = DeepResearchSkill::write_report(&config, &report).unwrap();
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Test Report"));
        assert!(content.contains("Test summary."));
    }

    #[test]
    fn test_write_report_absolute_output_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let abs_dir = tmp.path().join("output").join("research");

        let config = ResearchConfig {
            topic: "Absolute Path Test".to_string(),
            working_dir: tmp.path().to_path_buf(),
            output_dir: abs_dir.clone(),
            ..Default::default()
        };

        let report = ResearchReport {
            meta: ReportMeta {
                date: "2024-06-15".to_string(),
                slug: "absolute-path-test".to_string(),
                version: 1,
            },
            topic: "Absolute Path Test".to_string(),
            focus: None,
            summary: "Testing absolute paths.".to_string(),
            background: "Context.".to_string(),
            codebase_analysis: None,
            search_results: vec![],
            approaches: vec![],
            recommendation: "Works.".to_string(),
            next_steps: vec![],
            references: vec![],
        };

        let path = DeepResearchSkill::write_report(&config, &report).unwrap();
        assert!(path.exists());
        assert!(path.starts_with(&abs_dir));
    }

    #[test]
    fn test_analyze_project_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let analysis = DeepResearchSkill::analyze_project(tmp.path(), &[]).unwrap();
        // Empty dir, no keywords — should still return valid analysis
        assert!(
            analysis.files.is_empty()
                || analysis
                    .files
                    .iter()
                    .any(|f| f.contains("Cargo.toml") || f.contains("package.json"))
        );
    }

    #[test]
    fn test_analyze_project_rust_project() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a minimal Rust project structure
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "test-project"
version = "0.1.0"

[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = "1"
"#,
        )
        .unwrap();
        fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();

        let analysis = DeepResearchSkill::analyze_project(tmp.path(), &["serde"]).unwrap();
        assert!(analysis.patterns.iter().any(|p| p.contains("Rust")));
        assert!(analysis.dependencies.iter().any(|d| d.contains("serde")));
    }

    #[test]
    fn test_analyze_project_npm_project() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"dependencies": {"express": "^4.18.0", "lodash": "^4.17.21"}}"#,
        )
        .unwrap();
        fs::write(src_dir.join("index.ts"), "console.log('hi')").unwrap();

        let analysis = DeepResearchSkill::analyze_project(tmp.path(), &["express"]).unwrap();
        assert!(analysis.patterns.iter().any(|p| p.contains("Node.js")));
        assert!(analysis.dependencies.iter().any(|d| d.contains("express")));
    }

    #[test]
    fn test_analyze_project_skips_hidden_and_noise() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::create_dir_all(tmp.path().join("target")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let analysis = DeepResearchSkill::analyze_project(tmp.path(), &[]).unwrap();
        // Should not include .git, target, or node_modules in files
        for file in &analysis.files {
            assert!(!file.starts_with(".git/"), "Should skip .git: {}", file);
            assert!(!file.starts_with("target/"), "Should skip target: {}", file);
            assert!(
                !file.starts_with("node_modules/"),
                "Should skip node_modules: {}",
                file
            );
        }
    }

    #[test]
    fn test_analyze_project_depth_limited() {
        let tmp = tempfile::tempdir().unwrap();
        // Create deeply nested structure
        let deep = tmp
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("d")
            .join("e")
            .join("f");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.txt"), "content").unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let analysis = DeepResearchSkill::analyze_project(tmp.path(), &[]).unwrap();
        // deep.txt should not be found (depth > 4)
        assert!(!analysis.files.iter().any(|f| f.contains("deep.txt")));
    }

    #[test]
    fn test_skill_instructions_not_empty() {
        let instructions = DeepResearchSkill::skill_instructions();
        assert!(!instructions.is_empty());
    }

    #[test]
    fn test_research_config_default() {
        let config = ResearchConfig::default();
        assert!(config.topic.is_empty());
        assert_eq!(config.max_searches, 5);
        assert!(config.analyze_codebase);
        assert!(config.focus.is_none());
        assert_eq!(config.output_dir, PathBuf::from("docs/research"));
    }

    #[test]
    fn test_research_config_serde_roundtrip() {
        let config = ResearchConfig {
            topic: "Test".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            output_dir: PathBuf::from("docs/research"),
            max_searches: 10,
            analyze_codebase: false,
            focus: Some("narrow".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: ResearchConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.topic, "Test");
        assert_eq!(parsed.max_searches, 10);
        assert!(!parsed.analyze_codebase);
        assert_eq!(parsed.focus, Some("narrow".to_string()));
    }

    #[test]
    fn test_report_serde_roundtrip() {
        let report = ResearchReport {
            meta: ReportMeta {
                date: "2024-01-01".to_string(),
                slug: "test".to_string(),
                version: 1,
            },
            topic: "Test".to_string(),
            focus: None,
            summary: "Summary.".to_string(),
            background: "BG.".to_string(),
            codebase_analysis: None,
            search_results: vec![SearchResult {
                query: "q".to_string(),
                title: "t".to_string(),
                url: "https://example.com".to_string(),
                snippet: "s".to_string(),
                source: "ddg".to_string(),
            }],
            approaches: vec![ApproachComparison {
                name: "A".to_string(),
                description: "desc".to_string(),
                pros: vec!["good".to_string()],
                cons: vec!["bad".to_string()],
                complexity: 2,
                suitability: 4,
            }],
            recommendation: "Do it.".to_string(),
            next_steps: vec!["Step 1".to_string()],
            references: vec![Reference {
                title: "Ref".to_string(),
                url: "https://example.com".to_string(),
                ref_type: ReferenceType::Web,
            }],
        };

        let json = serde_json::to_string_pretty(&report).unwrap();
        let parsed: ResearchReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.topic, report.topic);
        assert_eq!(parsed.search_results.len(), 1);
        assert_eq!(parsed.approaches.len(), 1);
        assert_eq!(parsed.references.len(), 1);
        assert_eq!(parsed.next_steps.len(), 1);
    }

    #[test]
    fn test_extract_cargo_deps() {
        let content = r#"
[package]
name = "test"

[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = "1"
serde_json = "1"

[dev-dependencies]
tempfile = "3"
"#;
        let mut deps = Vec::new();
        DeepResearchSkill::extract_cargo_deps(content, &["serde"], &mut deps);
        assert!(deps.iter().any(|d| d.contains("serde")));
    }

    #[test]
    fn test_extract_npm_deps() {
        let content = r#"{"dependencies": {"express": "^4.18.0", "lodash": "^4.17.21"}}"#;
        let mut deps = Vec::new();
        DeepResearchSkill::extract_npm_deps(content, &["express"], &mut deps);
        assert!(deps.iter().any(|d| d.contains("express")));
    }
}
