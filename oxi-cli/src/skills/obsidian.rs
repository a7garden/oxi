//! Obsidian vault management skill for oxi
//!
//! Provides tools for working with Obsidian vaults programmatically:
//! - Search and read notes (by title, tag, path, or full-text)
//! - Analyze backlinks and forward links between notes
//! - Extract and query tags across the vault
//! - Git version control integration for vault backups and history
//!
//! This module does NOT depend on the Obsidian API or any external service.
//! It operates directly on the filesystem, reading Markdown files and
//! parsing wikilinks (`[[note-name]]`) and tags (`#tag`) from content.
//!
//! The module provides both:
//! - An [`ObsidianVault`] struct for programmatic vault access
//! - A [`skill_instructions`] function that produces system-prompt content
//!   for the LLM-driven Obsidian workflow

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for connecting to an Obsidian vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Path to the Obsidian vault root directory.
    pub vault_path: PathBuf,

    /// File extensions to treat as notes (default: `["md"]`).
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,

    /// Whether to include hidden directories (starting with `.`).
    #[serde(default)]
    pub include_hidden: bool,

    /// Directories to skip when scanning the vault.
    #[serde(default = "default_skip_dirs")]
    pub skip_dirs: Vec<String>,

    /// Maximum depth for directory traversal (default: 10).
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    /// Maximum number of notes to return in a single query (default: 200).
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_extensions() -> Vec<String> {
    vec!["md".to_string()]
}

fn default_skip_dirs() -> Vec<String> {
    vec![
        ".obsidian".to_string(),
        ".trash".to_string(),
        ".git".to_string(),
        "node_modules".to_string(),
    ]
}

fn default_max_depth() -> usize {
    10
}

fn default_max_results() -> usize {
    200
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            vault_path: PathBuf::from("."),
            extensions: default_extensions(),
            include_hidden: false,
            skip_dirs: default_skip_dirs(),
            max_depth: default_max_depth(),
            max_results: default_max_results(),
        }
    }
}

// ── Note types ───────────────────────────────────────────────────────

/// A single Obsidian note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    /// Relative path from the vault root.
    pub path: String,
    /// Note title (filename without extension).
    pub title: String,
    /// Full text content.
    pub content: String,
    /// Tags extracted from the note body (`#tag`).
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Forward links — notes this note links TO (`[[target]]`).
    #[serde(default)]
    pub forward_links: BTreeSet<String>,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last modified timestamp (seconds since epoch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}

impl Note {
    /// First non-empty, non-frontmatter line as a preview.
    pub fn preview(&self, max_chars: usize) -> &str {
        let content = if let Some(yaml_end) = self.content.find("\n---\n") {
            // Skip YAML frontmatter
            &self.content[yaml_end + 5..]
        } else {
            &self.content
        };

        let text = content.trim_start();
        if text.len() <= max_chars {
            text
        } else {
            // Try to cut at a word boundary
            match text[..max_chars].rfind(' ') {
                Some(pos) if pos > max_chars / 2 => &text[..pos],
                _ => &text[..max_chars],
            }
        }
    }
}

// ── Search types ─────────────────────────────────────────────────────

/// How to match search queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Case-insensitive substring match.
    Fuzzy,
    /// Exact match (case-insensitive).
    Exact,
    /// Regular expression match.
    Regex,
}

impl Default for SearchMode {
    fn default() -> Self {
        SearchMode::Fuzzy
    }
}

impl fmt::Display for SearchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchMode::Fuzzy => write!(f, "fuzzy"),
            SearchMode::Exact => write!(f, "exact"),
            SearchMode::Regex => write!(f, "regex"),
        }
    }
}

/// What part of a note to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    /// Search note titles only.
    Title,
    /// Search note content only.
    Content,
    /// Search both title and content.
    All,
}

impl Default for SearchScope {
    fn default() -> Self {
        SearchScope::All
    }
}

impl fmt::Display for SearchScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchScope::Title => write!(f, "title"),
            SearchScope::Content => write!(f, "content"),
            SearchScope::All => write!(f, "all"),
        }
    }
}

/// Result of a vault search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Notes matching the query.
    pub notes: Vec<NoteMatch>,
    /// Total number of matches (may exceed notes.len() if capped).
    pub total_matches: usize,
    /// Whether results were truncated by `max_results`.
    pub truncated: bool,
}

/// A single note match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteMatch {
    /// Relative path of the matching note.
    pub path: String,
    /// Note title.
    pub title: String,
    /// Which field matched.
    pub matched_field: MatchField,
    /// Matching text snippet (up to 200 chars around the match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Which field matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchField {
    Title,
    Content,
    Tag,
    Path,
}

// ── Link analysis types ──────────────────────────────────────────────

/// Result of backlink analysis for a single note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklinkInfo {
    /// The target note being analyzed.
    pub note_title: String,
    /// Notes that link TO this note.
    pub backlinks: Vec<LinkRef>,
    /// Notes this note links TO.
    pub forward_links: Vec<LinkRef>,
    /// Orphan status — this note has no backlinks.
    pub is_orphan: bool,
    /// Number of backlinks.
    pub backlink_count: usize,
    /// Number of forward links.
    pub forward_link_count: usize,
}

/// A reference from one note to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkRef {
    /// Title of the source note.
    pub source_title: String,
    /// Relative path of the source note.
    pub source_path: String,
    /// Display text of the link (if different from target).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
    /// Line number where the link appears (1-based, if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_number: Option<usize>,
}

/// Full vault graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultGraph {
    /// Number of notes in the vault.
    pub note_count: usize,
    /// Number of unique links between notes.
    pub link_count: usize,
    /// Number of orphan notes (no backlinks, no forward links).
    pub orphan_count: usize,
    /// Notes sorted by backlink count (most linked first).
    pub most_linked: Vec<(String, usize)>,
    /// Notes sorted by forward link count (most outgoing links).
    pub most_linking: Vec<(String, usize)>,
    /// Tag co-occurrence: tags that appear together in notes.
    #[serde(default)]
    pub tag_clusters: BTreeMap<String, BTreeSet<String>>,
}

// ── Tag analysis types ───────────────────────────────────────────────

/// Result of tag analysis across the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagAnalysis {
    /// All unique tags found in the vault.
    pub tags: BTreeMap<String, TagInfo>,
    /// Total number of unique tags.
    pub tag_count: usize,
    /// Tags sorted by frequency (most used first).
    pub top_tags: Vec<(String, usize)>,
    /// Tags only used once.
    pub singleton_tags: Vec<String>,
    /// Hierarchical tag breakdown (e.g., "project/alpha" under "project").
    #[serde(default)]
    pub hierarchy: BTreeMap<String, BTreeSet<String>>,
}

