//! Re-number a line-level diff between two file versions into a compact
//! current-file preview.
//!
//! Removed lines are omitted from the preview; added and kept (context) lines
//! are anchored to their post-edit positions so a follow-up edit can reuse
//! visible concrete lines directly. Long contiguous added runs are summarized
//! with a `…` marker instead of echoing every inserted line, and long runs of
//! unchanged lines are trimmed to a configurable context window around each
//! change.
//!
//! Ported from omp `packages/hashline/src/diff-preview.ts`, adapted to take the
//! before/after text directly (the crate's [`CompactDiffPreview`] type) rather
//! than a pre-formatted `<sign><lineNum>|<content>` diff string.

use crate::types::{CompactDiffOptions, CompactDiffPreview};

/// Marker substituted for long elided added runs (and for any literal `...` /
/// `+…` content lines, matching omp).
const PREVIEW_ELISION_MARKER: &str = "…";
/// Blank row separating non-contiguous regions of a numbered diff.
const PREVIEW_GAP_ROW: &str = "";

/// `true` for separator lines (elision marker or blank gap row).
fn is_preview_separator(line: &str) -> bool {
    line == PREVIEW_ELISION_MARKER || line == PREVIEW_GAP_ROW
}

/// Normalize omp's raw elision spellings (`...`, `…`, `+…`) to the single
/// marker, then append with separator de-stacking: separators never stack
/// (removed lines between two separators would otherwise leave them adjacent),
/// and a leading separator is dropped outright.
fn append_preview_line(output: &mut Vec<String>, line: &str) {
    let normalized: &str = match line {
        "..." | "…" | "+…" => PREVIEW_ELISION_MARKER,
        _ => line,
    };
    if is_preview_separator(normalized)
        && (output.is_empty()
            || output
                .last()
                .map(|l| is_preview_separator(l))
                .unwrap_or(false))
    {
        return;
    }
    output.push(normalized.to_string());
}

/// Append an accumulated added run, collapsing it to its edges + an elision
/// marker when it is longer than `edge * 2 + 1` lines.
fn append_added_run(output: &mut Vec<String>, run: &[String], edge: usize) {
    if run.is_empty() {
        return;
    }
    let edge = edge.max(1);
    let collapse_threshold = edge * 2 + 1;
    if run.len() <= collapse_threshold {
        for text in run {
            append_preview_line(output, text);
        }
        return;
    }
    for text in &run[..edge] {
        append_preview_line(output, text);
    }
    append_preview_line(output, PREVIEW_ELISION_MARKER);
    for text in &run[run.len() - edge..] {
        append_preview_line(output, text);
    }
}

/// Flush a pending added run into `output`.
fn flush(output: &mut Vec<String>, run: &mut Vec<String>, edge: usize) {
    append_added_run(output, run, edge);
    run.clear();
}

/// A single LCS step.
#[derive(Debug)]
enum Op {
    /// Line present in both versions.
    Keep { post: u32, content: String },
    /// Line removed from the before version.
    Remove,
    /// Line inserted in the after version.
    Insert { post: u32, content: String },
}

impl Op {
    fn is_keep(&self) -> bool {
        matches!(self, Op::Keep { .. })
    }
}

/// Produce the LCS-based edit script between `before` and `after`, with each
/// keep/insert op tagged by its 1-indexed post-edit line number.
fn diff_ops(before: &[&str], after: &[&str]) -> Vec<Op> {
    let m = before.len();
    let n = after.len();

    // dp[i][j] = LCS length of before[i..] and after[j..].
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if before[i] == after[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut ops = Vec::with_capacity(m + n);
    let (mut i, mut j, mut post) = (0usize, 0usize, 1u32);
    while i < m && j < n {
        if before[i] == after[j] {
            ops.push(Op::Keep {
                post,
                content: before[i].to_string(),
            });
            i += 1;
            j += 1;
            post += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Op::Remove);
            i += 1;
        } else {
            ops.push(Op::Insert {
                post,
                content: after[j].to_string(),
            });
            j += 1;
            post += 1;
        }
    }
    while i < m {
        ops.push(Op::Remove);
        i += 1;
    }
    while j < n {
        ops.push(Op::Insert {
            post,
            content: after[j].to_string(),
        });
        j += 1;
        post += 1;
    }
    ops
}

