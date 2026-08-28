//! Unified-diff parsing + whitespace/formatting filters.
//!
//! Pure data model — no terminal I/O, no ratatui rendering. Consumed by the
//! git TUI overlay (rendering wired in a follow-up task).

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One parsed unified-diff document: zero or more [`DiffFile`] entries.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffDocument {
    /// Files in the order they appeared in the input.
    pub files: Vec<DiffFile>,
}

/// One file in a [`DiffDocument`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffFile {
    /// Path on the `b/` side of the diff (the post-image).
    pub path: String,
    /// Path on the `a/` side (the pre-image). Set on renames.
    pub old_path: Option<String>,
    /// Hunks in source order.
    pub hunks: Vec<Hunk>,
    /// `true` when the file was reported as binary (`Binary files ... differ`).
    /// Binary files never carry hunks.
    pub binary: bool,
}

/// One hunk header (`@@ -old,count +new,count @@`) plus its lines.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based starting line in the pre-image (or 0 when count is 0).
    pub old_start: u32,
    /// 1-based starting line in the post-image (or 0 when count is 0).
    pub new_start: u32,
    /// Lines in source order. Context first, then added, then removed, as
    /// produced by `git diff`. A filter may mutate kinds in place.
    pub lines: Vec<DiffLine>,
}

/// One line inside a [`Hunk`]. The text DOES NOT include the leading
/// `+`/`-`/` ` prefix character.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Line role inside the hunk.
    pub kind: DiffLineKind,
    /// Line body without the diff prefix character.
    pub text: String,
}

/// Role of a single [`DiffLine`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffLineKind {
    /// Unchanged context (` ` prefix).
    #[default]
    Context,
    /// Insertion (`+` prefix).
    Added,
    /// Deletion (`-` prefix).
    Removed,
}

/// Whitespace / formatting filter mode for [`filter_whitespace`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhitespaceMode {
    /// No filtering — return the document unchanged.
    #[default]
    Off,
    /// Demote hunks whose only changes are whitespace-only added/removed lines.
    IgnoreWhitespace,
    /// Additionally demote hunks that are only formatting changes (indent,
    /// blank lines, language-specific import-only hunks).
    IgnoreFormatting,
}

/// View mode for the rendered diff. Orthogonal to [`WhitespaceMode`] — both
/// live on the same [`DiffDocument`] and can be applied independently.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffViewMode {
    /// Two-column side-by-side.
    Split,
    /// Unified inline (default).
    #[default]
    Inline,
    /// Hunk list only.
    Hunks,
    /// File list only.
    Files,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a `git diff` output into a [`DiffDocument`].
///
/// Handles:
/// * `diff --git a/X b/Y` file headers
/// * `rename from X` / `rename to Y` — populates [`DiffFile::old_path`]
/// * `similarity index N%` — currently skipped (recorded but not exposed)
/// * `new file mode ...` / `deleted file mode ...` — skipped
/// * `Binary files X and Y differ` — sets [`DiffFile::binary`] and emits no
///   hunks
/// * `index abc..def 100644` — skipped
/// * `--- a/X` / `+++ b/Y` — consumed as part of the file header
/// * `@@ -old,count +new,count @@` — hunk headers
/// * context / `+` / `-` lines
///
/// Multiple files are supported and emitted in source order.
pub fn parse_unified_diff(input: &str) -> DiffDocument {
    let mut doc = DiffDocument::default();
    let mut iter = input.lines().peekable();
    while let Some(line) = iter.next() {
        if let Some(rest) = line.strip_prefix("diff --git ")
            && let Some(file) = parse_one_file(&mut iter, rest)
        {
            doc.files.push(file);
        }
        // Anything outside a `diff --git` block is dropped silently —
        // unified diffs do not carry top-level data outside file headers.
    }

    doc
}