/// Information about a single tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagInfo {
    /// The tag string (without `#`).
    pub tag: String,
    /// Number of notes using this tag.
    pub count: usize,
    /// Notes that have this tag.
    pub notes: Vec<String>,
}

// ── Git integration types ────────────────────────────────────────────

/// Result of a git status check on the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    /// Whether the vault is a git repository.
    pub is_repo: bool,
    /// Current branch name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Number of uncommitted changes.
    #[serde(default)]
    pub uncommitted_changes: usize,
    /// Staged files.
    #[serde(default)]
    pub staged: Vec<String>,
    /// Modified (unstaged) files.
    #[serde(default)]
    pub modified: Vec<String>,
    /// Untracked files.
    #[serde(default)]
    pub untracked: Vec<String>,
}

/// Result of a git log query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogEntry {
    /// Commit hash (short).
    pub hash: String,
    /// Commit message.
    pub message: String,
    /// Author.
    pub author: String,
    /// Date string.
    pub date: String,
    /// Files changed in this commit.
    #[serde(default)]
    pub files_changed: Vec<String>,
}

/// Result of a git commit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitResult {
    /// Whether the commit succeeded.
    pub success: bool,
    /// Commit hash (short).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Number of files committed.
    pub files_committed: usize,
    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Main vault struct ────────────────────────────────────────────────

/// Provides access to an Obsidian vault on the filesystem.
///
/// # Example
///
/// ```rust,ignore
/// use oxi::skills::obsidian::{ObsidianVault, VaultConfig};
/// use std::path::PathBuf;
///
/// let config = VaultConfig {
///     vault_path: PathBuf::from("/path/to/vault"),
///     ..Default::default()
/// };
/// let vault = ObsidianVault::new(config);
///
/// // Search notes
/// let results = vault.search("meeting notes").unwrap();
///
/// // Analyze backlinks
/// let backlinks = vault.analyze_backlinks("Project Alpha").unwrap();
/// ```
pub struct ObsidianVault {
    config: VaultConfig,
    /// Lazily loaded note index: title → relative path.
    index: once_cell::sync::OnceCell<HashMap<String, String>>,
}

impl ObsidianVault {
    /// Create a new vault accessor with the given configuration.
    pub fn new(config: VaultConfig) -> Self {
        Self {
            config,
            index: once_cell::sync::OnceCell::new(),
        }
    }

    /// Access the vault configuration.
    pub fn config(&self) -> &VaultConfig {
        &self.config
    }

    /// Resolve the vault root as an absolute path.
    fn vault_root(&self) -> &Path {
        &self.config.vault_path
    }

    // ── Note reading ────────────────────────────────────────────────

    /// Read a single note by its relative path or title.
    ///
    /// Tries path first, then falls back to title lookup.
    pub fn read_note(&self, path_or_title: &str) -> Result<Note> {
        let vault_root = self.vault_root();

        // Try as path first
        let full_path = vault_root.join(path_or_title);
        let full_path = if full_path.exists() {
            full_path
        } else {
            // Try adding .md extension
            let with_ext = vault_root.join(format!("{}.md", path_or_title));
            if with_ext.exists() {
                with_ext
            } else {
                // Try title lookup
                let idx = self.get_or_build_index()?;
                match idx.get(&path_or_title.to_lowercase()) {
                    Some(rel_path) => vault_root.join(rel_path),
                    None => bail!("Note not found: {}", path_or_title),
                }
            }
        };

        self.parse_note(&full_path, vault_root)
    }

    /// Read multiple notes by their relative paths.
    pub fn read_notes(&self, paths: &[&str]) -> Result<Vec<Note>> {
        let mut notes = Vec::with_capacity(paths.len());
        for path in paths {
            notes.push(self.read_note(path)?);
        }
        Ok(notes)
    }

    /// List all notes in the vault.
    pub fn list_notes(&self) -> Result<Vec<Note>> {
        let vault_root = self.vault_root();
        let mut notes = Vec::new();
        self.walk_vault(vault_root, vault_root, 0, &mut notes)?;
        Ok(notes)
    }

    // ── Search ──────────────────────────────────────────────────────

    /// Search notes by query string.
    pub fn search(&self, query: &str) -> Result<SearchResult> {
        self.search_with_options(query, SearchMode::Fuzzy, SearchScope::All)
    }

    /// Search notes with explicit mode and scope.
    pub fn search_with_options(
        &self,
        query: &str,
        mode: SearchMode,
        scope: SearchScope,
    ) -> Result<SearchResult> {
        let notes = self.list_notes()?;
        let query_lower = query.to_lowercase();
        let max = self.config.max_results;

        let regex = if mode == SearchMode::Regex {
            Some(regex::Regex::new(&format!("(?i){}", query)).context("Invalid regex pattern")?)
        } else {
            None
        };

        let mut matches: Vec<NoteMatch> = Vec::new();
        let mut total = 0;

        for note in &notes {
            if total >= max {
                // Keep counting but don't collect more
                let has_match = self.note_matches(note, &query_lower, mode, scope, regex.as_ref());
                if has_match {
                    total += 1;
                }
                continue;
            }

            // Check title
            if scope == SearchScope::Title || scope == SearchScope::All {
                if self.text_matches(&note.title, &query_lower, mode, regex.as_ref()) {
                    total += 1;
                    matches.push(NoteMatch {
                        path: note.path.clone(),
                        title: note.title.clone(),
                        matched_field: MatchField::Title,
                        snippet: None,
                    });
                    continue;
                }
            }

            // Check path
            if scope == SearchScope::All {
                if self.text_matches(&note.path, &query_lower, mode, regex.as_ref()) {
                    total += 1;
                    matches.push(NoteMatch {
                        path: note.path.clone(),
                        title: note.title.clone(),
                        matched_field: MatchField::Path,
                        snippet: None,
                    });
                    continue;
                }
            }

            // Check tags
            if scope == SearchScope::All {
                for tag in &note.tags {
                    if self.text_matches(tag, &query_lower, mode, regex.as_ref()) {
                        total += 1;
                        matches.push(NoteMatch {
                            path: note.path.clone(),
                            title: note.title.clone(),
                            matched_field: MatchField::Tag,
                            snippet: Some(format!("#{}", tag)),
                        });
                        break;
                    }
                }
                if matches.len() > 0 && matches.last().unwrap().matched_field == MatchField::Tag {
                    continue;
                }
            }

            // Check content
            if scope == SearchScope::Content || scope == SearchScope::All {
                if self.text_matches(&note.content, &query_lower, mode, regex.as_ref()) {
                    total += 1;
                    let snippet = self.extract_snippet(&note.content, &query_lower, 200);
                    matches.push(NoteMatch {
                        path: note.path.clone(),
                        title: note.title.clone(),
                        matched_field: MatchField::Content,
                        snippet,
                    });
                }
            }
        }

        let truncated = matches.len() < total;

        Ok(SearchResult {
            notes: matches,
            total_matches: total,
            truncated,
        })
    }

