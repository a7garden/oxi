//! Line diff algorithm for differential rendering.
//!
//! Compares two frame buffers at the row level and identifies changed rows.

/// Result of comparing two frame buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult {
    /// Index of the first row that changed (inclusive).
    pub first_changed: usize,
    /// Index of the last row that changed (inclusive).
    pub last_changed: usize,
}

/// Find the range of changed rows between two buffers.
///
/// Returns `None` if the buffers are identical (no changes).
/// Returns `Some(DiffResult)` with the inclusive range of changed rows.
pub fn find_changed_rows(
    prev: &[Vec<(char, u32, u32, u16)>],
    curr: &[Vec<(char, u32, u32, u16)>],
) -> Option<DiffResult> {
    let max_rows = prev.len().max(curr.len());

    let mut first_changed = None;
    let mut last_changed = None;

    for row in 0..max_rows {
        let prev_row = prev.get(row);
        let curr_row = curr.get(row);

        match (prev_row, curr_row) {
            (Some(p), Some(c)) if p == c => continue,
            _ => {
                if first_changed.is_none() {
                    first_changed = Some(row);
                }
                last_changed = Some(row);
            }
        }
    }

    first_changed.map(|first| DiffResult {
        first_changed: first,
        last_changed: last_changed.unwrap_or(first),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str) -> Vec<(char, u32, u32, u16)> {
        s.chars().map(|c| (c, 0, 0, 0)).collect()
    }

    fn styled_row(s: &str, fg: u32) -> Vec<(char, u32, u32, u16)> {
        s.chars().map(|c| (c, fg, 0, 0)).collect()
    }

    #[test]
    fn test_no_changes() {
        let prev = vec![row("hello"), row("world")];
        let curr = vec![row("hello"), row("world")];
        assert_eq!(find_changed_rows(&prev, &curr), None);
    }

    #[test]
    fn test_first_row_changed() {
        let prev = vec![row("hello"), row("world")];
        let curr = vec![row("HELLO"), row("world")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 0);
        assert_eq!(diff.last_changed, 0);
    }

    #[test]
    fn test_last_row_changed() {
        let prev = vec![row("hello"), row("world")];
        let curr = vec![row("hello"), row("WORLD")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 1);
    }

    #[test]
    fn test_middle_row_changed() {
        let prev = vec![row("aaa"), row("bbb"), row("ccc")];
        let curr = vec![row("aaa"), row("BBB"), row("ccc")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 1);
    }

    #[test]
    fn test_multiple_rows_changed() {
        let prev = vec![row("aaa"), row("bbb"), row("ccc")];
        let curr = vec![row("AAA"), row("BBB"), row("ccc")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 0);
        assert_eq!(diff.last_changed, 1);
    }

    #[test]
    fn test_style_change_detected() {
        let prev = vec![styled_row("hello", 0)];
        let curr = vec![styled_row("hello", 1)];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 0);
    }

    #[test]
    fn test_buffer_grew() {
        let prev = vec![row("aaa")];
        let curr = vec![row("aaa"), row("bbb")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 1);
    }

    #[test]
    fn test_buffer_shrunk() {
        let prev = vec![row("aaa"), row("bbb")];
        let curr = vec![row("aaa")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 1);
    }

    #[test]
    fn test_empty_buffers() {
        let prev: Vec<Vec<(char, u32, u32, u16)>> = vec![];
        let curr: Vec<Vec<(char, u32, u32, u16)>> = vec![];
        assert_eq!(find_changed_rows(&prev, &curr), None);
    }

    #[test]
    fn test_prev_empty_curr_nonempty() {
        let prev: Vec<Vec<(char, u32, u32, u16)>> = vec![];
        let curr = vec![row("hello")];
        let diff = find_changed_rows(&prev, &curr).unwrap();
        assert_eq!(diff.first_changed, 0);
        assert_eq!(diff.last_changed, 0);
    }
}
