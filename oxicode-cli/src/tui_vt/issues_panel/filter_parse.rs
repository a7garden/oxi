//! Parses the `/` filter-modal free-text buffer into an `IssueFilter`.
//! Syntax: space-separated `key=value` tokens (`priority=`, `label=`); any
//! remaining unrecognized tokens are joined back into `text` (title substring
//! match). Unknown `priority=` values are ignored (filter falls back to no
//! priority constraint) rather than erroring — this is a live-typing buffer.

use oxicode_sdk::{IssueFilter, Priority, Status};

pub(crate) fn parse_issue_filter(input: &str, status_filter: Option<Status>) -> IssueFilter {
    let mut priority = None;
    let mut label = None;
    let mut text_tokens = Vec::new();

    for token in input.split_whitespace() {
        if let Some(v) = token.strip_prefix("priority=") {
            priority = parse_priority(v);
        } else if let Some(v) = token.strip_prefix("label=") {
            label = Some(v.to_string());
        } else {
            text_tokens.push(token);
        }
    }

    IssueFilter {
        status: status_filter,
        priority,
        label,
        assigned_to_session: None,
        text: if text_tokens.is_empty() {
            None
        } else {
            Some(text_tokens.join(" "))
        },
    }
}

fn parse_priority(v: &str) -> Option<Priority> {
    match v.to_ascii_lowercase().as_str() {
        "low" => Some(Priority::Low),
        "medium" | "med" => Some(Priority::Medium),
        "high" => Some(Priority::High),
        "critical" | "crit" => Some(Priority::Critical),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_keeps_only_status() {
        let f = parse_issue_filter("", Some(Status::Open));
        assert_eq!(f.status, Some(Status::Open));
        assert!(f.priority.is_none());
        assert!(f.label.is_none());
        assert!(f.text.is_none());
    }

    #[test]
    fn parses_priority_and_label() {
        let f = parse_issue_filter("priority=critical label=auth", Some(Status::Open));
        assert_eq!(f.priority, Some(Priority::Critical));
        assert_eq!(f.label.as_deref(), Some("auth"));
        assert!(f.text.is_none());
    }

    #[test]
    fn unrecognized_priority_value_is_ignored() {
        let f = parse_issue_filter("priority=urgent", Some(Status::Open));
        assert!(f.priority.is_none());
    }

    #[test]
    fn leftover_tokens_join_into_text() {
        let f = parse_issue_filter("priority=high login bug", Some(Status::Open));
        assert_eq!(f.priority, Some(Priority::High));
        assert_eq!(f.text.as_deref(), Some("login bug"));
    }

    #[test]
    fn none_status_passes_through() {
        let f = parse_issue_filter("", None);
        assert!(f.status.is_none());
        assert!(f.priority.is_none());
        assert!(f.label.is_none());
        assert!(f.text.is_none());
    }
}