    /// Search notes by tag.
    pub fn search_by_tag(&self, tag: &str) -> Result<Vec<Note>> {
        let tag_lower = tag.trim_start_matches('#').to_lowercase();
        let notes = self.list_notes()?;
        Ok(notes
            .into_iter()
            .filter(|n| n.tags.iter().any(|t| t.to_lowercase() == tag_lower))
            .collect())
    }

    // ── Backlink analysis ───────────────────────────────────────────

    /// Analyze backlinks for a specific note.
    pub fn analyze_backlinks(&self, note_title: &str) -> Result<BacklinkInfo> {
        let notes = self.list_notes()?;
        let title_lower = note_title.to_lowercase();

        // Find the target note
        let target = notes
            .iter()
            .find(|n| n.title.to_lowercase() == title_lower)
            .context(format!("Note '{}' not found", note_title))?;

        // Collect forward links (from target to others)
        let forward_links: Vec<LinkRef> = target
            .forward_links
            .iter()
            .map(|_link| LinkRef {
                source_title: target.title.clone(),
                source_path: target.path.clone(),
                display_text: None,
                line_number: None,
            })
            .collect();

        // Collect backlinks (from other notes to target)
        let mut backlinks = Vec::new();
        for note in &notes {
            if note.path == target.path {
                continue;
            }
            // Check if this note links to the target
            let link_refs = self.find_links_to(note, &target.title);
            backlinks.extend(link_refs);
        }

        let backlink_count = backlinks.len();
        let forward_link_count = forward_links.len();
        let is_orphan = backlink_count == 0 && forward_link_count == 0;

        Ok(BacklinkInfo {
            note_title: target.title.clone(),
            backlinks,
            forward_links,
            is_orphan,
            backlink_count,
            forward_link_count,
        })
    }

    /// Analyze backlinks for all notes in the vault (full graph).
    pub fn analyze_vault_graph(&self) -> Result<VaultGraph> {
        let notes = self.list_notes()?;
        let note_count = notes.len();

        // Build title → backlink count map
        let mut backlink_counts: HashMap<String, usize> = HashMap::new();
        let mut forward_link_counts: HashMap<String, usize> = HashMap::new();
        let mut link_total: usize = 0;
        let mut linked_notes: HashSet<String> = HashSet::new();

        for note in &notes {
            let fc = note.forward_links.len();
            forward_link_counts.insert(note.title.clone(), fc);
            link_total += fc;

            if fc > 0 {
                linked_notes.insert(note.title.clone());
            }

            for target in &note.forward_links {
                *backlink_counts.entry(target.clone()).or_insert(0) += 1;
                linked_notes.insert(target.clone());
            }
        }

        // Orphans: notes with no backlinks AND no forward links
        let orphan_count = notes
            .iter()
            .filter(|n| {
                *backlink_counts.get(&n.title).unwrap_or(&0) == 0
                    && *forward_link_counts.get(&n.title).unwrap_or(&0) == 0
            })
            .count();

        // Most linked (by backlink count)
        let mut most_linked: Vec<(String, usize)> = backlink_counts.into_iter().collect();
        most_linked.sort_by(|a, b| b.1.cmp(&a.1));
        most_linked.truncate(20);

        // Most linking (by forward link count)
        let mut most_linking: Vec<(String, usize)> = forward_link_counts.into_iter().collect();
        most_linking.sort_by(|a, b| b.1.cmp(&a.1));
        most_linking.truncate(20);

        // Tag clusters: co-occurring tags
        let mut tag_clusters: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for note in &notes {
            let tags: Vec<&String> = note.tags.iter().collect();
            for i in 0..tags.len() {
                for j in (i + 1)..tags.len() {
                    tag_clusters
                        .entry(tags[i].clone())
                        .or_default()
                        .insert(tags[j].clone());
                    tag_clusters
                        .entry(tags[j].clone())
                        .or_default()
                        .insert(tags[i].clone());
                }
            }
        }

        Ok(VaultGraph {
            note_count,
            link_count: link_total,
            orphan_count,
            most_linked,
            most_linking,
            tag_clusters,
        })
    }

    // ── Tag analysis ────────────────────────────────────────────────

    /// Analyze all tags in the vault.
    pub fn analyze_tags(&self) -> Result<TagAnalysis> {
        let notes = self.list_notes()?;
        let mut tag_map: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for note in &notes {
            for tag in &note.tags {
                tag_map
                    .entry(tag.clone())
                    .or_default()
                    .push(note.title.clone());
            }
        }

        let tag_count = tag_map.len();

        // Build TagInfo map
        let tags: BTreeMap<String, TagInfo> = tag_map
            .iter()
            .map(|(tag, note_titles)| {
                (
                    tag.clone(),
                    TagInfo {
                        tag: tag.clone(),
                        count: note_titles.len(),
                        notes: note_titles.clone(),
                    },
                )
            })
            .collect();

        // Top tags sorted by frequency
        let mut top_tags: Vec<(String, usize)> = tag_map
            .iter()
            .map(|(tag, notes)| (tag.clone(), notes.len()))
            .collect();
        top_tags.sort_by(|a, b| b.1.cmp(&a.1));

        // Singleton tags
        let singleton_tags: Vec<String> = tag_map
            .iter()
            .filter(|(_, notes)| notes.len() == 1)
            .map(|(tag, _)| tag.clone())
            .collect();

        // Hierarchical tags (e.g., "project/alpha" → "project" parent)
        let mut hierarchy: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for tag in tag_map.keys() {
            if let Some(slash_pos) = tag.find('/') {
                let parent = &tag[..slash_pos];
                hierarchy
                    .entry(parent.to_string())
                    .or_default()
                    .insert(tag.clone());
            }
        }

        Ok(TagAnalysis {
            tags,
            tag_count,
            top_tags,
            singleton_tags,
            hierarchy,
        })
    }

    // ── Git integration ─────────────────────────────────────────────

    /// Check the git status of the vault.
    pub async fn git_status(&self) -> Result<GitStatus> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to run git status. Is git installed?")?;