/// Build a compact before/after diff preview. Added and context (unchanged)
/// lines are numbered at their post-edit positions; unchanged lines far from any
/// change are trimmed to [`CompactDiffOptions::max_unchanged_context`] lines on
/// each side; long added runs are collapsed with a `…` marker.
pub fn build_compact_diff_preview(
    before: &str,
    after: &str,
    opts: &CompactDiffOptions,
) -> CompactDiffPreview {
    let ctx = opts.max_unchanged_context.max(1);
    let before_lines: Vec<&str> = before.split('\n').collect();
    let after_lines: Vec<&str> = after.split('\n').collect();
    let ops = diff_ops(&before_lines, &after_lines);

    // Decide which keep ops survive trimming: keep up to `ctx` unchanged lines
    // on each side of every change (skipping over adjacent changes).
    let n = ops.len();
    let mut keep_visible = vec![false; n];
    for center in 0..n {
        if ops[center].is_keep() {
            continue;
        }
        let mut c = 0usize;
        let mut k = center;
        while k > 0 && c < ctx {
            k -= 1;
            if ops[k].is_keep() {
                keep_visible[k] = true;
                c += 1;
            }
        }
        c = 0;
        let mut k = center + 1;
        while k < n && c < ctx {
            if ops[k].is_keep() {
                keep_visible[k] = true;
                c += 1;
            }
            k += 1;
        }
    }

    let mut output: Vec<String> = Vec::new();
    let mut added_run: Vec<String> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op {
            Op::Keep { post, content } => {
                if !keep_visible[idx] {
                    continue;
                }
                flush(&mut output, &mut added_run, ctx);
                append_preview_line(&mut output, &format!("{post}:{content}"));
            }
            Op::Remove => {
                flush(&mut output, &mut added_run, ctx);
            }
            Op::Insert { post, content } => {
                added_run.push(format!("{post}:{content}"));
            }
        }
    }
    flush(&mut output, &mut added_run, ctx);

    // Strip trailing separators.
    while output
        .last()
        .map(|l| is_preview_separator(l))
        .unwrap_or(false)
    {
        output.pop();
    }

    CompactDiffPreview { lines: output }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(before: &str, after: &str) -> Vec<String> {
        build_compact_diff_preview(before, after, &CompactDiffOptions::default()).lines
    }

    #[test]
    fn identical_input_is_empty() {
        assert!(preview("a\nb\nc", "a\nb\nc").is_empty());
    }

    #[test]
    fn pure_insert_numbers_at_post_edit_positions() {
        let lines = preview("a\nb\nc", "x\na\nb\nc");
        assert!(lines.iter().any(|l| l == "1:x"), "insert x at post line 1");
        // The inserted line is adjacent to kept context.
        assert!(lines.iter().any(|l| l.starts_with("2:a")));
    }

    #[test]
    fn replace_shows_insert_and_surrounding_context() {
        let lines = preview("a\nb\nc", "a\nB\nc");
        // `b` is gone; `B` inserted at post line 2.
        assert!(lines.iter().any(|l| l == "2:B"));
        assert!(lines.iter().any(|l| l == "1:a"));
        assert!(lines.iter().any(|l| l == "3:c"));
        // The removed `b` never appears as content.
        assert!(!lines.iter().any(|l| l.ends_with(":b")));
    }

    #[test]
    fn trailing_insert_is_emitted() {
        let lines = preview("a", "a\nb");
        assert!(lines.iter().any(|l| l == "2:b"));
    }

    #[test]
    fn long_added_run_collapses_with_marker() {
        // 10 inserted lines with default ctx=3 -> edge=3, threshold=7 -> first 3 + … + last 3.
        let after: Vec<&str> = (0..10).map(|_| "x").collect();
        let lines = preview("", &after.join("\n"));
        assert!(lines.iter().any(|l| l == "…"), "elision marker present");
        // No more than 2*edge + 1 content lines.
        assert!(lines.len() <= 7);
        // First and last inserted lines appear.
        assert!(lines.first().map(|l| l == "1:x").unwrap_or(false));
    }

    #[test]
    fn unchanged_lines_far_from_change_are_trimmed() {
        // A change at line 1; the long tail must be trimmed away.
        let before: String = "head\n".repeat(1) + &"tail\n".repeat(50);
        let after: String = "CHANGED\n".to_string() + &"tail\n".repeat(50);
        let lines = preview(&before, &after);
        assert!(lines.iter().any(|l| l == "1:CHANGED"));
        // The preview is far smaller than 51 lines.
        assert!(lines.len() < 20, "trimmed: got {} lines", lines.len());
    }

    #[test]
    fn context_window_respects_option() {
        // ctx=1: only the immediately adjacent kept line on each side.
        let opts = CompactDiffOptions {
            max_unchanged_context: 1,
        };
        let lines = build_compact_diff_preview("a\nb\nc\nd\ne", "a\nb\nX\nd\ne", &opts).lines;
        // Change at position 3 (c -> X); ctx=1 keeps b (pre) and d (post).
        assert!(lines.iter().any(|l| l == "2:b"));
        assert!(lines.iter().any(|l| l == "3:X"));
        assert!(lines.iter().any(|l| l == "4:d"));
        // `a` and `e` are outside the 1-line window.
        assert!(!lines.iter().any(|l| l == "1:a"));
        assert!(!lines.iter().any(|l| l == "5:e"));
    }

    #[test]
    fn options_min_clamps_to_one() {
        let opts = CompactDiffOptions {
            max_unchanged_context: 0,
        };
        // ctx=0 must be clamped to >=1 and still produce output, not panic.
        let lines = build_compact_diff_preview("a\nb", "a\nB", &opts).lines;
        assert!(lines.iter().any(|l| l == "2:B"));
    }

    #[test]
    fn elision_marker_is_unique_and_interior() {
        // A long added run collapses to a single interior … marker, never
        // stacked and never left leading/trailing.
        let after: Vec<&str> = (0..10).map(|_| "x").collect();
        let lines = preview("", &after.join("\n"));
        let markers: Vec<&String> = lines.iter().filter(|l| **l == "…").collect();
        assert_eq!(
            markers.len(),
            1,
            "exactly one elision marker, never stacked"
        );
        assert_ne!(
            lines.first().map(String::as_str),
            Some("…"),
            "no leading marker"
        );
        assert_ne!(
            lines.last().map(String::as_str),
            Some("…"),
            "no trailing marker"
        );
    }
}
