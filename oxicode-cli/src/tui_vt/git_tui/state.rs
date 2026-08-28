//! Git status — `git status --porcelain -z` parsing.
//!
//! Pure data model — no terminal I/O. Consumed by the git TUI overlay.

/// One entry parsed from `git status --porcelain -z`.
///
/// `-z` mode uses NUL (`\0`) as the record separator. Renames produce two
/// consecutive records: the status line (`XY old\0`) followed by the new
/// path (`new\0`). Paths are emitted verbatim — `-z` mode does NOT escape or
/// quote them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Final path on disk (the new path on renames).
    pub path: String,
    /// Previous path on renames; `None` otherwise.
    pub old_path: Option<String>,
    /// Two-character status code. `X` is the staged index status, `Y` is
    /// the unstaged worktree status. A literal space denotes "no change".
    pub xy: [char; 2],
    /// `true` when `R` appears in `xy`.
    pub is_rename: bool,
    /// `true` when either index slot or worktree slot is `U`, or when the
    /// pair is `AA`/`DD` (unmerged add/add or delete/delete conflicts).
    pub is_unmerged: bool,
}

/// Parse the raw bytes of `git status --porcelain -z` into [`StatusEntry`]s.
///
/// Records are separated by NUL bytes. Each non-rename entry is exactly one
/// record: `XY path\0`. A rename is two consecutive records: `XY old\0new\0`
/// — the second record carries only the new path (no status code) and is
/// merged into the previous entry.
pub fn parse_status_porcelain_z(data: &[u8]) -> Vec<StatusEntry> {
    // Split on NUL; trailing empty token (from a final NUL) is dropped.
    let parts: Vec<&[u8]> = data.split(|b| *b == 0).filter(|p| !p.is_empty()).collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let rec = parts[i];
        if rec.len() < 3 {
            // Malformed — skip.
            i += 1;
            continue;
        }
        let x = rec[0] as char;
        let y = rec[1] as char;
        // `git status --porcelain -z` keeps the single space separator
        // between the XY status code and the path. Skip it.
        let path_bytes = if rec.len() > 3 && rec[2] == b' ' {
            &rec[3..]
        } else {
            &rec[2..]
        };
        let path = std::str::from_utf8(path_bytes)
            .map(str::to_string)
            .unwrap_or_default();

        let is_rename = x == 'R' || y == 'R';

        // `git status --porcelain -z` emits renames as two consecutive
        // NUL-separated records: the status record (`XY<space>old_path\0`)
        // followed by a path-only record containing only the new path
        // (`new_path\0`). When the XY has `R` in either slot we consume
        // the next record as the new path.
        let (final_path, old_path) = if is_rename {
            if let Some(next) = parts.get(i + 1) {
                let new_path = std::str::from_utf8(next)
                    .map(str::to_string)
                    .unwrap_or_default();
                (new_path, Some(path))
            } else {
                // Rename at end of stream with no follow-up — fall back to
                // the original path (no rename visible).
                (path, None)
            }
        } else {
            (path, None)
        };

        let merged = StatusEntry {
            path: final_path,
            old_path,
            xy: [x, y],
            is_rename,
            is_unmerged: detect_unmerged(x, y),
        };

        if is_rename && i + 1 < parts.len() {
            // Consume the follow-up path token.
            out.push(merged);
            i += 2;
        } else {
            out.push(merged);
            i += 1;
        }
    }

    out
}

fn detect_unmerged(x: char, y: char) -> bool {
    // Per `git status --porcelain`: any of the resolved XY codes that signal
    // an in-progress conflict between the index and the worktree. The two
    // `U` wildcards already cover every cross-pair (AU / UA / DU / UD /
    // UT / TU), so the explicit list below only enumerates the non-`U`
    // symmetric and asymmetric AA/AD/DA/DD conflicts. Together these flag
    // every "user intervention required before commit" combination.
    matches!(
        (x, y),
        ('U', _) | (_, 'U') | ('A', 'A') | ('A', 'D') | ('D', 'A') | ('D', 'D')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_z_parse_handles_rename_and_unmerged() {
        // Three records: normal M file, rename R  old → new, unmerged UU path.
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"M  file.rs\0");
        bytes.extend_from_slice(b"R  old.rs\0new.rs\0");
        bytes.extend_from_slice(b"UU conflicted.txt\0");

        let entries = parse_status_porcelain_z(&bytes);
        assert_eq!(entries.len(), 3);

        // 1) normal entry
        assert_eq!(entries[0].path, "file.rs");
        assert_eq!(entries[0].old_path, None);
        assert_eq!(entries[0].xy, ['M', ' ']);
        assert!(!entries[0].is_rename);
        assert!(!entries[0].is_unmerged);

        // 2) rename — new path becomes `path`, old_path becomes the source.
        assert_eq!(entries[1].path, "new.rs");
        assert_eq!(entries[1].old_path.as_deref(), Some("old.rs"));
        assert_eq!(entries[1].xy, ['R', ' ']);
        assert!(entries[1].is_rename);
        assert!(!entries[1].is_unmerged);

        // 3) unmerged UU
        assert_eq!(entries[2].path, "conflicted.txt");
        assert_eq!(entries[2].old_path, None);
        assert_eq!(entries[2].xy, ['U', 'U']);
        assert!(!entries[2].is_rename);
        assert!(entries[2].is_unmerged);
    }

    #[test]
    fn porcelain_z_parse_flags_au_and_ut_as_unmerged() {
        // AU (add by us / unmerged) and UT (unmerged / type-change) are
        // symmetric unmerged indicators that the original detection
        // predicate missed.
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"AU halfmerged.txt\0");
        bytes.extend_from_slice(b"UT typechanged.bin\0");

        let entries = parse_status_porcelain_z(&bytes);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].xy, ['A', 'U']);
        assert!(!entries[0].is_rename);
        assert!(entries[0].is_unmerged, "AU must be flagged unmerged");

        assert_eq!(entries[1].xy, ['U', 'T']);
        assert!(!entries[1].is_rename);
        assert!(entries[1].is_unmerged, "UT must be flagged unmerged");
    }
}