        if !output.status.success() {
            return Ok(GitStatus {
                is_repo: false,
                branch: None,
                uncommitted_changes: 0,
                staged: vec![],
                modified: vec![],
                untracked: vec![],
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut staged = Vec::new();
        let mut modified = Vec::new();
        let mut untracked = Vec::new();

        for line in stdout.lines() {
            if line.len() < 4 {
                continue;
            }
            let status = &line[..2];
            let file = line[3..].to_string();

            match status {
                "?? " => untracked.push(file),
                "A " | "M " | "R " => staged.push(file),
                _ if status.starts_with(' ') => modified.push(file),
                _ => {
                    // Both staged and unstaged changes
                    modified.push(file);
                }
            }
        }

        let uncommitted = staged.len() + modified.len() + untracked.len();

        // Get branch name
        let branch_output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to get git branch")?;

        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();
        let branch = if branch.is_empty() || branch == "HEAD" {
            None
        } else {
            Some(branch)
        };

        Ok(GitStatus {
            is_repo: true,
            branch,
            uncommitted_changes: uncommitted,
            staged,
            modified,
            untracked,
        })
    }

    /// Get recent git log entries for the vault.
    pub async fn git_log(&self, max_entries: usize) -> Result<Vec<GitLogEntry>> {
        let output = Command::new("git")
            .args([
                "log",
                &format!("-{}", max_entries),
                "--pretty=format:%h|%s|%an|%ai",
                "--name-only",
            ])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to run git log")?;

        if !output.status.success() {
            bail!(
                "git log failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();
        let mut current: Option<GitLogEntry> = None;

        for line in stdout.lines() {
            if line.is_empty() {
                if let Some(entry) = current.take() {
                    entries.push(entry);
                }
                continue;
            }

            if let Some(_pipe_pos) = line.find('|') {
                // New commit line
                if let Some(entry) = current.take() {
                    entries.push(entry);
                }

                let parts: Vec<&str> = line.splitn(4, '|').collect();
                if parts.len() >= 4 {
                    current = Some(GitLogEntry {
                        hash: parts[0].to_string(),
                        message: parts[1].to_string(),
                        author: parts[2].to_string(),
                        date: parts[3].to_string(),
                        files_changed: Vec::new(),
                    });
                }
            } else if let Some(ref mut entry) = current {
                // File name line
                entry.files_changed.push(line.to_string());
            }
        }

        if let Some(entry) = current.take() {
            entries.push(entry);
        }

        Ok(entries)
    }

    /// Stage all changes and create a commit.
    pub async fn git_commit_all(&self, message: &str) -> Result<GitCommitResult> {
        // Stage everything
        let add_output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to run git add")?;

        if !add_output.status.success() {
            return Ok(GitCommitResult {
                success: false,
                hash: None,
                files_committed: 0,
                error: Some(String::from_utf8_lossy(&add_output.stderr).to_string()),
            });
        }

        // Check if there are changes to commit
        let diff_output = Command::new("git")
            .args(["diff", "--cached", "--stat"])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to check staged changes")?;

        let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);
        if diff_stdout.trim().is_empty() {
            return Ok(GitCommitResult {
                success: true,
                hash: None,
                files_committed: 0,
                error: Some("No changes to commit".to_string()),
            });
        }

        // Count files
        let file_count = diff_stdout.lines().count().saturating_sub(1); // Last line is summary

        // Commit
        let commit_output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to run git commit")?;

        if !commit_output.status.success() {
            return Ok(GitCommitResult {
                success: false,
                hash: None,
                files_committed: 0,
                error: Some(String::from_utf8_lossy(&commit_output.stderr).to_string()),
            });
        }

        // Get the commit hash
        let hash_output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to get commit hash")?;

        let hash = String::from_utf8_lossy(&hash_output.stdout)
            .trim()
            .to_string();

        Ok(GitCommitResult {
            success: true,
            hash: Some(hash),
            files_committed: file_count,
            error: None,
        })
    }

    /// Initialize a git repository in the vault if one doesn't exist.
    pub async fn git_init(&self) -> Result<bool> {
        let git_dir = self.vault_root().join(".git");
        if git_dir.exists() {
            return Ok(false);
        }

        let output = Command::new("git")
            .args(["init"])
            .current_dir(self.vault_root())
            .output()
            .await
            .context("Failed to run git init")?;

        if !output.status.success() {
            bail!(
                "git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        tracing::info!(
            "Initialized git repository in {}",
            self.vault_root().display()
        );
        Ok(true)
    }

    /// Create a .gitignore for the vault (ignoring Obsidian metadata).
    pub async fn create_gitignore(&self) -> Result<PathBuf> {
        let gitignore_path = self.vault_root().join(".gitignore");

        let content = r#".obsidian/
.trash/
.DS_Store
*.swp
*.swo
*~
"#;

        fs::write(&gitignore_path, content).context("Failed to write .gitignore")?;
        Ok(gitignore_path)
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Build the note index (title → relative path).
    fn build_index(&self) -> Result<HashMap<String, String>> {
        let notes = self.list_notes()?;
        let mut index = HashMap::with_capacity(notes.len());

        for note in notes {
            // Lowercase title for case-insensitive lookup
            index.insert(note.title.to_lowercase(), note.path.clone());
        }

        Ok(index)
    }

    /// Get or build the note index.
    fn get_or_build_index(&self) -> Result<&HashMap<String, String>> {
        self.index.get_or_try_init(|| self.build_index())
    }

    /// Recursively walk the vault directory and collect notes.
    fn walk_vault(
        &self,
        dir: &Path,
        vault_root: &Path,
        depth: usize,
        notes: &mut Vec<Note>,
    ) -> Result<()> {
        if depth > self.config.max_depth {
            return Ok(());
        }

        let entries = fs::read_dir(dir)
            .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();

            // Skip hidden files/dirs
            if !self.config.include_hidden && name.starts_with('.') {
                continue;
            }

            // Skip configured directories
            if self.config.skip_dirs.iter().any(|d| *d == name) {
                continue;
            }

            if path.is_dir() {
                self.walk_vault(&path, vault_root, depth + 1, notes)?;
            } else {
                // Check extension
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                if self.config.extensions.contains(&ext) {
                    match self.parse_note(&path, vault_root) {
                        Ok(note) => notes.push(note),
                        Err(e) => {
                            tracing::debug!("Failed to parse {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse a single note file into a [`Note`].
    fn parse_note(&self, path: &Path, vault_root: &Path) -> Result<Note> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let relative = path
            .strip_prefix(vault_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        let tags = Self::extract_tags(&content);
        let forward_links = Self::extract_wikilinks(&content);

        let metadata = fs::metadata(path).ok();
        let size_bytes = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        Ok(Note {
            path: relative,
            title,
            content,
            tags,
            forward_links,
            size_bytes,
            modified,
        })
    }

    /// Extract `#tag` occurrences from note content.
    ///
    /// Handles:
    /// - `#tag`
    /// - `#nested/tag`
    /// - Ignores headings (`# Heading`) by requiring no space before text
    /// - Ignores hex colors (`#fff`, `#aabbcc`)
    fn extract_tags(content: &str) -> BTreeSet<String> {
        let mut tags = BTreeSet::new();

        for line in content.lines() {
            // Skip code blocks
            if line.trim().starts_with("```") {
                continue;
            }

            // Scan for '#' characters that represent tags
            let bytes = line.as_bytes();
            let mut pos = 0;

            while pos < bytes.len() {
                if bytes[pos] != b'#' {
                    pos += 1;
                    continue;
                }

                let hash_pos = pos;
                pos += 1;

                // Check if this is a heading: '#' at start of line (or after whitespace)
                // followed by a space or another '#'
                if pos >= bytes.len() {
                    continue;
                }

                let next_ch = bytes[pos];

                // Heading detection: if all chars before '#' are whitespace AND
                // next char is '#' or whitespace, it's a heading marker
                let prefix = &line[..hash_pos];
                if prefix.trim().is_empty()
                    && (next_ch == b'#' || next_ch == b' ' || next_ch == b'\t')
                {
                    continue;
                }

                // Also skip if followed by whitespace (not a tag)
                if next_ch == b' ' || next_ch == b'\t' || next_ch == b'\n' {
                    continue;
                }

                // Extract the tag text after '#'
                let tag: String = line[pos..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '/')
                    .collect();

                // Tags need at least 2 chars and shouldn't be hex colors
                // Hex colors are exactly 3 or 6 hex chars (e.g., #fff, #aabbcc)
                let is_hex_color = (tag.len() == 3 || tag.len() == 6)
                    && tag.chars().all(|c| c.is_ascii_hexdigit());

                if tag.len() >= 2 && !is_hex_color {
                    tags.insert(tag.to_lowercase());
                }
            }
        }

        tags
    }

    /// Extract `[[wikilink]]` references from note content.
    ///
    /// Handles:
    /// - `[[Note Name]]`
    /// - `[[Note Name|Display Text]]`
    /// - `[[note-name]]`
    fn extract_wikilinks(content: &str) -> BTreeSet<String> {
        let mut links = BTreeSet::new();
        let mut remaining = content;

        while let Some(start) = remaining.find("[[") {
            remaining = &remaining[start + 2..];
            if let Some(end) = remaining.find("]]") {
                let link_text = &remaining[..end];

                // Handle alias syntax: [[target|display]]
                let target = if let Some(pipe_pos) = link_text.find('|') {
                    &link_text[..pipe_pos]
                } else {
                    link_text
                };

                let target = target.trim();
                if !target.is_empty() {
                    links.insert(target.to_string());
                }

                remaining = &remaining[end + 2..];
            } else {
                break;
            }
        }

        links
    }

    /// Find all links from one note to a target.
    fn find_links_to(&self, note: &Note, target_title: &str) -> Vec<LinkRef> {
        let target_lower = target_title.to_lowercase();
        let mut refs = Vec::new();

        for (line_num, line) in note.content.lines().enumerate() {
            let mut remaining = line;
            while let Some(start) = remaining.find("[[") {
                remaining = &remaining[start + 2..];
                if let Some(end) = remaining.find("]]") {
                    let link_text = &remaining[..end];
                    let target = if let Some(pipe_pos) = link_text.find('|') {
                        &link_text[..pipe_pos]
                    } else {
                        link_text
                    };

                    if target.trim().to_lowercase() == target_lower {
                        let display = if link_text.contains('|') {
                            let pipe_pos = link_text.find('|').unwrap();
                            Some(link_text[pipe_pos + 1..].trim().to_string())
                        } else {
                            None
                        };

                        refs.push(LinkRef {
                            source_title: note.title.clone(),
                            source_path: note.path.clone(),
                            display_text: display,
                            line_number: Some(line_num + 1),
                        });
                    }

                    remaining = &remaining[end + 2..];
                } else {
                    break;
                }
            }
        }

        refs
    }

    /// Check if text matches the query using the given mode.
    fn text_matches(
        &self,
        text: &str,
        query_lower: &str,
        mode: SearchMode,
        regex: Option<&regex::Regex>,
    ) -> bool {
        match mode {
            SearchMode::Fuzzy => text.to_lowercase().contains(query_lower),
            SearchMode::Exact => text.to_lowercase() == *query_lower,
            SearchMode::Regex => regex
                .map(|r: &regex::Regex| r.is_match(text))
                .unwrap_or(false),
        }
    }

    /// Check if a note matches the query.
    fn note_matches(
        &self,
        note: &Note,
        query_lower: &str,
        mode: SearchMode,
        scope: SearchScope,
        regex: Option<&regex::Regex>,
    ) -> bool {
        match scope {
            SearchScope::Title => self.text_matches(&note.title, query_lower, mode, regex),
            SearchScope::Content => self.text_matches(&note.content, query_lower, mode, regex),
            SearchScope::All => {
                self.text_matches(&note.title, query_lower, mode, regex)
                    || self.text_matches(&note.path, query_lower, mode, regex)
                    || self.text_matches(&note.content, query_lower, mode, regex)
                    || note
                        .tags
                        .iter()
                        .any(|t| self.text_matches(t, query_lower, mode, regex))
            }
        }
    }

    /// Extract a text snippet around the first match in content.
    fn extract_snippet(&self, content: &str, query: &str, max_chars: usize) -> Option<String> {
        let content_lower = content.to_lowercase();
        let pos = content_lower.find(query)?;

        let start = if pos > max_chars / 2 {
            // Find a word boundary
            let candidate = pos - max_chars / 2;
            content[..candidate]
                .rfind(' ')
                .map(|p| p + 1)
                .unwrap_or(candidate)
        } else {
            0
        };

        let end = (pos + query.len() + max_chars / 2).min(content.len());

        let mut snippet = String::new();
        if start > 0 {
            snippet.push_str("...");
        }
        snippet.push_str(&content[start..end]);
        if end < content.len() {
            snippet.push_str("...");
        }

        Some(snippet)
    }
}

impl fmt::Debug for ObsidianVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObsidianVault")
            .field("vault_path", &self.config.vault_path)
            .field("extensions", &self.config.extensions)
            .finish()
    }
}

// ── Skill instructions ───────────────────────────────────────────────

/// Generate the skill instructions to be injected into the system prompt
/// when the Obsidian skill is active.
///
/// This tells the LLM how to interact with Obsidian vaults using the
/// tools available to it (bash, read, write).
pub fn skill_instructions() -> String {
    let prompt = r#"# Obsidian Vault Skill

You are running the **obsidian** skill. Your job is to help the user
manage, search, and analyze an Obsidian vault.

## Capabilities

### 1. Note Search and Reading
- Search notes by title, content, tag, or path (fuzzy, exact, or regex)
- Read individual notes or batches of notes
- List all notes in the vault
- Extract snippets around search matches

### 2. Backlink and Link Analysis
- Analyze backlinks (which notes link TO a target note)
- Analyze forward links (which notes a source links TO)
- Detect orphan notes (no backlinks, no forward links)
- Generate a full vault graph with most-linked and most-linking notes
- Identify tag co-occurrence clusters

### 3. Tag Analysis
- Extract all tags from the vault
- Show tag frequency and distribution
- Find singleton tags (used only once)
- Analyze hierarchical tags (e.g., `project/alpha` under `project`)
- Search notes by tag

### 4. Git Version Control
- Check git status of the vault
- View git history for the vault
- Commit all changes with a message
- Initialize a git repository
- Create a .gitignore for Obsidian metadata

## Workflow

### For Searching Notes
1. Determine the vault path (ask the user or infer from context)
2. Use the search API or grep/ripgrep to find matching notes
3. Read the matching notes
4. Present results with titles, paths, and relevant snippets

### For Backlink Analysis
1. Identify the target note
2. Use grep for `[[target]]` patterns across all notes
3. Present the backlinks with source note, line number, and context
4. Highlight orphan notes that might need linking

### For Tag Analysis
1. Scan all markdown files for `#tag` patterns
2. Aggregate and count tag usage
3. Present tag frequency, hierarchy, and co-occurrence
4. Suggest tag cleanup if there are many singletons

### For Git Operations
1. Check if the vault is a git repo
2. If not, offer to initialize one
3. Show status, diff, or log as requested
4. Commit changes with descriptive messages

## Guidelines

- **Respect vault structure** — don't modify `.obsidian/` configuration
- **Preserve wikilinks** — when editing notes, maintain `[[link]]` syntax
- **Tag consistency** — prefer lowercase tags, suggest normalizing mixed case
- **Git safety** — always show status before committing, never force push
- **Large vaults** — for vaults with 1000+ notes, use streaming/limited results

## Common Commands

### Search with ripgrep
```bash
rg -i "query" --type md /path/to/vault
```

### Find backlinks to a note
```bash
rg '\[\[Note Title\]\]' --type md /path/to/vault
```

### Find all tags
```bash
rg -o '#[a-zA-Z][a-zA-Z0-9_/-]+' --type md /path/to/vault | sort | uniq -c | sort -rn
```

### Git status
```bash
cd /path/to/vault && git status --short
```

### Commit all changes
```bash
cd /path/to/vault && git add -A && git commit -m "vault: update notes"
```
"#;
    prompt.to_string()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary vault with a known structure for testing.
    fn setup_test_vault() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Create some notes
        fs::write(
            root.join("index.md"),
            "# Welcome\n\nThis is the vault index.\n\n[[Project Alpha]] [[meeting-notes]]\n\n#status/active",
        )
        .unwrap();

        fs::write(
            root.join("Project Alpha.md"),
            "# Project Alpha\n\nA major project.\n\nLinks: [[index]] [[Team]]\n\n#project #status/active",
        )
        .unwrap();

        fs::write(
            root.join("meeting-notes.md"),
            "# Meeting Notes\n\nNotes from meetings.\n\n[[Project Alpha]] was discussed.\n\n#meeting #project",
        )
        .unwrap();

        fs::create_dir_all(root.join("archive")).unwrap();
        fs::write(
            root.join("archive").join("old-note.md"),
            "# Old Note\n\nAn archived note.\n\n#archive #status/inactive",
        )
        .unwrap();

        // Create a file that should be skipped
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(root.join(".obsidian").join("app.json"), "{}").unwrap();

        tmp
    }

    fn make_vault(path: &Path) -> ObsidianVault {
        ObsidianVault::new(VaultConfig {
            vault_path: path.to_path_buf(),
            ..Default::default()
        })
    }

    // ── Configuration tests ─────────────────────────────────────────

    #[test]
    fn test_vault_config_default() {
        let config = VaultConfig::default();
        assert_eq!(config.extensions, vec!["md"]);
        assert!(!config.include_hidden);
        assert!(config.skip_dirs.contains(&".obsidian".to_string()));
        assert_eq!(config.max_depth, 10);
        assert_eq!(config.max_results, 200);
    }

    #[test]
    fn test_vault_config_serde_roundtrip() {
        let config = VaultConfig {
            vault_path: PathBuf::from("/my/vault"),
            extensions: vec!["md".to_string(), "txt".to_string()],
            include_hidden: true,
            skip_dirs: vec![".git".to_string()],
            max_depth: 5,
            max_results: 100,
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: VaultConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.vault_path, PathBuf::from("/my/vault"));
        assert_eq!(parsed.extensions.len(), 2);
        assert!(parsed.include_hidden);
        assert_eq!(parsed.max_depth, 5);
    }

    // ── Note reading tests ──────────────────────────────────────────

    #[test]
    fn test_list_notes() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());
        let notes = vault.list_notes().unwrap();

        assert_eq!(notes.len(), 4);
        let titles: Vec<&str> = notes.iter().map(|n| n.title.as_str()).collect();
        assert!(titles.contains(&"index"));
        assert!(titles.contains(&"Project Alpha"));
        assert!(titles.contains(&"meeting-notes"));
        assert!(titles.contains(&"old-note"));
    }

    #[test]
    fn test_list_notes_skips_obsidian_dir() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());
        let notes = vault.list_notes().unwrap();

        // Should not include files from .obsidian/
        for note in &notes {
            assert!(
                !note.path.contains(".obsidian"),
                "Should skip .obsidian: {}",
                note.path
            );
        }
    }

    #[test]
    fn test_read_note_by_path() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let note = vault.read_note("index.md").unwrap();
        assert_eq!(note.title, "index");
        assert!(note.content.contains("Welcome"));
    }

    #[test]
    fn test_read_note_by_title() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let note = vault.read_note("Project Alpha").unwrap();
        assert_eq!(note.title, "Project Alpha");
        assert!(note.content.contains("major project"));
    }

    #[test]
    fn test_read_note_not_found() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        assert!(vault.read_note("nonexistent").is_err());
    }

    #[test]
    fn test_read_note_subdirectory() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let note = vault.read_note("archive/old-note.md").unwrap();
        assert_eq!(note.title, "old-note");
    }

