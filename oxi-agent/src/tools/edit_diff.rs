//! Edit diff computation engine
//!
//! Computes unified diffs for edit previews. Supports:
//! - Multiple non-overlapping edits in one call
//! - Line ending normalization (CRLF → LF)
//! - BOM detection and preservation
//! - Fuzzy matching fallback

use std::fmt;

/// A single edit operation
#[derive(Debug, Clone)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// Result of computing diffs for a set of edits
#[derive(Debug, Clone)]
pub struct EditDiffResult {
    /// The unified diff string
    pub diff: String,
    /// Line number of the first change in the new file (for editor navigation)
    pub first_changed_line: Option<usize>,
}

/// Error during diff computation
#[derive(Debug, Clone)]
pub struct EditDiffError {
    pub message: String,
}

impl fmt::Display for EditDiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Detect line ending type of content
pub fn detect_line_ending(content: &str) -> LineEnding {
    if content.contains("\r\n") {
        LineEnding::Crlf
    } else if content.contains('\r') {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

/// Line ending type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
    Cr,
}

/// Normalize content to LF line endings for diff computation
pub fn normalize_to_lf(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

/// Restore original line endings
pub fn restore_line_endings(content: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Crlf => content.replace('\n', "\r\n"),
        LineEnding::Cr => content.replace('\n', "\r"),
        LineEnding::Lf => content.to_string(),
    }
}

/// Strip UTF-8 BOM from content
pub fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

/// Check if content starts with BOM
pub fn has_bom(content: &str) -> bool {
    content.starts_with('\u{feff}')
}

/// Apply multiple edits to normalized (LF) content.
/// Validates that edits don't overlap and all find matches.
pub fn apply_edits_to_normalized_content(
    content: &str,
    edits: &[Edit],
) -> Result<String, EditDiffError> {
    if edits.is_empty() {
        return Ok(content.to_string());
    }

    // Find all match positions first
    let mut matches: Vec<(usize, usize, &Edit)> = Vec::new(); // (start, end, edit)

    for edit in edits {
        let start = content.find(&edit.old_text).ok_or_else(|| EditDiffError {
            message: format!(
                "Text to replace not found in file. Make sure to match the exact text including whitespace and newlines."
            ),
        })?;
        let end = start + edit.old_text.len();

        // Check for overlaps with existing matches
        for &(existing_start, existing_end, _) in &matches {
            if start < existing_end && end > existing_start {
                return Err(EditDiffError {
                    message: "Edits overlap — merge nearby edits into one.".to_string(),
                });
            }
        }

        matches.push((start, end, edit));
    }

    // Sort by position (reverse order for safe replacement)
    matches.sort_by(|a, b| b.0.cmp(&a.0));

    let mut result = content.to_string();
    for (start, end, edit) in matches {
        result.replace_range(start..end, &edit.new_text);
    }

    Ok(result)
}

/// Compute a unified diff between original and modified content.
/// Returns the diff string and the first changed line number.
pub fn compute_edits_diff(original: &str, modified: &str, context_lines: usize) -> EditDiffResult {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mod_lines: Vec<&str> = modified.lines().collect();

    let mut diff = String::new();
    let mut first_changed_line: Option<usize> = None;

    // Simple line-by-line diff using longest common subsequence approach
    let lcs = compute_lcs_table(&orig_lines, &mod_lines);
    let mut diff_ops = Vec::new();
    build_diff_ops(
        &lcs,
        &orig_lines,
        &mod_lines,
        orig_lines.len(),
        mod_lines.len(),
        &mut diff_ops,
    );

    // Group into hunks with context
    let hunks = group_into_hunks(&diff_ops, &orig_lines, &mod_lines, context_lines);

    for (i, hunk) in hunks.iter().enumerate() {
        if i > 0 {
            diff.push('\n');
        }

        if first_changed_line.is_none() {
            first_changed_line = Some(hunk.new_start);
        }

        diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start + 1,
            hunk.old_count,
            hunk.new_start + 1,
            hunk.new_count,
        ));

        for line in &hunk.lines {
            match line {
                DiffLine::Context(s) => diff.push_str(&format!(" {}\n", s)),
                DiffLine::Remove(s) => diff.push_str(&format!("-{}\n", s)),
                DiffLine::Add(s) => diff.push_str(&format!("+{}\n", s)),
            }
        }
    }

    EditDiffResult {
        diff,
        first_changed_line,
    }
}

