//! Centralized error/warning text for the hashline parser, applier, and patcher.
//!
//! Ported from omp `packages/hashline/src/messages.ts`. Message text is kept
//! close to omp's so behaviour and diagnostics line up.

use std::collections::{BTreeSet, HashSet};

use crate::format::{
    HL_FILE_HASH_SEP, HL_FILE_PREFIX, HL_FILE_SUFFIX, HL_RANGE_SEP, format_numbered_line,
};

/// Lines of context shown either side of a hash mismatch.
pub const MISMATCH_CONTEXT: usize = 2;

// ── Optional patch envelope markers ──────────────────────────────────────

/// Optional patch envelope start marker; silently consumed.
pub const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";

/// Optional patch envelope end marker; terminates parsing.
pub const END_PATCH_MARKER: &str = "*** End Patch";

/// Truncation sentinel emitted by an agent loop mid-call. Ends parsing like
/// [`END_PATCH_MARKER`], without a warning.
pub const ABORT_MARKER: &str = "*** Abort";

// ── Warning messages ─────────────────────────────────────────────────────

/// Two consecutive hunks targeted the exact same concrete range.
pub const REPLACE_PAIR_COALESCED_WARNING: &str = "Two hunks targeted the same range; kept only the second. One `SWAP N.=M:` hunk per range — the body is the final content, never old+new.";

/// Bare bodyless hunk followed by an overlapping concrete hunk.
pub const BARE_BODY_OVERLAPPED_WARNING: &str = "Dropped a bare hunk overlapped by the concrete hunk after it. One `SWAP N.=M:` hunk per range — the body is the final content, never old+new.";

/// Bare body rows auto-converted to literal `+` rows.
pub const BARE_BODY_AUTO_PIPED_WARNING: &str =
    "Auto-prefixed bare body row(s) with `+`. Body rows must be `+TEXT` literal lines.";

/// Unified-diff-style `-` row in a hunk body.
pub const MINUS_ROW_REJECTED: &str = "`-` rows are not valid; the range already names the lines being changed. For a literal `-` line, write `+-…`.";

/// Block-anchored edit reached a path with no block resolver wired in.
pub const BLOCK_RESOLVER_UNAVAILABLE: &str = "`SWAP.BLK`/`DEL.BLK`/`INS.BLK.POST` are not available here (no block resolver configured). Use a concrete line range.";

/// Internal invariant: an unresolved `SWAP.BLK` edit reached the applier.
pub const UNRESOLVED_BLOCK_INTERNAL: &str = "internal error: unresolved `SWAP.BLK` edit reached the applier (resolveBlockEdits was not run).";

/// `Recovery`: an external write matched a cached snapshot.
pub const RECOVERY_EXTERNAL_WARNING: &str = "Recovered from a stale file hash using a previous read snapshot (file changed externally between read and edit).";

/// `Recovery`: a prior in-session edit advanced the hash.
pub const RECOVERY_SESSION_CHAIN_WARNING: &str = "Recovered from a stale file hash using an earlier in-session snapshot (a prior edit in this session advanced the hash).";

/// `Recovery`: session-chain replay fast-path (verify hedge).
pub const RECOVERY_SESSION_REPLAY_WARNING: &str = "Recovered by replaying your edits onto the current file content (a prior in-session edit changed the lines you re-targeted with a stale hash). Verify the diff matches your intent.";

/// `INS.HEAD:`/`INS.TAIL:` applied despite a stale snapshot tag.
pub const HEADTAIL_DRIFT_WARNING: &str = "Applied the `INS.HEAD:`/`INS.TAIL:` edit despite a stale snapshot tag (file changed since your read) — head/tail position is content-independent. Re-read if the drift was unexpected.";

// ── Error messages ───────────────────────────────────────────────────────

/// Replace hunk with no body.
pub const EMPTY_REPLACE: &str =
    "`SWAP N.=M:` needs at least one `+TEXT` body row. To delete lines, use `DEL N.=M`.";

/// `SWAP.BLK N:` hunk with no body.
pub const EMPTY_BLOCK: &str =
    "`SWAP.BLK N:` needs at least one `+TEXT` body row. To delete a block, use `DEL.BLK N`.";

/// Delete hunk received a body row.
pub const DELETE_TAKES_NO_BODY: &str =
    "`DEL N.=M` does not take body rows. Remove the body, or use `SWAP N.=M:`.";

