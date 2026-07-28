//! Filter predicates for [`crate::store::issues::FileIssueStore::list`].

use crate::store::issues::types::{Issue, Priority, Status};

/// Filter for `list`. All fields optional (None = no constraint).
#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    pub label: Option<String>,
    pub assigned_to_session: Option<String>,
    /// Text substring match on title (case-insensitive).
    pub text: Option<String>,
}

impl IssueFilter {
    /// Check if an issue matches this filter. All non-None fields must match.
    pub fn matches(&self, issue: &Issue) -> bool {
        if let Some(s) = self.status
            && issue.meta.status != s
        {
            return false;
        }
        if let Some(p) = self.priority
            && issue.meta.priority != p
        {
            return false;
        }
        if let Some(ref label) = self.label
            && !issue.meta.labels.iter().any(|l| l == label)
        {
            return false;
        }
        if let Some(ref session) = self.assigned_to_session {
            let mine = issue
                .meta
                .assigned_to
                .as_ref()
                .map(|a| &a.session == session)
                .unwrap_or(false);
            if !mine {
                return false;
            }
        }
        if let Some(ref text) = self.text
            && !issue
                .meta
                .title
                .to_lowercase()
                .contains(&text.to_lowercase())
        {
            return false;
        }
        true
    }
}