/// Generate a diff string from a set of edits applied to content
pub fn generate_diff_string(
    content: &str,
    edits: &[Edit],
    context_lines: usize,
) -> Result<EditDiffResult, EditDiffError> {
    let normalized = normalize_to_lf(strip_bom(content));
    let modified = apply_edits_to_normalized_content(&normalized, edits)?;
    Ok(compute_edits_diff(&normalized, &modified, context_lines))
}

// LCS table computation for diff
fn compute_lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    dp
}

#[derive(Debug, Clone)]
enum DiffOp {
    Equal(usize, usize), // (old_idx, new_idx)
    Remove(usize),       // old_idx
    Add(usize),          // new_idx
}

fn build_diff_ops(
    dp: &[Vec<usize>],
    a: &[&str],
    b: &[&str],
    i: usize,
    j: usize,
    ops: &mut Vec<DiffOp>,
) {
    if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
        build_diff_ops(dp, a, b, i - 1, j - 1, ops);
        ops.push(DiffOp::Equal(i - 1, j - 1));
    } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
        build_diff_ops(dp, a, b, i, j - 1, ops);
        ops.push(DiffOp::Add(j - 1));
    } else if i > 0 {
        build_diff_ops(dp, a, b, i - 1, j, ops);
        ops.push(DiffOp::Remove(i - 1));
    }
}

#[derive(Debug)]
enum DiffLine<'a> {
    Context(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

struct Hunk<'a> {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<DiffLine<'a>>,
}

fn group_into_hunks<'a>(
    ops: &[DiffOp],
    old_lines: &[&'a str],
    new_lines: &[&'a str],
    context: usize,
) -> Vec<Hunk<'a>> {
    // Find change boundaries
    let mut changes: Vec<usize> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op {
            DiffOp::Remove(_) | DiffOp::Add(_) => changes.push(i),
            DiffOp::Equal(_, _) => {}
        }
    }

    if changes.is_empty() {
        return Vec::new();
    }

    // Group changes into hunks
    let mut hunks: Vec<Hunk<'a>> = Vec::new();
    let mut hunk_start = changes[0];
    let mut hunk_end = changes[0];

    for &change_idx in &changes[1..] {
        if change_idx <= hunk_end + 2 * context {
            // Within context range, extend current hunk
            hunk_end = change_idx;
        } else {
            // Start a new hunk
            hunks.push(build_hunk(
                ops, old_lines, new_lines, hunk_start, hunk_end, context,
            ));
            hunk_start = change_idx;
            hunk_end = change_idx;
        }
    }
    hunks.push(build_hunk(
        ops, old_lines, new_lines, hunk_start, hunk_end, context,
    ));

    hunks
}

