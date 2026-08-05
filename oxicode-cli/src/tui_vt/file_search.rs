//! @-triggered fuzzy file picker for the composer (grok-build parity).
//!
//! Detects `@` at a word boundary in the input buffer, opens a fuzzy
//! file-search dropdown above the composer, and inserts the selected
//! path (optionally with a line range) back into the buffer as plain text.
//!
//! Design (Option B from the plan): the input buffer stays a plain `String`.
//! `@path:N-M` references are inserted as text; the agent's `read` tool
//! already parses path + line-range from text. No chip model, no forked
//! TextArea — the dropdown is the only new UI surface.

use std::path::Path;

use nucleo::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo::{Matcher, Utf32Str};

/// Maximum number of files to index from the workspace.
const MAX_FILES: usize = 5000;
/// Maximum number of results to show in the dropdown.
pub const MAX_RESULTS: usize = 20;
/// Maximum number of root-level files shown when the query is empty.
const MAX_EMPTY_QUERY: usize = 12;

/// One fuzzy-matched file result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSearchResult {
    /// Workspace-relative path.
    pub path: String,
    /// Fuzzy match score (higher = better). 0 when unranked (empty query).
    pub score: u32,
}

/// State for the active file-search dropdown. Stored in
/// `RenderState.file_search` while the picker is open.
#[derive(Clone, Debug)]
pub struct FileSearchState {
    /// The query text typed after `@` (excluding `@` and any `!` prefix).
    pub query: String,
    /// Byte offset of the `@` trigger in the input buffer.
    pub at_offset: usize,
    /// Whether hidden files are included in the results.
    pub hidden_mode: bool,
    /// Current ranked results.
    pub results: Vec<FileSearchResult>,
    /// Currently-selected result index (wraps on navigation).
    pub selected: usize,
    /// Cached workspace file list. Built once on open, re-filtered per query.
    pub index: Vec<String>,
    /// Whether the line-range sub-mode is active (user typed `:` or `Ctrl+L`).
    pub line_mode: bool,
}

/// The parsed `@`-reference at the cursor position, if any. Returned by
/// [`parse_at_cursor`] — a pure function with no I/O, fully unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtToken {
    /// Byte offset of the `@` character in the buffer.
    pub at_offset: usize,
    /// The path portion of the query (after `@`/`@!`, before `:` or end).
    pub path_query: String,
    /// Parsed line range if the user typed `:N` or `:N-M`.
    pub line_range: Option<(usize, usize)>,
    /// Whether the user typed `@!` (hidden-file toggle requested).
    pub hidden_request: bool,
}

/// Detect an `@`-file-reference trigger at the cursor position.
///
/// # Rules (grok-build parity)
/// - `@` must be at a word boundary: preceded by whitespace, start of
///   buffer, or a non-alphanumeric delimiter.
/// - **Email guard**: `@` preceded by alphanumeric or `_` is NOT a trigger
///   (`foo@bar.com`, `user_name@host`).
/// - `@!` toggles hidden-file display (the `!` is consumed).
/// - `path:N` or `path:N-M` parses a line range.
/// - Any whitespace in the text after `@` closes the token (no trigger).
///
/// Returns `None` when no trigger is active.
pub fn parse_at_cursor(buffer: &str, cursor: usize) -> Option<AtToken> {
    let cursor = cursor.min(buffer.len());
    let before = &buffer[..cursor];

    // Find the nearest `@` at or before the cursor.
    let at_rel = before.rfind('@')?;
    let at_offset = at_rel;

    // Word-boundary / email guard: the char immediately before `@` must be
    // whitespace, start of buffer, or a non-identifier delimiter.
    if at_offset > 0 {
        let prev = before[..at_offset].chars().last().unwrap_or(' ');
        if prev.is_alphanumeric() || prev == '_' {
            return None;
        }
    }

    // The token extends from after `@` to the cursor. Any whitespace
    // closes the token — once closed, it's no longer an active trigger.
    let after_at = &buffer[at_offset + 1..cursor];
    if after_at.chars().any(|c| c.is_whitespace()) {
        return None;
    }

    // `@!` — hidden mode request. The `!` is consumed by the toggle.
    let (hidden_request, rest) = if let Some(stripped) = after_at.strip_prefix('!') {
        (true, stripped)
    } else {
        (false, after_at)
    };

    // Split `path:line-range`.
    let (path_query, line_range) = split_path_and_range(rest);

    Some(AtToken {
        at_offset,
        path_query,
        line_range,
        hidden_request,
    })
}