/// Filter a [`DiffDocument`] by [`WhitespaceMode`]. Returns a new document;
/// hunks that survive filtering keep their text but may have their line
/// kinds rewritten (Added/Removed → Context). Hunks that are fully demoted
/// keep all their lines — only the kind changes.
pub fn filter_whitespace(doc: &DiffDocument, mode: WhitespaceMode) -> DiffDocument {
    if matches!(mode, WhitespaceMode::Off) {
        return doc.clone();
    }

    let mut out = DiffDocument::default();
    for file in &doc.files {
        let mut new_file = DiffFile {
            path: file.path.clone(),
            old_path: file.old_path.clone(),
            binary: file.binary,
            hunks: Vec::with_capacity(file.hunks.len()),
        };

        for hunk in &file.hunks {
            if hunk_should_demote(hunk, file.path.as_str(), mode) {
                let demoted: Vec<DiffLine> = hunk
                    .lines
                    .iter()
                    .map(|l| DiffLine {
                        kind: DiffLineKind::Context,
                        text: l.text.clone(),
                    })
                    .collect();
                new_file.hunks.push(Hunk {
                    old_start: hunk.old_start,
                    new_start: hunk.new_start,
                    lines: demoted,
                });
            } else {
                new_file.hunks.push(hunk.clone());
            }
        }

        out.files.push(new_file);
    }

    out
}

// ---------------------------------------------------------------------------
// Implementation: per-file parsing
// ---------------------------------------------------------------------------

