//! Git TUI — interactive overlay for `git status`, diff viewing, staging, commit.
//!
//! Module layout (built incrementally across tasks 11a/11b):
//!
//! * [`diff_doc`] — unified diff parser + whitespace/formatting filters (pure)
//! * [`state`] — `git status --porcelain -z` parser (pure)
//! * [`keys`] — keymap → [`GitKeyAction`] (pure)
//! * [`git_io`] — thin `std::process::Command` wrappers around `git` (impure)
//! * [`render`] — ratatui pane layout + pairing math (pure layout, impure draw)
//!
//! `GitTuiState` here is the interactive overlay's source of truth — it
//! composes the pure data modules above and exposes mutating methods that
//! shell out to `git_io` for state changes (`add`/`restore --staged`/`commit`).

pub mod diff_doc;
pub mod git_io;
pub mod keys;
pub mod render;
pub mod state;

pub use diff_doc::{
    DiffDocument, DiffFile, DiffLine, DiffLineKind, DiffViewMode, Hunk, WhitespaceMode,
    filter_whitespace, parse_unified_diff,
};
pub use git_io::{diff_head, run_git, status_porcelain_z};
pub use keys::{GitKeyAction, match_git_key};
pub use render::{
    RenderPlan, hunk_scroll_offset, pair_split_view, plan_overlay, render_overlay_lines,
    render_sidebar_rows,
};
pub use state::{StatusEntry, parse_status_porcelain_z};

use std::collections::HashSet;
use std::path::Path;

/// Interactive overlay state. One instance per open `/git` session.
///
/// `entries` are kept in the order produced by `git status --porcelain -z` —
/// the sidebar renders them as-is. `staged` mirrors the porcelain index
/// column: paths whose `xy[0]` is neither `' '` (unstaged) nor `'?'`
/// (untracked) are staged. It is re-derived on every [`GitTuiState::load`]
/// and [`GitTuiState::refresh`], so files staged outside the overlay
/// (plain `git add` in another terminal) are operable too.
///
/// `needs_refresh` is available for callers that batch mutations and want
/// to force a refresh on the next frame; it is consumed (cleared) by
/// [`GitTuiState::refresh`]. The built-in mutators
/// ([`GitTuiState::toggle_stage`], [`GitTuiState::commit`]) refresh
/// inline after their git command succeeds, so the overlay reflects the
/// new state on the very next frame without it.
#[derive(Debug, Clone)]
pub struct GitTuiState {
    pub doc: DiffDocument,
    pub entries: Vec<StatusEntry>,
    pub view: DiffViewMode,
    pub ws: WhitespaceMode,
    pub selected_file: usize,
    pub selected_hunk: usize,
    pub sidebar_focus: bool,
    pub staged: HashSet<String>,
    pub commit_mode: bool,
    pub commit_msg: String,
    pub needs_refresh: bool,
    pub width: u16,
    pub height: u16,
    /// Cached `git branch --show-current` captured at [`GitTuiState::load`].
    /// Refreshed by [`GitTuiState::refresh`].
    pub branch: Option<String>,
    /// Soft-wrap toggle for the diff pane (the brief's `w` binding).
    pub wrap: bool,
}

impl GitTuiState {
    /// Load status + diff for `cwd`. Builds the entry list from
    /// `git status --porcelain -z` and the diff document from `git diff HEAD
    /// --no-ext-diff`. Untracked entries (`XY[0]=='?'`) get a placeholder
    /// [`DiffFile`] with no hunks so selection still works.
    pub fn load(cwd: &Path) -> anyhow::Result<Self> {
        let entries = status_porcelain_z(cwd)?;
        let diff_text = diff_head(cwd)?;
        let mut doc = parse_unified_diff(&diff_text);
        // Insert placeholders for untracked entries (`??`) so the sidebar
        // can select them and render "(untracked)".
        for entry in &entries {
            if entry.xy[0] == '?' && !doc.files.iter().any(|f| f.path == entry.path) {
                doc.files.push(DiffFile {
                    path: entry.path.clone(),
                    old_path: None,
                    hunks: Vec::new(),
                    binary: false,
                });
            }
        }
        let branch = crate::util::git_utils::get_current_branch(cwd);
        Ok(Self {
            doc,
            staged: Self::staged_from_entries(&entries),
            entries,
            view: DiffViewMode::default(),
            ws: WhitespaceMode::default(),
            selected_file: 0,
            selected_hunk: 0,
            sidebar_focus: false,
            commit_mode: false,
            commit_msg: String::new(),
            needs_refresh: false,
            width: 80,
            height: 24,
            branch,
            wrap: false,
        })
    }