    // ── Tag extraction tests ────────────────────────────────────────

    #[test]
    fn test_extract_tags_basic() {
        let content = "Some text #project and #status/active";
        let tags = ObsidianVault::extract_tags(content);
        assert!(tags.contains("project"));
        assert!(tags.contains("status/active"));
    }

    #[test]
    fn test_extract_tags_ignores_headings() {
        let content = "# Heading One\n\n## Heading Two\n\nSome #tag here";
        let tags = ObsidianVault::extract_tags(content);
        assert!(!tags.contains("heading"));
        assert!(!tags.contains("heading-one"));
        assert!(tags.contains("tag"));
    }

    #[test]
    fn test_extract_tags_ignores_hex_colors() {
        let content = "Color #fff and #aabbcc but #real-tag";
        let tags = ObsidianVault::extract_tags(content);
        assert!(!tags.contains("fff"));
        assert!(!tags.contains("aabbcc"));
        assert!(tags.contains("real-tag"));
    }

    #[test]
    fn test_extract_tags_minimum_length() {
        let content = "#a #ab #my-tag";
        let tags = ObsidianVault::extract_tags(content);
        assert!(!tags.contains("a")); // too short
        assert!(tags.contains("ab"));
        assert!(tags.contains("my-tag"));
    }

    #[test]
    fn test_extract_tags_from_note() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());
        let note = vault.read_note("index.md").unwrap();

        assert!(note.tags.contains("status/active"));
    }

    // ── Wikilink extraction tests ───────────────────────────────────

    #[test]
    fn test_extract_wikilinks_basic() {
        let content = "See [[Target]] for details.";
        let links = ObsidianVault::extract_wikilinks(content);
        assert!(links.contains("Target"));
    }

    #[test]
    fn test_extract_wikilinks_with_alias() {
        let content = "See [[Target|display text]] for details.";
        let links = ObsidianVault::extract_wikilinks(content);
        assert!(links.contains("Target"));
        assert!(!links.contains("display text"));
    }

    #[test]
    fn test_extract_wikilinks_multiple() {
        let content = "[[Alpha]] and [[Beta]] and [[Gamma]]";
        let links = ObsidianVault::extract_wikilinks(content);
        assert_eq!(links.len(), 3);
        assert!(links.contains("Alpha"));
        assert!(links.contains("Beta"));
        assert!(links.contains("Gamma"));
    }

    #[test]
    fn test_extract_wikilinks_empty() {
        let content = "No links here.";
        let links = ObsidianVault::extract_wikilinks(content);
        assert!(links.is_empty());
    }

    #[test]
    fn test_forward_links_from_note() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());
        let note = vault.read_note("index.md").unwrap();

        assert!(note.forward_links.contains("Project Alpha"));
        assert!(note.forward_links.contains("meeting-notes"));
    }

    // ── Search tests ────────────────────────────────────────────────

    #[test]
    fn test_search_fuzzy() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let results = vault.search("project").unwrap();
        assert!(results.total_matches >= 2); // "Project Alpha" and "meeting-notes" (contains "project")
    }

    #[test]
    fn test_search_by_tag() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let notes = vault.search_by_tag("project").unwrap();
        assert!(notes.len() >= 1);
        assert!(notes.iter().any(|n| n.title == "Project Alpha"));
    }

    #[test]
    fn test_search_by_tag_with_hash() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let notes = vault.search_by_tag("#project").unwrap();
        assert!(notes.len() >= 1);
    }

    #[test]
    fn test_search_title_only() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let results = vault
            .search_with_options("alpha", SearchMode::Fuzzy, SearchScope::Title)
            .unwrap();
        assert!(results.total_matches >= 1);
        assert!(results
            .notes
            .iter()
            .any(|m| m.matched_field == MatchField::Title));
    }

    #[test]
    fn test_search_no_results() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let results = vault.search("zzzznonexistent").unwrap();
        assert_eq!(results.total_matches, 0);
    }

    #[test]
    fn test_search_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Create more notes than max_results allows
        for i in 0..5 {
            fs::write(
                root.join(format!("note{}.md", i)),
                format!("Find me matchtest {}", i),
            )
            .unwrap();
        }

        let vault = ObsidianVault::new(VaultConfig {
            vault_path: root.to_path_buf(),
            max_results: 3,
            ..Default::default()
        });

        let results = vault.search("matchtest").unwrap();
        assert_eq!(results.notes.len(), 3);
        assert!(results.truncated);
        assert_eq!(results.total_matches, 5);
    }

    // ── Backlink tests ──────────────────────────────────────────────

    #[test]
    fn test_analyze_backlinks() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let info = vault.analyze_backlinks("Project Alpha").unwrap();
        assert_eq!(info.note_title, "Project Alpha");
        assert!(info.backlink_count >= 1); // index links to it
        assert!(info.forward_link_count >= 2); // links to index and Team
        assert!(!info.is_orphan);
    }

    #[test]
    fn test_analyze_backlinks_orphan() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        // "old-note" has no incoming or outgoing links
        let info = vault.analyze_backlinks("old-note").unwrap();
        assert!(info.is_orphan);
        assert_eq!(info.backlink_count, 0);
    }

    #[test]
    fn test_analyze_backlinks_not_found() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        assert!(vault.analyze_backlinks("nonexistent").is_err());
    }

    #[test]
    fn test_find_links_to_with_line_numbers() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let info = vault.analyze_backlinks("Project Alpha").unwrap();
        // Backlinks should have line numbers
        for bl in &info.backlinks {
            assert!(bl.line_number.is_some());
            assert!(bl.line_number.unwrap() > 0);
        }
    }

    // ── Vault graph tests ───────────────────────────────────────────

    #[test]
    fn test_analyze_vault_graph() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let graph = vault.analyze_vault_graph().unwrap();
        assert_eq!(graph.note_count, 4);
        assert!(graph.link_count > 0);
        assert!(graph.orphan_count >= 1); // old-note is an orphan
        assert!(!graph.most_linked.is_empty());
        assert!(!graph.most_linking.is_empty());
    }

    #[test]
    fn test_vault_graph_most_linked() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let graph = vault.analyze_vault_graph().unwrap();
        // "Project Alpha" should be one of the most linked
        assert!(graph
            .most_linked
            .iter()
            .any(|(title, _)| title == "Project Alpha"));
    }

    // ── Tag analysis tests ──────────────────────────────────────────

    #[test]
    fn test_analyze_tags() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let analysis = vault.analyze_tags().unwrap();
        assert!(analysis.tag_count >= 4);
        assert!(!analysis.top_tags.is_empty());
        assert!(analysis.tags.contains_key(&"project".to_string()));
        assert!(analysis.tags.contains_key(&"meeting".to_string()));
        assert!(analysis.tags.contains_key(&"status/active".to_string()));
    }

    #[test]
    fn test_tag_hierarchy() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let analysis = vault.analyze_tags().unwrap();
        assert!(analysis.hierarchy.contains_key(&"status".to_string()));
        let children = &analysis.hierarchy["status"];
        assert!(children.contains(&"status/active".to_string()));
        assert!(children.contains(&"status/inactive".to_string()));
    }

    #[test]
    fn test_singleton_tags() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let analysis = vault.analyze_tags().unwrap();
        // "archive" is only used in old-note
        assert!(analysis.singleton_tags.contains(&"archive".to_string()));
    }

    // ── Note preview tests ──────────────────────────────────────────

    #[test]
    fn test_note_preview() {
        let note = Note {
            path: "test.md".to_string(),
            title: "test".to_string(),
            content: "Some content that is long enough to need truncation at some point"
                .to_string(),
            tags: BTreeSet::new(),
            forward_links: BTreeSet::new(),
            size_bytes: 100,
            modified: None,
        };

        let preview = note.preview(20);
        assert!(preview.len() <= 20);
    }

    #[test]
    fn test_note_preview_skips_frontmatter() {
        let note = Note {
            path: "test.md".to_string(),
            title: "test".to_string(),
            content: "---\ntitle: Test\ndate: 2024-01-01\n---\nActual content here".to_string(),
            tags: BTreeSet::new(),
            forward_links: BTreeSet::new(),
            size_bytes: 100,
            modified: None,
        };

        let preview = note.preview(200);
        assert!(preview.starts_with("Actual content"));
    }

    // ── Search mode tests ───────────────────────────────────────────

    #[test]
    fn test_search_mode_display() {
        assert_eq!(format!("{}", SearchMode::Fuzzy), "fuzzy");
        assert_eq!(format!("{}", SearchMode::Exact), "exact");
        assert_eq!(format!("{}", SearchMode::Regex), "regex");
    }

    #[test]
    fn test_search_scope_display() {
        assert_eq!(format!("{}", SearchScope::Title), "title");
        assert_eq!(format!("{}", SearchScope::Content), "content");
        assert_eq!(format!("{}", SearchScope::All), "all");
    }

    // ── Search with regex ───────────────────────────────────────────

    #[test]
    fn test_search_regex() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let results = vault
            .search_with_options("project|meeting", SearchMode::Regex, SearchScope::Title)
            .unwrap();
        assert!(results.total_matches >= 2);
    }

    // ── Skill instructions test ─────────────────────────────────────

    #[test]
    fn test_skill_instructions() {
        let instructions = skill_instructions();
        assert!(instructions.contains("Obsidian Vault Skill"));
        assert!(instructions.contains("Note Search"));
        assert!(instructions.contains("Backlink"));
        assert!(instructions.contains("Git Version Control"));
    }

    // ── Empty vault tests ───────────────────────────────────────────

    #[test]
    fn test_empty_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = make_vault(tmp.path());

        let notes = vault.list_notes().unwrap();
        assert!(notes.is_empty());

        let results = vault.search("anything").unwrap();
        assert_eq!(results.total_matches, 0);

        let analysis = vault.analyze_tags().unwrap();
        assert_eq!(analysis.tag_count, 0);
    }

    // ── Debug implementation test ───────────────────────────────────

    #[test]
    fn test_debug_format() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());
        let debug = format!("{:?}", vault);
        assert!(debug.contains("ObsidianVault"));
    }

    // ── Snippet extraction test ─────────────────────────────────────

    #[test]
    fn test_extract_snippet() {
        let tmp = setup_test_vault();
        let vault = make_vault(tmp.path());

        let note = vault.read_note("meeting-notes.md").unwrap();
        let snippet = vault.extract_snippet(&note.content, "discussed", 50);
        assert!(snippet.is_some());
        let s = snippet.unwrap();
        assert!(s.contains("discussed"));
    }

    // ── Serialization roundtrip tests ───────────────────────────────

    #[test]
    fn test_note_serde_roundtrip() {
        let note = Note {
            path: "test/note.md".to_string(),
            title: "note".to_string(),
            content: "Content with [[link]] and #tag".to_string(),
            tags: {
                let mut s = BTreeSet::new();
                s.insert("tag".to_string());
                s
            },
            forward_links: {
                let mut s = BTreeSet::new();
                s.insert("link".to_string());
                s
            },
            size_bytes: 30,
            modified: Some(1700000000),
        };

        let json = serde_json::to_string(&note).unwrap();
        let parsed: Note = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, note.title);
        assert_eq!(parsed.tags, note.tags);
        assert_eq!(parsed.forward_links, note.forward_links);
    }

    #[test]
    fn test_backlink_info_serde_roundtrip() {
        let info = BacklinkInfo {
            note_title: "Target".to_string(),
            backlinks: vec![LinkRef {
                source_title: "Source".to_string(),
                source_path: "source.md".to_string(),
                display_text: Some("click here".to_string()),
                line_number: Some(5),
            }],
            forward_links: vec![],
            is_orphan: false,
            backlink_count: 1,
            forward_link_count: 0,
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: BacklinkInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.note_title, "Target");
        assert_eq!(parsed.backlinks.len(), 1);
        assert_eq!(parsed.backlinks[0].line_number, Some(5));
    }

    #[test]
    fn test_vault_graph_serde_roundtrip() {
        let graph = VaultGraph {
            note_count: 10,
            link_count: 25,
            orphan_count: 3,
            most_linked: vec![("Alpha".to_string(), 5), ("Beta".to_string(), 3)],
            most_linking: vec![("Index".to_string(), 10)],
            tag_clusters: BTreeMap::new(),
        };

        let json = serde_json::to_string(&graph).unwrap();
        let parsed: VaultGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.note_count, 10);
        assert_eq!(parsed.most_linked.len(), 2);
    }

    #[test]
    fn test_tag_analysis_serde_roundtrip() {
        let analysis = TagAnalysis {
            tags: BTreeMap::new(),
            tag_count: 5,
            top_tags: vec![("rust".to_string(), 10)],
            singleton_tags: vec!["unique".to_string()],
            hierarchy: BTreeMap::new(),
        };

        let json = serde_json::to_string(&analysis).unwrap();
        let parsed: TagAnalysis = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tag_count, 5);
        assert_eq!(parsed.top_tags.len(), 1);
    }

    #[test]
    fn test_git_status_serde_roundtrip() {
        let status = GitStatus {
            is_repo: true,
            branch: Some("main".to_string()),
            uncommitted_changes: 3,
            staged: vec!["a.md".to_string()],
            modified: vec!["b.md".to_string()],
            untracked: vec!["c.md".to_string()],
        };

        let json = serde_json::to_string(&status).unwrap();
        let parsed: GitStatus = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_repo);
        assert_eq!(parsed.branch, Some("main".to_string()));
        assert_eq!(parsed.staged.len(), 1);
    }

    #[test]
    fn test_git_commit_result_serde_roundtrip() {
        let result = GitCommitResult {
            success: true,
            hash: Some("abc1234".to_string()),
            files_committed: 5,
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: GitCommitResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.hash, Some("abc1234".to_string()));
    }
}