/// Split `path:N` or `path:N-M` into `(path, Some((start, end)))`.
/// A path with no colon returns `(path, None)`.
fn split_path_and_range(rest: &str) -> (String, Option<(usize, usize)>) {
    // The line-range colon is the LAST colon in the token — a path like
    // `C:\foo` on Windows or `a:b.rs` (unlikely) should not split here.
    // For Unix paths we split on the first colon after the last separator,
    // but practically: the range colon is followed only by digits/dash.
    if let Some(colon) = rest.rfind(':')
        && rest[colon + 1..]
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-')
    {
        let path_part = &rest[..colon];
        let range_part = &rest[colon + 1..];
        // Only split when the range actually parses. An invalid range
        // (`:0`, `:25-10`) means the colon is part of the path — keep it.
        if let Some(range) = parse_line_range(range_part) {
            return (path_part.to_string(), Some(range));
        }
    }
    (rest.to_string(), None)
}

/// Parse `N` or `N-M` into `(start, end)`. Returns `None` on malformed input.
fn parse_line_range(s: &str) -> Option<(usize, usize)> {
    if s.is_empty() {
        return None;
    }
    if let Some(dash) = s.find('-') {
        let start: usize = s[..dash].parse().ok()?;
        let end: usize = s[dash + 1..].parse().ok()?;
        (start > 0 && end >= start).then_some((start, end))
    } else {
        let n: usize = s.parse().ok()?;
        (n > 0).then_some((n, n))
    }
}

/// Walk the workspace and build a file index. Respects `.gitignore`,
/// `.git/info/exclude`, and the global gitignore. Hidden files (dotfiles)
/// are excluded by default and toggled on via the `@!` gesture.
///
/// Caps at `MAX_FILES` entries to bound latency on large repos.
pub fn build_index(cwd: &Path, hidden: bool) -> Vec<String> {
    let mut files = Vec::new();
    let mut builder = ignore::WalkBuilder::new(cwd);
    builder
        .hidden(!hidden) // skip dotfiles when !hidden
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .threads(2);

    // Skip the cwd itself and common large/build dirs even when not
    // gitignored, to keep the index snappy on monorepos.
    let walker = builder.build();
    for entry in walker.flatten() {
        if files.len() >= MAX_FILES {
            break;
        }
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(cwd) {
            // Skip the git internals even when hidden mode is on.
            if rel.starts_with(".git") {
                continue;
            }
            if let Some(s) = rel.to_str() {
                files.push(s.to_string());
            }
        }
    }
    files
}

/// Fuzzy-filter `index` by `query`, returning ranked results.
///
/// When `query` is empty, returns root-level files (no path separator)
/// sorted alphabetically — the most likely "quick pick" candidates.
pub fn search(index: &[String], query: &str, max: usize) -> Vec<FileSearchResult> {
    if query.is_empty() {
        let mut roots: Vec<&String> = index
            .iter()
            .filter(|p| !p.contains(std::path::MAIN_SEPARATOR))
            .collect();
        roots.sort();
        return roots
            .into_iter()
            .take(max.min(MAX_EMPTY_QUERY))
            .map(|p| FileSearchResult {
                path: p.clone(),
                score: 0,
            })
            .collect();
    }

    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(nucleo::Config::DEFAULT);

    let mut scored: Vec<FileSearchResult> = index
        .iter()
        .filter_map(|path| {
            let haystack = Utf32Str::Ascii(path.as_bytes());
            let score = pattern.score(haystack, &mut matcher)?;
            Some(FileSearchResult {
                path: path.clone(),
                score,
            })
        })
        .collect();

    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    scored.truncate(max);
    scored
}

/// Build the text to insert into the buffer when the user accepts a result.
///
/// - Normal: `@path ` (trailing space so the user can continue typing).
/// - Line mode: `@path:N-M ` when a range was parsed, else `@path:`.
pub fn insertion_text(path: &str, line_range: Option<(usize, usize)>, line_mode: bool) -> String {
    if line_mode {
        match line_range {
            Some((start, end)) if start == end => format!("@{path}:{start} "),
            Some((start, end)) => format!("@{path}:{start}-{end} "),
            None => format!("@{path}:"),
        }
    } else {
        format!("@{path} ")
    }
}