    /// Re-derive staged-ness from the porcelain XY index column
    /// (final-review finding 4): a path is staged when `xy[0]` is
    /// neither `' '` (not in the index) nor `'?'` (untracked). This
    /// sees external `git add`s too, not just overlay-issued ones.
    fn staged_from_entries(entries: &[StatusEntry]) -> HashSet<String> {
        entries
            .iter()
            .filter(|e| e.xy[0] != ' ' && e.xy[0] != '?')
            .map(|e| e.path.clone())
            .collect()
    }

    /// Re-run `git status` + `git diff HEAD` and rebuild the document.
    /// Preserves the current selection where possible (clamped to the new
    /// entry count).
    pub fn refresh(&mut self, cwd: &Path) -> anyhow::Result<()> {
        let entries = status_porcelain_z(cwd)?;
        let diff_text = diff_head(cwd)?;
        let mut doc = parse_unified_diff(&diff_text);
        for entry in &entries {
            if entry.xy[0] == '?' && !doc.files.iter().any(|f| f.path == entry.path) {
                doc.files.push(DiffFile {
                    path: entry.path.clone(),
                    old_path: None,
                    hunks: Vec::new(),
                    binary: false,
                });
            }
        }
        let branch = crate::util::git_utils::get_current_branch(cwd);
        self.doc = doc;
        self.staged = Self::staged_from_entries(&entries);
        self.entries = entries;
        self.branch = branch;
        if self.selected_file >= self.entries.len() {
            self.selected_file = self.entries.len().saturating_sub(1);
        }
        self.needs_refresh = false;
        Ok(())
    }

    /// Toggle staging for `path`. Staged entries leave the unstaged list
    /// (status output will show them with `X` != space); unstaged entries
    /// reappear in the unstaged list. Refreshes inline on success so
    /// `entries` / `doc` / `staged` reflect the git state immediately
    /// (final-review finding 3 — the overlay used to keep showing the
    /// pre-mutation diff until a manual `r`).
    pub fn toggle_stage(&mut self, cwd: &Path, path: &str) -> anyhow::Result<()> {
        if self.staged.contains(path) {
            // Currently staged → unstage.
            run_git(cwd, &["restore", "--staged", "--", path])?;
        } else {
            run_git(cwd, &["add", "--", path])?;
        }
        // `refresh` re-derives `staged` from the porcelain output, so
        // the mirror stays in sync with reality (including external
        // staging) without manual bookkeeping here.
        self.refresh(cwd)
    }

    /// Commit with the current `commit_msg`. Clears the message on
    /// success and refreshes inline so the committed change leaves the
    /// diff view on the next frame. Empty messages are rejected
    /// (`git commit -m ""` would otherwise open an editor and hang the
    /// overlay).
    pub fn commit(&mut self, cwd: &Path) -> anyhow::Result<()> {
        let msg = self.commit_msg.trim();
        if msg.is_empty() {
            anyhow::bail!("commit message is empty");
        }
        run_git(cwd, &["commit", "-m", msg])?;
        self.commit_msg.clear();
        self.commit_mode = false;
        self.refresh(cwd)
    }