fn build_hunk<'a>(
    ops: &[DiffOp],
    old_lines: &[&'a str],
    new_lines: &[&'a str],
    change_start: usize,
    change_end: usize,
    context: usize,
) -> Hunk<'a> {
    let start = change_start.saturating_sub(context);
    let end = (change_end + context + 1).min(ops.len());

    let mut lines = Vec::new();
    let mut _old_pos = usize::MAX;
    let mut _new_pos = usize::MAX;
    let mut old_count = 0;
    let mut new_count = 0;
    let mut first_old = None;
    let mut first_new = None;

    for i in start..end {
        match &ops[i] {
            DiffOp::Equal(oi, ni) => {
                if first_old.is_none() {
                    first_old = Some(*oi);
                }
                if first_new.is_none() {
                    first_new = Some(*ni);
                }
                _old_pos = *oi;
                _new_pos = *ni;
                lines.push(DiffLine::Context(old_lines[*oi]));
                old_count += 1;
                new_count += 1;
            }
            DiffOp::Remove(oi) => {
                if first_old.is_none() {
                    first_old = Some(*oi);
                }
                _old_pos = *oi;
                lines.push(DiffLine::Remove(old_lines[*oi]));
                old_count += 1;
            }
            DiffOp::Add(ni) => {
                if first_new.is_none() {
                    first_new = Some(*ni);
                }
                _new_pos = *ni;
                lines.push(DiffLine::Add(new_lines[*ni]));
                new_count += 1;
            }
        }
    }

    Hunk {
        old_start: first_old.unwrap_or(0),
        old_count,
        new_start: first_new.unwrap_or(0),
        new_count,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_to_lf() {
        assert_eq!(normalize_to_lf("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(normalize_to_lf("a\nb\n"), "a\nb\n");
    }

    #[test]
    fn test_detect_line_ending() {
        assert_eq!(detect_line_ending("a\r\nb"), LineEnding::Crlf);
        assert_eq!(detect_line_ending("a\nb"), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\rb"), LineEnding::Cr);
    }

    #[test]
    fn test_strip_bom() {
        assert_eq!(strip_bom("\u{feff}hello"), "hello");
        assert_eq!(strip_bom("hello"), "hello");
    }

    #[test]
    fn test_apply_edits_simple() {
        let content = "hello world\nfoo bar\n";
        let edits = vec![Edit {
            old_text: "hello world".to_string(),
            new_text: "hello Rust".to_string(),
        }];
        let result = apply_edits_to_normalized_content(content, &edits).unwrap();
        assert_eq!(result, "hello Rust\nfoo bar\n");
    }

    #[test]
    fn test_apply_multiple_edits() {
        let content = "aaa\nbbb\nccc\n";
        let edits = vec![
            Edit {
                old_text: "aaa".to_string(),
                new_text: "AAA".to_string(),
            },
            Edit {
                old_text: "ccc".to_string(),
                new_text: "CCC".to_string(),
            },
        ];
        let result = apply_edits_to_normalized_content(content, &edits).unwrap();
        assert_eq!(result, "AAA\nbbb\nCCC\n");
    }

    #[test]
    fn test_apply_overlapping_edits_fails() {
        let content = "aaa\nbbb\nccc\n";
        let edits = vec![
            Edit {
                old_text: "aaa\nbbb".to_string(),
                new_text: "AAA".to_string(),
            },
            Edit {
                old_text: "bbb\nccc".to_string(),
                new_text: "CCC".to_string(),
            },
        ];
        let result = apply_edits_to_normalized_content(content, &edits);
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_not_found_fails() {
        let content = "hello world";
        let edits = vec![Edit {
            old_text: "not found".to_string(),
            new_text: "replacement".to_string(),
        }];
        let result = apply_edits_to_normalized_content(content, &edits);
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_diff() {
        let original = "line1\nline2\nline3\n";
        let modified = "line1\nmodified\nline3\n";
        let result = compute_edits_diff(original, modified, 1);
        assert!(result.diff.contains("-line2"));
        assert!(result.diff.contains("+modified"));
        assert_eq!(result.first_changed_line, Some(0)); // 0-indexed
    }

    #[test]
    fn test_generate_diff_string() {
        let content = "hello world\nfoo bar\n";
        let edits = vec![Edit {
            old_text: "hello world".to_string(),
            new_text: "hello Rust".to_string(),
        }];
        let result = generate_diff_string(content, &edits, 2).unwrap();
        assert!(result.diff.contains("-hello world"));
        assert!(result.diff.contains("+hello Rust"));
    }
}