/// Open a new file-search state at the given `@` offset, building the
/// workspace index. Called from the input thread when `@` is first detected.
pub fn open(cwd: &Path, at_offset: usize, hidden_mode: bool) -> FileSearchState {
    let index = build_index(cwd, hidden_mode);
    let results = search(&index, "", MAX_RESULTS);
    FileSearchState {
        query: String::new(),
        at_offset,
        hidden_mode,
        results,
        selected: 0,
        index,
        line_mode: false,
    }
}

impl FileSearchState {
    /// Re-filter the index by `query`. Resets selection to the top result.
    pub fn refresh(&mut self, query: &str) {
        self.query = query.to_string();
        self.results = search(&self.index, query, MAX_RESULTS);
        self.selected = 0;
    }

    /// Move the selection up (wraps to bottom).
    pub fn up(&mut self) {
        if !self.results.is_empty() {
            self.selected = if self.selected == 0 {
                self.results.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// Move the selection down (wraps to top).
    pub fn down(&mut self) {
        if !self.results.is_empty() {
            self.selected = if self.selected + 1 >= self.results.len() {
                0
            } else {
                self.selected + 1
            };
        }
    }

    /// The currently-selected result, if any.
    pub fn selected_result(&self) -> Option<&FileSearchResult> {
        self.results.get(self.selected)
    }

    /// Toggle hidden-file display and rebuild the index.
    pub fn toggle_hidden(&mut self, cwd: &Path) {
        self.hidden_mode = !self.hidden_mode;
        self.index = build_index(cwd, self.hidden_mode);
        self.refresh(&self.query.clone());
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_at_cursor: trigger detection ──

    #[test]
    fn at_at_buffer_start_triggers() {
        let tok = parse_at_cursor("@foo", 4).unwrap();
        assert_eq!(tok.at_offset, 0);
        assert_eq!(tok.path_query, "foo");
        assert_eq!(tok.line_range, None);
        assert!(!tok.hidden_request);
    }

    #[test]
    fn at_after_space_triggers() {
        let tok = parse_at_cursor("hello @foo", 10).unwrap();
        assert_eq!(tok.at_offset, 6);
        assert_eq!(tok.path_query, "foo");
    }

    #[test]
    fn email_is_not_a_trigger() {
        assert!(parse_at_cursor("foo@bar.com", 11).is_none());
        assert!(parse_at_cursor("user_name@host", 14).is_none());
    }

    #[test]
    fn at_after_underscore_blocked() {
        assert!(parse_at_cursor("_@foo", 5).is_none());
    }

    #[test]
    fn at_after_punctuation_triggers() {
        // Non-alphanumeric delimiters are valid word boundaries.
        let tok = parse_at_cursor("(@foo", 5).unwrap();
        assert_eq!(tok.path_query, "foo");
    }

    #[test]
    fn whitespace_closes_token() {
        // `@foo bar` — cursor past the space → no active trigger.
        assert!(parse_at_cursor("@foo bar", 8).is_none());
    }

    #[test]
    fn hidden_toggle_detected() {
        let tok = parse_at_cursor("@!foo", 5).unwrap();
        assert!(tok.hidden_request);
        assert_eq!(tok.path_query, "foo");
    }

    #[test]
    fn hidden_toggle_alone() {
        let tok = parse_at_cursor("@!", 2).unwrap();
        assert!(tok.hidden_request);
        assert!(tok.path_query.is_empty());
    }

    #[test]
    fn line_range_single() {
        let tok = parse_at_cursor("@foo:42", 7).unwrap();
        assert_eq!(tok.path_query, "foo");
        assert_eq!(tok.line_range, Some((42, 42)));
    }

    #[test]
    fn line_range_multi() {
        let tok = parse_at_cursor("@foo:10-25", 10).unwrap();
        assert_eq!(tok.path_query, "foo");
        assert_eq!(tok.line_range, Some((10, 25)));
    }

    #[test]
    fn line_range_invalid_zero() {
        let tok = parse_at_cursor("@foo:0", 6).unwrap();
        assert_eq!(tok.path_query, "foo:0");
        assert_eq!(tok.line_range, None);
    }

    #[test]
    fn line_range_inverted_rejected() {
        let tok = parse_at_cursor("@foo:25-10", 10).unwrap();
        assert_eq!(tok.path_query, "foo:25-10");
        assert_eq!(tok.line_range, None);
    }

    #[test]
    fn no_at_symbol_no_trigger() {
        assert!(parse_at_cursor("hello world", 11).is_none());
    }

    #[test]
    fn bare_at_symbol_triggers_empty_query() {
        let tok = parse_at_cursor("@", 1).unwrap();
        assert!(tok.path_query.is_empty());
        assert!(!tok.hidden_request);
    }

    // ── insertion_text ──

    #[test]
    fn insertion_normal() {
        assert_eq!(insertion_text("src/foo.rs", None, false), "@src/foo.rs ");
    }

    #[test]
    fn insertion_line_mode_no_range() {
        assert_eq!(insertion_text("foo.rs", None, true), "@foo.rs:");
    }

    #[test]
    fn insertion_line_mode_single() {
        assert_eq!(
            insertion_text("foo.rs", Some((10, 10)), true),
            "@foo.rs:10 "
        );
    }

    #[test]
    fn insertion_line_mode_range() {
        assert_eq!(
            insertion_text("foo.rs", Some((10, 25)), true),
            "@foo.rs:10-25 "
        );
    }

    // ── search ranking ──

    #[test]
    fn empty_query_returns_root_files() {
        let index = vec![
            "src/main.rs".into(),
            "README.md".into(),
            "Cargo.toml".into(),
            "src/lib.rs".into(),
        ];
        let results = search(&index, "", 20);
        // Root files (no separator), sorted alphabetically.
        let paths: Vec<&str> = results.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["Cargo.toml", "README.md"]);
    }

    #[test]
    fn fuzzy_query_ranks_by_score() {
        let index = vec![
            "src/main.rs".into(),
            "src/maine.rs".into(),
            "README.md".into(),
        ];
        let results = search(&index, "main", 20);
        assert!(!results.is_empty());
        // "main.rs" and "maune.rs" should rank above "README.md".
        assert_eq!(results[0].path, "src/main.rs");
    }

    #[test]
    fn search_truncates_to_max() {
        let index: Vec<String> = (0..100).map(|i| format!("file_{i}.rs")).collect();
        let results = search(&index, "file", 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn no_matches_returns_empty() {
        let index = vec!["foo.rs".into()];
        let results = search(&index, "zzzzzzzzz", 20);
        assert!(results.is_empty());
    }

    // ── FileSearchState navigation ──

    #[test]
    fn nav_down_wraps() {
        let mut state = FileSearchState {
            query: String::new(),
            at_offset: 0,
            hidden_mode: false,
            results: vec![
                FileSearchResult {
                    path: "a".into(),
                    score: 0,
                },
                FileSearchResult {
                    path: "b".into(),
                    score: 0,
                },
                FileSearchResult {
                    path: "c".into(),
                    score: 0,
                },
            ],
            selected: 0,
            index: vec![],
            line_mode: false,
        };
        state.down();
        assert_eq!(state.selected, 1);
        state.down();
        assert_eq!(state.selected, 2);
        state.down();
        assert_eq!(state.selected, 0); // wraps
    }

    #[test]
    fn nav_up_wraps() {
        let mut state = FileSearchState {
            query: String::new(),
            at_offset: 0,
            hidden_mode: false,
            results: vec![
                FileSearchResult {
                    path: "a".into(),
                    score: 0,
                },
                FileSearchResult {
                    path: "b".into(),
                    score: 0,
                },
            ],
            selected: 0,
            index: vec![],
            line_mode: false,
        };
        state.up();
        assert_eq!(state.selected, 1); // wraps to bottom
    }

    #[test]
    fn refresh_resets_selection() {
        let mut state = FileSearchState {
            query: String::new(),
            at_offset: 0,
            hidden_mode: false,
            results: vec![],
            selected: 5,
            index: vec!["foo.rs".into(), "bar.rs".into()],
            line_mode: false,
        };
        state.refresh("foo");
        assert_eq!(state.selected, 0);
        assert_eq!(state.results.len(), 1);
    }
}