    /// Apply a [`GitKeyAction`] (or raw char for commit-mode text input).
    /// Returns `true` if the action was consumed.
    pub fn apply_action(&mut self, cwd: &Path, action: GitKeyAction) -> anyhow::Result<bool> {
        match action {
            GitKeyAction::Close => return Ok(false), // closed by main_loop
            GitKeyAction::Down => {
                if self.sidebar_focus {
                    if !self.entries.is_empty() {
                        self.selected_file = (self.selected_file + 1).min(self.entries.len() - 1);
                    }
                } else if let Some(file) = self.doc.files.get(self.selected_file)
                    && !file.hunks.is_empty()
                {
                    self.selected_hunk = (self.selected_hunk + 1).min(file.hunks.len() - 1);
                }
            }
            GitKeyAction::Up => {
                if self.sidebar_focus {
                    self.selected_file = self.selected_file.saturating_sub(1);
                } else {
                    self.selected_hunk = self.selected_hunk.saturating_sub(1);
                }
            }
            GitKeyAction::GotoTop => {
                if self.sidebar_focus {
                    self.selected_file = 0;
                } else {
                    self.selected_hunk = 0;
                }
            }
            GitKeyAction::GotoBottom => {
                if self.sidebar_focus {
                    self.selected_file = self.entries.len().saturating_sub(1);
                } else if let Some(file) = self.doc.files.get(self.selected_file) {
                    self.selected_hunk = file.hunks.len().saturating_sub(1);
                }
            }
            GitKeyAction::HunkNext => {
                if let Some(file) = self.doc.files.get(self.selected_file)
                    && !file.hunks.is_empty()
                {
                    self.selected_hunk = (self.selected_hunk + 1).min(file.hunks.len() - 1);
                }
            }
            GitKeyAction::HunkPrev => {
                self.selected_hunk = self.selected_hunk.saturating_sub(1);
            }
            GitKeyAction::FileNext => {
                if !self.entries.is_empty() {
                    self.selected_file = (self.selected_file + 1).min(self.entries.len() - 1);
                    self.selected_hunk = 0;
                }
            }
            GitKeyAction::FilePrev => {
                self.selected_file = self.selected_file.saturating_sub(1);
                self.selected_hunk = 0;
            }
            GitKeyAction::ViewMode(mode) => {
                self.view = match mode {
                    1 => DiffViewMode::Split,
                    2 => DiffViewMode::Inline,
                    3 => DiffViewMode::Hunks,
                    4 => DiffViewMode::Files,
                    _ => self.view,
                };
            }
            GitKeyAction::ToggleSidebar => {
                self.sidebar_focus = !self.sidebar_focus;
            }
            GitKeyAction::CycleWhitespace => {
                self.ws = match self.ws {
                    WhitespaceMode::Off => WhitespaceMode::IgnoreWhitespace,
                    WhitespaceMode::IgnoreWhitespace => WhitespaceMode::IgnoreFormatting,
                    WhitespaceMode::IgnoreFormatting => WhitespaceMode::Off,
                };
            }
            GitKeyAction::ToggleWrap => {
                self.wrap = !self.wrap;
            }
            GitKeyAction::Stage | GitKeyAction::Unstage => {
                if let Some(entry) = self.entries.get(self.selected_file) {
                    let path = entry.path.clone();
                    let add = matches!(action, GitKeyAction::Stage);
                    let already_staged = self.staged.contains(&path);
                    if add != already_staged {
                        self.toggle_stage(cwd, &path)?;
                    }
                }
            }
            GitKeyAction::Commit => {
                self.commit_mode = true;
            }
            GitKeyAction::Refresh => {
                self.refresh(cwd)?;
            }
            GitKeyAction::Left | GitKeyAction::Right => {
                // Sidebar tree collapse/expand deferred to v1.1 — the brief
                // marks the sidebar as a flat list.
            }
        }
        Ok(true)
    }

    /// Append one char to the commit message (commit-mode text input).
    pub fn commit_input_char(&mut self, ch: char) {
        if self.commit_mode {
            self.commit_msg.push(ch);
        }
    }