/// `DEL.BLK N` hunk received a body row.
pub const DELETE_BLOCK_TAKES_NO_BODY: &str =
    "`DEL.BLK N` does not take body rows. Remove the body, or use `SWAP.BLK N:`.";

/// Insert hunk with no body.
pub const EMPTY_INSERT: &str = "`INS` needs at least one `+TEXT` body row.";

// ── Op kinds for block messages ──────────────────────────────────────────

/// Op kind of a deferred block edit, for [`block_unresolved_message`] and
/// [`block_single_line_message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOpKind {
    /// Block replacement (`SWAP.BLK`).
    Replace,
    /// Block deletion (`DEL.BLK`).
    Delete,
    /// Insert immediately after a block (`INS.BLK.POST`).
    InsertAfter,
}

// ── Message-builder functions ────────────────────────────────────────────

/// Numbered `LINE:TEXT` rows around `anchor_lines` (± [`MISMATCH_CONTEXT`]),
/// `*`-marking anchors, `...` between non-adjacent runs. Out-of-range anchors
/// contribute no rows.
pub fn format_anchored_context(anchor_lines: &[u32], file_lines: &[String]) -> Vec<String> {
    let len = file_lines.len() as u32;
    let mut display: BTreeSet<u32> = BTreeSet::new();
    for &line in anchor_lines {
        if line < 1 || line > len {
            continue;
        }
        let lo = (line as usize).saturating_sub(MISMATCH_CONTEXT).max(1) as u32;
        let hi = line + MISMATCH_CONTEXT as u32;
        let hi = hi.min(len);
        for n in lo..=hi {
            display.insert(n);
        }
    }

    let anchor_set: HashSet<u32> = anchor_lines.iter().copied().collect();
    let mut rows: Vec<String> = Vec::new();
    let mut previous: i64 = -1;
    for &line_num in display.iter() {
        // BTreeSet iterates ascending; emit a gap when runs are non-adjacent.
        if previous != -1 && (line_num as i64) > previous + 1 {
            rows.push("...".to_string());
        }
        previous = line_num as i64;
        let marker = if anchor_set.contains(&line_num) {
            "*"
        } else {
            " "
        };
        let text = file_lines
            .get((line_num - 1) as usize)
            .map(String::as_str)
            .unwrap_or("");
        rows.push(format!("{marker}{}", format_numbered_line(line_num, text)));
    }
    rows
}

/// `SWAP.BLK`/`DEL.BLK` could not resolve to a syntactic block. Appends a
/// [`format_anchored_context`] preview when `file_lines` is given.
pub fn block_unresolved_message(
    line: u32,
    op: BlockOpKind,
    file_lines: Option<&[String]>,
) -> String {
    let is_delete = op == BlockOpKind::Delete;
    let phrase = if is_delete {
        format!("DEL.BLK {line}")
    } else {
        format!("SWAP.BLK {line}:")
    };
    let fallback = if is_delete {
        format!("DEL {line}{HL_RANGE_SEP}M")
    } else {
        format!("SWAP {line}{HL_RANGE_SEP}M:")
    };
    let mut message = format!(
        "`{phrase}` could not resolve a syntactic block beginning on line {line} \
         (unsupported language, blank/closer line, or parse error). Use `{fallback}` with explicit lines."
    );
    if let Some(lines) = file_lines {
        let context = format_anchored_context(&[line], lines);
        if !context.is_empty() {
            message.push_str("\n\n");
            message.push_str(&context.join("\n"));
        }
    }
    message
}

/// `INS.BLK.POST N:` anchored on a closing-delimiter line, lowered to plain
/// `INS.POST N:`.
pub fn insert_after_block_closer_lowered_warning(line: u32) -> String {
    format!(
        "`INS.BLK.POST {line}:` anchors on a closing delimiter, so it was applied as plain \
         `INS.POST {line}:`. Anchor on the line that OPENS the construct."
    )
}

/// `INS.BLK.POST N:` anchor unresolvable, lowered to plain `INS.POST N:`.
pub fn insert_after_block_unresolved_lowered_warning(line: u32) -> String {
    format!(
        "`INS.BLK.POST {line}:` could not resolve a syntactic block on line {line}, so it was \
         applied as plain `INS.POST {line}:`. Verify the landing line; anchor on a line that \
         OPENS a construct."
    )
}