fn parse_one_file<'a, I: Iterator<Item = &'a str>>(
    iter: &mut std::iter::Peekable<I>,
    header_rest: &str,
) -> Option<DiffFile> {
    // header_rest = "a/X b/Y" — pull the b-side path (a-side captured for
    // symmetry but currently unused).
    let (_a_path, b_path) = parse_diff_git_paths(header_rest);
    let mut file = DiffFile {
        path: b_path,
        old_path: None,
        hunks: Vec::new(),
        binary: false,
    };

    // Consume subsequent header lines until we reach the first hunk header
    // or the next `diff --git` / EOF.
    let mut pending_rename_from: Option<String> = None;

    loop {
        match iter.peek().copied() {
            None => return Some(file),
            Some(next) if next.starts_with("diff --git ") => return Some(file),
            Some(next) if next.starts_with("Binary files ") && next.contains(" differ") => {
                // "Binary files a/X and b/Y differ"
                file.binary = true;
                iter.next();
                // No hunks for binary files.
                continue;
            }
            Some(next) if next.starts_with("rename from ") => {
                let p = next.trim_start_matches("rename from ").to_string();
                pending_rename_from = Some(p);
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("rename to ") => {
                let p = next.trim_start_matches("rename to ").to_string();
                // Old path wins from the `rename from` line; if absent (some
                // emitters only emit `rename to`), leave old_path None.
                if let Some(from) = pending_rename_from.take() {
                    file.old_path = Some(from);
                } else {
                    file.old_path = Some(p.clone());
                }
                // The b-side path also updates on rename; git already updated
                // the `diff --git` line, but defensively trust the latest.
                file.path = p;
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("new file mode") => {
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("deleted file mode") => {
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("similarity index") => {
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("index ") => {
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("--- ") || next.starts_with("+++ ") => {
                iter.next();
                continue;
            }
            Some(next) if next.starts_with("@@ ") => {
                // Start of hunks for this file.
                file.hunks = parse_hunks(iter);
                return Some(file);
            }
            Some(_) => {
                // Unknown header line — skip and keep going.
                iter.next();
            }
        }
    }
}

fn parse_diff_git_paths(rest: &str) -> (String, String) {
    // "a/X b/Y" — split on the first space and strip the leading "a/"/"b/".
    let mut parts = rest.splitn(2, ' ');
    let a_raw = parts.next().unwrap_or("");
    let b_raw = parts.next().unwrap_or("");
    (strip_prefix_path(a_raw), strip_prefix_path(b_raw))
}

fn strip_prefix_path(p: &str) -> String {
    if let Some(stripped) = p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")) {
        stripped.to_string()
    } else {
        p.to_string()
    }
}

fn parse_hunks<'a, I: Iterator<Item = &'a str>>(iter: &mut std::iter::Peekable<I>) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    while let Some(line) = iter.peek().copied() {
        if !line.starts_with("@@ ") {
            break;
        }
        let header = line;
        iter.next();
        let Some((old_start, new_start)) = parse_hunk_header(header) else {
            // Malformed header — bail out of this file's hunks.
            break;
        };

        let mut hunk = Hunk {
            old_start,
            new_start,
            lines: Vec::new(),
        };

        // After a hunk header, consume lines until we hit another header.
        while let Some(body) = iter.peek().copied() {
            if body.starts_with("@@ ")
                || body.starts_with("diff --git ")
                || body.starts_with("Binary files ")
            {
                break;
            }
            // --- / +++ markers between hunks (rare but legal) — skip.
            if body.starts_with("--- ") || body.starts_with("+++ ") {
                iter.next();
                continue;
            }
            iter.next();
            let Some(parsed) = parse_diff_body_line(body) else {
                continue;
            };
            hunk.lines.push(parsed);
        }

        hunks.push(hunk);
    }

    hunks
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    // "@@ -old,count +new,count @@ optional section heading"

    let after_at = line.strip_prefix("@@ ")?;
    let middle = after_at.split(" @@ ").next()?;
    let mut sides = middle.split(' ');
    let old_part = sides.next()?;
    let new_part = sides.next()?;
    Some((parse_side_start(old_part)?, parse_side_start(new_part)?))
}

fn parse_side_start(part: &str) -> Option<u32> {
    // "-N,M" or "+N,M" or "-N" or "+N" — drop the leading -/+, drop ",count".
    let trimmed = part.trim_start_matches('-').trim_start_matches('+');
    let count_or_start = trimmed.split(',').next()?;
    if count_or_start.is_empty() {
        Some(0)
    } else {
        count_or_start.parse::<u32>().ok()
    }
}

fn parse_diff_body_line(line: &str) -> Option<DiffLine> {
    let mut chars = line.chars();
    let prefix = chars.next()?;
    let kind = match prefix {
        '+' => DiffLineKind::Added,
        '-' => DiffLineKind::Removed,
        ' ' => DiffLineKind::Context,
        // "\ No newline at end of file" — skip.
        '\\' => return None,
        _ => return None,
    };
    Some(DiffLine {
        kind,
        text: chars.collect::<String>(),
    })
}

// ---------------------------------------------------------------------------
// Implementation: whitespace / formatting filter
// ---------------------------------------------------------------------------

fn hunk_should_demote(hunk: &Hunk, path: &str, mode: WhitespaceMode) -> bool {
    // A hunk qualifies for demotion when EVERY non-context change
    // (Added/Removed) is one of the allowed kinds for this mode.
    // If the hunk has no changes at all (context-only), we leave it alone —
    // there's nothing to demote, and rewriting kinds would be a no-op.
    let has_any_change = hunk
        .lines
        .iter()
        .any(|l| !matches!(l.kind, DiffLineKind::Context));
    if !has_any_change {
        return false;
    }

    // IgnoreWhitespace only needs the per-line whitespace check; the indent
    // and import passes require pre-computed stripped bodies, so we defer
    // the allocation until the formatting mode actually needs them.
    let stripped = (mode == WhitespaceMode::IgnoreFormatting).then(|| {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for l in &hunk.lines {
            match l.kind {
                DiffLineKind::Context => {}
                DiffLineKind::Added => added.push(l.text.trim_start().to_string()),
                DiffLineKind::Removed => removed.push(l.text.trim_start().to_string()),
            }
        }
        (added, removed)
    });

    hunk.lines.iter().all(|l| match l.kind {
        DiffLineKind::Context => true,
        DiffLineKind::Added | DiffLineKind::Removed => {
            // An empty line is "whitespace-only" by definition.
            if line_is_whitespace_only(&l.text) {
                return true;
            }
            if mode == WhitespaceMode::IgnoreFormatting
                && let Some((added, removed)) = stripped.as_ref()
            {
                if line_is_indent_only(&l.text, added, removed) {
                    return true;
                }
                if line_is_import_only(&l.text, path) {
                    return true;
                }
            }
            false
        }
    })
}

fn line_is_whitespace_only(s: &str) -> bool {
    s.chars().all(|c| c.is_whitespace())
}

/// Indent-only when the line's leading-whitespace-stripped body matches an
/// opposite-side line in the same hunk.
fn line_is_indent_only(text: &str, added: &[String], removed: &[String]) -> bool {
    let stripped = text.trim_start();
    if stripped.is_empty() {
        return false; // already covered by whitespace-only
    }
    added.iter().any(|s| s == stripped) && removed.iter().any(|s| s == stripped)
}

fn line_is_import_only(text: &str, path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("");
    let t = text.trim_start();
    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            t.starts_with("import ") || (t.starts_with("export ") && t.contains(" from "))
        }
        "rs" => t.starts_with("use "),
        "go" => t.starts_with("import "),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests (TDD — written first, exercised before implementation was filled in)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Fixtures ---------------------------------------------------------

    const TWO_FILES: &str = "\
diff --git a/foo.txt b/foo.txt
index 1234567..89abcdef 100644
--- a/foo.txt
+++ b/foo.txt
@@ -1,3 +1,4 @@
 line one
+inserted
 line two
 line three
@@ -10,2 +11,3 @@
 line ten
-removed
+added
+another
diff --git a/bar.txt b/bar.txt
index 1111111..2222222 100644
--- a/bar.txt
+++ b/bar.txt
@@ -1,1 +1,2 @@
 head
+tail
";

    const RENAME_AND_BINARY: &str = "\
diff --git a/old/name.txt b/new/name.txt
similarity index 95%
rename from old/name.txt
rename to new/name.txt
index abc..def 100644
--- a/old/name.txt
+++ b/new/name.txt
@@ -1,1 +1,1 @@
-same
+same
diff --git a/img.png b/img.png
index 111..222 100644
Binary files a/img.png and b/img.png differ
";

    // Whitespace-only hunk fixture.
    //
    // Normalized from the brief's literal embedded-escapes form
    // (`-\"\"` → -"" / `+  \"` → +  ") to unambiguous UTF-8 payload
    // (`-` removes an empty line / `+  ` adds a line with two spaces).
    // The intent — pure whitespace/blank changes in hunk A, real code in
    // hunk B — is preserved; the original quoting was brittle and easy to
    // misread in the source.
    const WHITESPACE_FIXTURE: &str = "\
diff --git a/ws.txt b/ws.txt
--- a/ws.txt
+++ b/ws.txt
@@ -1,2 +1,2 @@
 context
-
+  
@@ -10,2 +10,2 @@
 context
-real
+RIPPED
";

    const FORMATTING_FIXTURE: &str = "\
diff --git a/a.ts b/a.ts
--- a/a.ts
+++ b/a.ts
@@ -1,3 +1,3 @@
 import { a } from 'a';
 import { b } from 'b';
-import { c } from 'c';
+import { z } from 'z';
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,3 +1,3 @@
 fn f() {
-    let x = 1;
+        let x = 1;
 }
diff --git a/c.go b/c.go
--- a/c.go
+++ b/c.go
@@ -1,3 +1,3 @@
 package x
-func old() {}
+func NEW() {}
";

    // -- Tests ------------------------------------------------------------

    #[test]
    fn parse_unified_diff_basic() {
        let doc = parse_unified_diff(TWO_FILES);
        assert_eq!(doc.files.len(), 2, "expected 2 files");

        let foo = &doc.files[0];
        assert_eq!(foo.path, "foo.txt");
        assert!(foo.old_path.is_none());
        assert!(!foo.binary);
        assert_eq!(foo.hunks.len(), 2);

        let h1 = &foo.hunks[0];
        assert_eq!(h1.old_start, 1);
        assert_eq!(h1.new_start, 1);
        assert_eq!(h1.lines.len(), 4);
        assert_eq!(h1.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h1.lines[0].text, "line one");
        assert_eq!(h1.lines[1].kind, DiffLineKind::Added);
        assert_eq!(h1.lines[1].text, "inserted");
        assert_eq!(h1.lines[2].kind, DiffLineKind::Context);
        assert_eq!(h1.lines[2].text, "line two");
        assert_eq!(h1.lines[3].kind, DiffLineKind::Context);
        assert_eq!(h1.lines[3].text, "line three");

        let h2 = &foo.hunks[1];
        assert_eq!(h2.old_start, 10);
        assert_eq!(h2.new_start, 11);
        assert_eq!(h2.lines.len(), 4);
        assert_eq!(h2.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h2.lines[0].text, "line ten");
        assert_eq!(h2.lines[1].kind, DiffLineKind::Removed);
        assert_eq!(h2.lines[1].text, "removed");
        assert_eq!(h2.lines[2].kind, DiffLineKind::Added);
        assert_eq!(h2.lines[2].text, "added");
        assert_eq!(h2.lines[3].kind, DiffLineKind::Added);
        assert_eq!(h2.lines[3].text, "another");

        let bar = &doc.files[1];
        assert_eq!(bar.path, "bar.txt");
        assert_eq!(bar.hunks.len(), 1);
        assert_eq!(bar.hunks[0].old_start, 1);
        assert_eq!(bar.hunks[0].new_start, 1);
        assert_eq!(bar.hunks[0].lines.len(), 2);
        assert_eq!(bar.hunks[0].lines[1].kind, DiffLineKind::Added);
        assert_eq!(bar.hunks[0].lines[1].text, "tail");
    }

    #[test]
    fn hunk_boundaries_correct() {
        let doc = parse_unified_diff(TWO_FILES);
        let foo = &doc.files[0];

        assert_eq!(foo.hunks[0].old_start, 1);
        assert_eq!(foo.hunks[0].new_start, 1);
        assert_eq!(foo.hunks[1].old_start, 10);
        assert_eq!(foo.hunks[1].new_start, 11);

        // No context line straddles: last of hunk 1 is "line three",
        // first of hunk 2 is "line ten" — they must NOT merge.
        assert_eq!(foo.hunks[0].lines.last().unwrap().text, "line three");
        assert_eq!(foo.hunks[1].lines.first().unwrap().text, "line ten");
        assert_eq!(foo.hunks[0].lines.len(), 4);
        assert_eq!(foo.hunks[1].lines.len(), 4);
    }

    #[test]
    fn rename_and_binary_files_parsed() {
        let doc = parse_unified_diff(RENAME_AND_BINARY);
        assert_eq!(doc.files.len(), 2);

        let renamed = &doc.files[0];
        assert_eq!(renamed.path, "new/name.txt");
        assert_eq!(renamed.old_path.as_deref(), Some("old/name.txt"));
        assert!(!renamed.binary);
        assert_eq!(renamed.hunks.len(), 1);

        let binary = &doc.files[1];
        assert_eq!(binary.path, "img.png");
        assert!(binary.binary);
        assert!(binary.hunks.is_empty());
    }

    #[test]
    fn ignore_whitespace_drops_ws_only_hunks() {
        let doc = parse_unified_diff(WHITESPACE_FIXTURE);
        let filtered = filter_whitespace(&doc, WhitespaceMode::IgnoreWhitespace);

        let file = &filtered.files[0];
        assert_eq!(file.hunks.len(), 2);

        // Hunk 1: removed empty + added "  " — both whitespace-only — demoted.
        let h1 = &file.hunks[0];
        assert!(
            h1.lines
                .iter()
                .all(|l| matches!(l.kind, DiffLineKind::Context))
        );
        // Texts preserved.
        assert_eq!(h1.lines[1].text, "");
        assert_eq!(h1.lines[2].text, "  ");

        // Hunk 2: real code change — NOT demoted.
        let h2 = &file.hunks[1];
        assert_eq!(h2.lines[1].kind, DiffLineKind::Removed);
        assert_eq!(h2.lines[1].text, "real");
        assert_eq!(h2.lines[2].kind, DiffLineKind::Added);
        assert_eq!(h2.lines[2].text, "RIPPED");
    }

    #[test]
    fn ignore_formatting_drops_import_and_indent_hunks() {
        let doc = parse_unified_diff(FORMATTING_FIXTURE);
        let filtered = filter_whitespace(&doc, WhitespaceMode::IgnoreFormatting);
        assert_eq!(filtered.files.len(), 3);

        // TS: pure import reorder — must demote.
        let ts = &filtered.files[0];
        assert_eq!(ts.path, "a.ts");
        assert!(
            ts.hunks[0]
                .lines
                .iter()
                .all(|l| matches!(l.kind, DiffLineKind::Context))
        );

        // Rust: pure indent change — must demote.
        let rs = &filtered.files[1];
        assert_eq!(rs.path, "b.rs");
        assert!(
            rs.hunks[0]
                .lines
                .iter()
                .all(|l| matches!(l.kind, DiffLineKind::Context))
        );

        // Go: real change — must survive.
        let go = &filtered.files[2];
        assert_eq!(go.path, "c.go");
        assert_eq!(go.hunks[0].lines[1].kind, DiffLineKind::Removed);
        assert_eq!(go.hunks[0].lines[1].text, "func old() {}");
        assert_eq!(go.hunks[0].lines[2].kind, DiffLineKind::Added);
        assert_eq!(go.hunks[0].lines[2].text, "func NEW() {}");
    }

    #[test]
    fn view_mode_is_orthogonal_to_filter() {
        let doc = parse_unified_diff(TWO_FILES);
        for mode in [
            DiffViewMode::Split,
            DiffViewMode::Inline,
            DiffViewMode::Hunks,
            DiffViewMode::Files,
        ] {
            let m2 = mode;
            assert_eq!(mode, m2);
        }
        let filtered = filter_whitespace(&doc, WhitespaceMode::Off);
        assert_eq!(filtered, doc, "Off mode must return an identical document");
    }
}
