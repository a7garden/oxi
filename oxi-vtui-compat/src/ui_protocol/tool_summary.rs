//! Data and grouping rules for compact tool transition summaries.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactToolSummaryLine {
    pub kind: CompactToolSummaryLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactToolSummaryLineKind {
    Info,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactToolSummaryStatus {
    Success,
    Failure,
    Warning,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactToolSummaryCall {
    pub canonical_tool_name: String,
    pub semantic_action: String,
    pub stable_arguments: String,
    pub headline: String,
    pub details: Vec<CompactToolSummaryDetail>,
    pub output_boundary: bool,
    pub status: CompactToolSummaryStatus,
    pub expanded_lines: Vec<CompactToolSummaryLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactToolSummaryGroup {
    pub calls: Vec<CompactToolSummaryCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactToolSummaryDetail {
    pub label: String,
    pub value: String,
}

pub const MAX_COMPACT_DISTINCT_VALUES: usize = 3;

fn can_join(left: &CompactToolSummaryCall, right: &CompactToolSummaryCall) -> bool {
    !left.output_boundary
        && !right.output_boundary
        && left.status == CompactToolSummaryStatus::Success
        && right.status == CompactToolSummaryStatus::Success
        && left.canonical_tool_name == right.canonical_tool_name
        && left.semantic_action == right.semantic_action
        && left.stable_arguments == right.stable_arguments
}

pub fn adjacent_compact_summary_groups(calls: Vec<CompactToolSummaryCall>) -> Vec<CompactToolSummaryGroup> {
    let mut groups = Vec::new();
    let mut current: Option<CompactToolSummaryGroup> = None;

    for call in calls {
        let joins = current
            .as_ref()
            .and_then(|group| group.calls.last())
            .is_some_and(|last| can_join(last, &call));
        if joins {
            if let Some(group) = &mut current {
                group.calls.push(call);
            }
        } else {
            if let Some(group) = current.take() {
                groups.push(group);
            }
            current = Some(CompactToolSummaryGroup { calls: vec![call] });
        }
    }
    if let Some(group) = current {
        groups.push(group);
    }
    groups
}

pub fn compact_detail_values(group: &CompactToolSummaryGroup) -> Vec<CompactToolSummaryDetail> {
    let mut labels = Vec::new();
    for call in &group.calls {
        for detail in &call.details {
            if !labels.contains(&detail.label) {
                labels.push(detail.label.clone());
            }
        }
    }

    labels
        .into_iter()
        .map(|label| {
            let mut values = Vec::new();
            for call in &group.calls {
                let value = call
                    .details
                    .iter()
                    .find(|detail| detail.label == label)
                    .map(|detail| detail.value.clone())
                    .unwrap_or_else(|| "-".to_string());
                if !values.contains(&value) {
                    values.push(value);
                }
            }
            if values.len() <= 1 {
                return CompactToolSummaryDetail {
                    label,
                    value: values.into_iter().next().unwrap_or_default(),
                };
            }

            let omitted = values.len().saturating_sub(MAX_COMPACT_DISTINCT_VALUES);
            values.truncate(MAX_COMPACT_DISTINCT_VALUES);
            let mut value = values.join(", ");
            if omitted > 0 {
                value.push_str(&format!(", +{omitted} more"));
            }
            CompactToolSummaryDetail { label, value }
        })
        .collect()
}

pub fn stable_arguments_json(value: &serde_json::Value) -> String {
    const PAGINATION_ARGUMENTS: &[&str] = &["max_results", "limit", "offset", "page", "cursor"];

    fn normalize(value: &serde_json::Value, root: bool) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let ordered = map
                    .iter()
                    .filter(|(key, _)| !root || !PAGINATION_ARGUMENTS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), normalize(value, false)))
                    .collect::<BTreeMap<_, _>>();
                serde_json::Value::Object(ordered.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|item| normalize(item, false)).collect())
            }
            _ => value.clone(),
        }
    }

    serde_json::to_string(&normalize(value, true)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(stable_arguments: &str, limit: &str) -> CompactToolSummaryCall {
        CompactToolSummaryCall {
            canonical_tool_name: "code_search".to_string(),
            semantic_action: "Search code".to_string(),
            stable_arguments: stable_arguments.to_string(),
            headline: "Search code".to_string(),
            details: vec![CompactToolSummaryDetail {
                label: "Max results".to_string(),
                value: limit.to_string(),
            }],
            output_boundary: false,
            status: CompactToolSummaryStatus::Success,
            expanded_lines: Vec::new(),
        }
    }

    #[test]
    fn groups_identical_adjacent_calls() {
        let groups = adjacent_compact_summary_groups(vec![call("{}", "30"), call("{}", "100")]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].calls.len(), 2);
        assert_eq!(compact_detail_values(&groups[0])[0].value, "30, 100");
    }

    #[test]
    fn stable_argument_changes_split_groups() {
        let groups =
            adjacent_compact_summary_groups(vec![call("{\"path\":\"a\"}", "30"), call("{\"path\":\"b\"}", "30")]);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn non_adjacent_compatible_calls_remain_separate() {
        let mut other_tool = call("{}", "30");
        other_tool.canonical_tool_name = "list_files".to_string();
        let groups = adjacent_compact_summary_groups(vec![call("{}", "30"), other_tool, call("{}", "100")]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].calls.len(), 1);
        assert_eq!(groups[2].calls.len(), 1);
    }

    #[test]
    fn output_boundaries_split_groups() {
        let mut boundary = call("{}", "30");
        boundary.output_boundary = true;
        let groups = adjacent_compact_summary_groups(vec![call("{}", "30"), boundary, call("{}", "100")]);
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn failures_split_groups_and_remain_visible() {
        let mut failure = call("{}", "30");
        failure.status = CompactToolSummaryStatus::Failure;
        let groups = adjacent_compact_summary_groups(vec![call("{}", "30"), failure, call("{}", "100")]);
        assert_eq!(groups.len(), 3);
        assert!(groups[1].calls[0].status == CompactToolSummaryStatus::Failure);
    }

    #[test]
    fn warnings_and_cancellations_split_groups() {
        for status in [CompactToolSummaryStatus::Warning, CompactToolSummaryStatus::Cancelled] {
            let mut non_success = call("{}", "30");
            non_success.status = status;
            let groups = adjacent_compact_summary_groups(vec![call("{}", "30"), non_success, call("{}", "100")]);
            assert_eq!(groups.len(), 3);
            assert_eq!(groups[1].calls[0].status, status);
        }
    }

    #[test]
    fn distinct_detail_values_are_capped_deterministically() {
        let calls = (0..5).map(|value| call("{}", &value.to_string())).collect::<Vec<_>>();
        let groups = adjacent_compact_summary_groups(calls);
        assert_eq!(compact_detail_values(&groups[0])[0].value, "0, 1, 2, +2 more");
    }

    #[test]
    fn stable_arguments_ignore_pagination_only_values() {
        let first = stable_arguments_json(&json!({"query": "foo", "max_results": 30, "cursor": "a"}));
        let second = stable_arguments_json(&json!({"query": "foo", "max_results": 100, "cursor": "b"}));
        assert_eq!(first, second);
    }
}