/// `INS.POST N:` body indented shallower than the anchor: the landing slid
/// forward past trailing closer lines.
pub fn after_insert_landing_shift_warning(
    anchor_line: u32,
    landing_line: u32,
    crossed: u32,
) -> String {
    let plural = if crossed == 1 { "" } else { "s" };
    format!(
        "INS.POST {anchor_line}: body indented shallower than the anchor, so the landing moved \
         past {crossed} closing line{plural} to after line {landing_line}. For the deeper position \
         inside the block, re-issue with the body indented to match."
    )
}

/// `INS.BLK.POST N:` body indented deeper than the block's closer: the landing
/// was pulled inside the block.
pub fn block_insert_landing_shift_warning(
    block_start: u32,
    closer_line: u32,
    landing_line: u32,
) -> String {
    format!(
        "INS.BLK.POST {block_start}: body indented deeper than closing line {closer_line}, so it \
         was placed inside the block, after line {landing_line}. `INS.BLK.POST` lands AFTER the \
         block at sibling depth — if inside was intended, use plain `INS.POST {closer_line}:`."
    )
}

/// Section omitted the mandatory snapshot tag.
pub fn missing_snapshot_tag_message(section_path: &str) -> String {
    format!(
        "Missing hashline snapshot tag for {section_path}; use \
         `{pfx}{section_path}{sep}tag{sfx}` from your latest read/search output. To create a new \
         file, use the write tool.",
        pfx = HL_FILE_PREFIX,
        sep = HL_FILE_HASH_SEP,
        sfx = HL_FILE_SUFFIX,
    )
}

/// An anchored edit referenced lines the read that minted `tag` never displayed.
pub fn unseen_lines_message(section_path: &str, unseen_lines: &[u32], tag: &str) -> String {
    let ranges = format_line_ranges(unseen_lines);
    let selector = ranges.replace(", ", ",");
    format!(
        "This edit anchors to lines {ranges} of {section_path} that \
         {pfx}{section_path}{sep}{tag}{sfx} never displayed (it showed a partial range, a search \
         hit, or a folded summary). Re-read them in full first with a ranged read like \
         `{section_path}:{selector}` — it skips summarization and mints a fresh tag (a plain \
         re-read just re-folds them) — then re-issue the edit.",
        pfx = HL_FILE_PREFIX,
        sep = HL_FILE_HASH_SEP,
        sfx = HL_FILE_SUFFIX,
    )
}

/// A block-anchored op resolved to a single line — the plain op is unambiguous
/// for one line.
pub fn block_single_line_message(line: u32, op: BlockOpKind) -> String {
    let block_form = match op {
        BlockOpKind::InsertAfter => "INS.BLK.POST",
        BlockOpKind::Delete => "DEL.BLK",
        BlockOpKind::Replace => "SWAP.BLK",
    };
    let plain_form = match op {
        BlockOpKind::InsertAfter => format!("INS.POST {line}:"),
        BlockOpKind::Delete => format!("DEL {line}"),
        BlockOpKind::Replace => format!("SWAP {line}{HL_RANGE_SEP}{line}:"),
    };
    format!(
        "`{block_form} {line}` resolved a single-line block — line {line} is a bare statement, \
         not the opening line of a multi-line construct. For that one line use `{plain_form}`; \
         to act on an enclosing construct, anchor {block_form} on the line that OPENS it \
         (e.g. its `function`/`if`/`case` header), never a statement inside it."
    )
}

