// oxicode-cli/src/tui_vt/issues_panel/form.rs
//! Pure helpers for the Create/Edit form (Task 8).
//!
//! `cycle_priority` and `parse_labels` have no interaction with `TextArea`,
//! `FileIssueStore`, or any TUI state — they're pure functions on `Priority`
//! and `&str`, so each gets a focused unit test below. Keeping them out of
//! `mod.rs`/`input.rs` means the tests run instantly and without a
//! `parking_lot::Mutex` plumbing.

use oxicode_sdk::Priority;

pub(crate) fn cycle_priority(p: Priority, forward: bool) -> Priority {
    use Priority::*;
    match (p, forward) {
        (Low, true) => Medium,
        (Medium, true) => High,
        (High, true) => Critical,
        (Critical, true) => Low,
        (Low, false) => Critical,
        (Medium, false) => Low,
        (High, false) => Medium,
        (Critical, false) => High,
    }
}

pub(crate) fn parse_labels(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_priority_forward_wraps_at_critical() {
        assert_eq!(cycle_priority(Priority::Critical, true), Priority::Low);
    }

    #[test]
    fn cycle_priority_backward_wraps_at_low() {
        assert_eq!(cycle_priority(Priority::Low, false), Priority::Critical);
    }

    #[test]
    fn cycle_priority_forward_steps_through_all() {
        assert_eq!(cycle_priority(Priority::Low, true), Priority::Medium);
        assert_eq!(cycle_priority(Priority::Medium, true), Priority::High);
        assert_eq!(cycle_priority(Priority::High, true), Priority::Critical);
    }

    #[test]
    fn parse_labels_splits_trims_and_drops_empty() {
        assert_eq!(
            parse_labels(" auth, bug ,,ui "),
            vec!["auth".to_string(), "bug".to_string(), "ui".to_string()]
        );
    }

    #[test]
    fn parse_labels_empty_string_yields_empty_vec() {
        assert!(parse_labels("").is_empty());
        assert!(parse_labels("   ").is_empty());
    }
}
