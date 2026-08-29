//! Filter predicates for [`crate::issues::FileIssueStore::list`].

use crate::issues::types::{Issue, Priority, Status};

/// Filter for `list`. All fields optional (None = no constraint).
#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    /// Constrain by status.
    pub status: Option<Status>,
    /// Constrain by priority.
    pub priority: Option<Priority>,
    /// Constrain to issues carrying this label.
    pub label: Option<String>,
    /// Constrain to issues assigned to this session id.
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