    /// Backspace one char from the commit message (commit-mode).
    pub fn commit_backspace(&mut self) {
        if self.commit_mode {
            self.commit_msg.pop();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (TDD — written first, made green by impl)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `git --version` succeeds — skip the binary-dependent tests when git
    /// is missing (CI runners without git, offline sandboxes, etc.).
    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Make a unique temp directory; caller is responsible for cleanup.
    fn temp_repo_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("oxicode-git-tui-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp repo dir");
        dir
    }

    /// Init a temp repo, set git identity, commit an initial file.
    /// Returns the repo root.
    fn init_repo(label: &str, filename: &str, content: &str) -> PathBuf {
        let dir = temp_repo_dir(label);
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "tester")
                .env("GIT_AUTHOR_EMAIL", "tester@example.com")
                .env("GIT_COMMITTER_NAME", "tester")
                .env("GIT_COMMITTER_EMAIL", "tester@example.com")
                .status()
                .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
            assert!(status.success(), "git {args:?} failed: {status}");
        };
        run(&["init", "--quiet"]);
        std::fs::write(dir.join(filename), content).unwrap();
        run(&["add", "--", filename]);
        run(&["commit", "--quiet", "-m", "init"]);
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_populates_entries_and_doc() {
        if !git_available() {
            eprintln!("git binary not available — skipping");
            return;
        }
        let dir = init_repo("load", "hello.txt", "first\n");
        // Modify + add a new file so we get both `M` and `??` statuses.
        std::fs::write(dir.join("hello.txt"), "first\nsecond\n").unwrap();
        std::fs::write(dir.join("new.txt"), "untracked body\n").unwrap();

        let state = GitTuiState::load(&dir).expect("load");
        let paths: Vec<&str> = state.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"hello.txt"),
            "entries missing hello.txt: {paths:?}"
        );
        assert!(
            paths.contains(&"new.txt"),
            "entries missing new.txt: {paths:?}"
        );
        // `hello.txt` is modified (in the diff), so the doc carries it.
        let doc_paths: Vec<&str> = state.doc.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            doc_paths.contains(&"hello.txt"),
            "doc missing hello.txt: {doc_paths:?}"
        );
        // `new.txt` is untracked → placeholder DiffFile must exist.
        assert!(
            state
                .doc
                .files
                .iter()
                .any(|f| f.path == "new.txt" && f.hunks.is_empty()),
            "untracked placeholder missing"
        );

        cleanup(&dir);
    }

    #[test]
    fn toggle_stage_moves_path_between_staged_and_entries() {
        if !git_available() {
            return;
        }
        let dir = init_repo("stage", "a.txt", "alpha\n");
        std::fs::write(dir.join("a.txt"), "alpha\nbeta\n").unwrap();

        let mut state = GitTuiState::load(&dir).expect("load");
        // Find index of a.txt in entries.
        let idx = state
            .entries
            .iter()
            .position(|e| e.path == "a.txt")
            .expect("a.txt entry");
        state.selected_file = idx;

        // Stage it via the public method (mirrors `s` keypress).
        // Mutations refresh inline (final-review finding 3): entries /
        // staged must already reflect the change — no manual `r`.
        state.toggle_stage(&dir, "a.txt").expect("stage");
        assert!(
            state.staged.contains("a.txt"),
            "expected a.txt in staged set right after toggle_stage"
        );
        let staged_entry = state.entries.iter().find(|e| e.path == "a.txt").unwrap();
        assert_eq!(
            staged_entry.xy,
            ['M', ' '],
            "after stage a.txt should be staged-modification (XY=['M',' '])"
        );

        // Toggle again → unstage. Inline refresh re-derives; the
        // worktree slot carries `M` again.
        state.toggle_stage(&dir, "a.txt").expect("unstage");
        assert!(
            !state.staged.contains("a.txt"),
            "expected a.txt removed from staged"
        );
        let unstaged_entry = state.entries.iter().find(|e| e.path == "a.txt").unwrap();
        assert_eq!(
            unstaged_entry.xy,
            [' ', 'M'],
            "after unstage a.txt should be worktree-modification (XY=[' ','M'])"
        );

        cleanup(&dir);
    }

    #[test]
    fn externally_staged_files_are_operable() {
        // Final-review finding 4: the `staged` mirror is derived from
        // the porcelain XY index column, so a file staged OUTSIDE the
        // overlay (plain `git add` in a shell) is visible as staged
        // and `u` (unstage) actually works on it.
        if !git_available() {
            return;
        }
        let dir = init_repo("extstage", "e.txt", "epsilon\n");
        std::fs::write(dir.join("e.txt"), "epsilon\nzeta\n").unwrap();
        // External staging — NOT via the overlay.
        run_git(&dir, &["add", "--", "e.txt"]).expect("external git add");

        let mut state = GitTuiState::load(&dir).expect("load");
        assert!(
            state.staged.contains("e.txt"),
            "externally staged e.txt must be in the staged mirror: {:?}",
            state.entries
        );

        // Unstage via the overlay: previously `already_staged` was
        // false for external staging, so `u` was a silent no-op.
        state.toggle_stage(&dir, "e.txt").expect("unstage external");
        let entry = state.entries.iter().find(|e| e.path == "e.txt").unwrap();
        assert_eq!(
            entry.xy[0], ' ',
            "after unstage the index column must be blank again: {:?}",
            entry.xy
        );
        assert!(!state.staged.contains("e.txt"));

        cleanup(&dir);
    }

    #[test]
    fn commit_clears_message_on_success() {
        if !git_available() {
            return;
        }
        let dir = init_repo("commit", "c.txt", "gamma\n");
        std::fs::write(dir.join("c.txt"), "gamma\ndelta\n").unwrap();

        let mut state = GitTuiState::load(&dir).expect("load");
        state.toggle_stage(&dir, "c.txt").expect("stage");
        state.commit_msg = "feat: add delta".to_string();
        state.commit(&dir).expect("commit");
        assert!(
            state.commit_msg.is_empty(),
            "commit_msg must clear on success"
        );
        assert!(!state.commit_mode);
        // Inline refresh after commit (final-review finding 3): the
        // committed change is gone from status and the diff doc.
        assert!(
            state.entries.iter().all(|e| e.path != "c.txt"),
            "committed file must leave the status entries: {:?}",
            state.entries
        );
        assert!(
            state.doc.files.iter().all(|f| f.path != "c.txt"),
            "committed file must leave the diff document"
        );

        // Empty commit message must error.
        state.commit_msg.clear();
        let err = state.commit(&dir).expect_err("empty message rejected");
        assert!(err.to_string().contains("empty"));

        cleanup(&dir);
    }

    #[test]
    fn untracked_file_gets_placeholder_doc_entry() {
        if !git_available() {
            return;
        }
        let dir = init_repo("untracked", "x.txt", "x body\n");
        std::fs::write(dir.join("y.txt"), "new untracked\n").unwrap();

        let state = GitTuiState::load(&dir).expect("load");
        let untracked = state
            .doc
            .files
            .iter()
            .find(|f| f.path == "y.txt")
            .expect("placeholder for y.txt");
        assert!(untracked.hunks.is_empty());
        assert!(!untracked.binary);

        cleanup(&dir);
    }
}