/// Format a comma-separated list of example anchors with an optional
/// line-number prefix, quoted for inclusion in error messages:
/// `"160", "42", "7"` (no prefix) or `"119", "112", "7"` (prefix `"119"`).
///
/// Ported from omp `format.ts:describeAnchorExamples`.
pub fn describe_anchor_examples(line_prefix: &str) -> String {
    let examples: Vec<String> = if line_prefix.is_empty() {
        ["160", "42", "7"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else {
        let stem = &line_prefix[..line_prefix.len().saturating_sub(1)];
        let stem = if stem.is_empty() { "4" } else { stem };
        vec![line_prefix.to_string(), format!("{stem}2"), "7".to_string()]
    };
    examples
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compress a line list into a sorted `1-4, 7, 10-12` range string.
fn format_line_ranges(lines: &[u32]) -> String {
    let mut sorted: Vec<u32> = lines.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut start = sorted[0];
    let mut prev = sorted[0];
    for &current in &sorted[1..] {
        if current == prev + 1 {
            prev = current;
            continue;
        }
        parts.push(run_range(start, prev));
        start = current;
        prev = current;
    }
    parts.push(run_range(start, prev));
    parts.join(", ")
}

/// Format a single run `[start..=prev]` as `start` or `start-prev`.
fn run_range(start: u32, prev: u32) -> String {
    if start == prev {
        start.to_string()
    } else {
        format!("{start}-{prev}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchored_context_marks_and_gaps() {
        let file: Vec<String> = (1..=10).map(|n| format!("L{n}")).collect();
        // Anchor at 3 and 8: windows [1..5] and [6..10], contiguous (5,6) so no gap.
        let rows = format_anchored_context(&[3, 8], &file);
        assert!(rows.iter().any(|r| r.starts_with("*3:L3")));
        assert!(rows.iter().any(|r| r.starts_with("*8:L8")));
        assert!(
            !rows.iter().any(|r| r == "..."),
            "adjacent windows should not produce a gap"
        );

        // Non-adjacent anchors produce a gap row.
        let rows = format_anchored_context(&[1, 9], &file);
        assert!(rows.iter().any(|r| r == "..."), "non-adjacent windows gap");
    }

    #[test]
    fn anchored_context_skips_out_of_range() {
        let file: Vec<String> = vec!["only".to_string()];
        let rows = format_anchored_context(&[0, 1, 99], &file);
        // Only line 1 (in range) contributes.
        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("*1:only"));
    }

    #[test]
    fn format_line_ranges_compresses() {
        assert_eq!(format_line_ranges(&[]), "");
        assert_eq!(format_line_ranges(&[1, 2, 3, 4]), "1-4");
        assert_eq!(format_line_ranges(&[1, 3, 5]), "1, 3, 5");
        assert_eq!(format_line_ranges(&[10, 11, 12, 7]), "7, 10-12");
        // Duplicates collapse and order is normalized.
        assert_eq!(format_line_ranges(&[3, 3, 1, 2]), "1-3");
    }

    #[test]
    fn unseen_lines_message_renders_ranges() {
        let msg = unseen_lines_message("src/a.rs", &[1, 2, 3, 7], "ABCD");
        assert!(msg.contains("lines 1-3, 7 of src/a.rs"));
        assert!(msg.contains("[src/a.rs#ABCD]"));
        assert!(msg.contains("`src/a.rs:1-3,7`"));
    }

    #[test]
    fn missing_tag_message_renders_header_hint() {
        let msg = missing_snapshot_tag_message("src/a.rs");
        assert!(msg.contains("Missing hashline snapshot tag for src/a.rs"));
        assert!(msg.contains("`[src/a.rs#tag]`"));
    }

    #[test]
    fn block_unresolved_appends_context() {
        let file: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let msg = block_unresolved_message(2, BlockOpKind::Replace, Some(&file));
        assert!(msg.contains("`SWAP.BLK 2:`"));
        assert!(msg.contains("Use `SWAP 2.=M:`"));
        assert!(msg.contains("\n\n"));
    }

    #[test]
    fn block_unresolved_delete_form() {
        let msg = block_unresolved_message(5, BlockOpKind::Delete, None);
        assert!(msg.contains("`DEL.BLK 5`"));
        assert!(msg.contains("Use `DEL 5.=M`"));
    }

    #[test]
    fn after_insert_landing_pluralizes() {
        assert!(
            !after_insert_landing_shift_warning(1, 3, 1).contains("closing lines"),
            "singular crossing"
        );
        assert!(
            after_insert_landing_shift_warning(1, 3, 2).contains("closing lines"),
            "plural crossing"
        );
    }

    #[test]
    fn block_single_line_message_forms() {
        let m = block_single_line_message(4, BlockOpKind::Replace);
        assert!(m.contains("`SWAP.BLK 4`"));
        assert!(m.contains("use `SWAP 4.=4:`"));
        let m = block_single_line_message(4, BlockOpKind::Delete);
        assert!(m.contains("use `DEL 4`"));
        let m = block_single_line_message(4, BlockOpKind::InsertAfter);
        assert!(m.contains("use `INS.POST 4:`"));
    }

    #[test]
    fn marker_constants_are_stable() {
        assert_eq!(BEGIN_PATCH_MARKER, "*** Begin Patch");
        assert_eq!(END_PATCH_MARKER, "*** End Patch");
        assert_eq!(ABORT_MARKER, "*** Abort");
        assert_eq!(MISMATCH_CONTEXT, 2);
    }
}
