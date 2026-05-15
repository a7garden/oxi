//! Session working directory tracking and validation.
//!
//! Session working directory tracking and validation.

use std::path::Path;

/// Information about a missing session working directory.
#[derive(Debug, Clone)]
pub struct SessionCwdIssue {
    /// Path to the session file (if known).
    pub session_file: Option<String>,
    /// The stored session working directory that no longer exists.
    pub session_cwd: String,
    /// The fallback (current) working directory.
    pub fallback_cwd: String,
}

/// Trait for sources that can provide session CWD info.
pub trait SessionCwdSource {
    /// Get the working directory stored in the session.
    fn get_cwd(&self) -> Option<String>;
    /// Get the session file path, if any.
    fn get_session_file(&self) -> Option<String>;
}

/// Check whether the session's stored CWD is missing.
///
/// Returns `Some(SessionCwdIssue)` if the session has a CWD recorded but the
/// directory no longer exists on disk.
pub fn get_missing_session_cwd_issue(
    source: &dyn SessionCwdSource,
    fallback_cwd: &str,
) -> Option<SessionCwdIssue> {
    let session_file = source.get_session_file()?;
    let session_cwd = source.get_cwd()?;

    // If the directory still exists, no issue
    if Path::new(&session_cwd).is_dir() {
        return None;
    }

    Some(SessionCwdIssue {
        session_file: Some(session_file),
        session_cwd,
        fallback_cwd: fallback_cwd.to_string(),
    })
}

/// Format an error message for a missing session CWD.
pub fn format_missing_session_cwd_error(issue: &SessionCwdIssue) -> String {
    let session_file_line = issue
        .session_file
        .as_ref()
        .map(|f| format!("\nSession file: {}", f))
        .unwrap_or_default();
    format!(
        "Stored session working directory does not exist: {}\nCurrent working directory: {}",
        issue.session_cwd, session_file_line, issue.fallback_cwd
    )
}

/// Format a user-facing prompt for the missing CWD situation.
pub fn format_missing_session_cwd_prompt(issue: &SessionCwdIssue) -> String {
    format!(
        "cwd from session file does not exist\n{}\n\ncontinue in current cwd\n{}",
        issue.session_cwd, issue.fallback_cwd
    )
}

/// Error when the session's working directory no longer exists.
#[derive(Debug)]
pub struct MissingSessionCwdError {
/// pub.
    pub issue: SessionCwdIssue,
}

impl std::fmt::Display for MissingSessionCwdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_missing_session_cwd_error(&self.issue))
    }
}

impl std::error::Error for MissingSessionCwdError {}

/// Assert that the session CWD exists, returning an error if it doesn't.
pub fn assert_session_cwd_exists(
    source: &dyn SessionCwdSource,
    fallback_cwd: &str,
) -> Result<(), MissingSessionCwdError> {
    if let Some(issue) = get_missing_session_cwd_issue(source, fallback_cwd) {
        return Err(MissingSessionCwdError { issue });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSource {
        cwd: Option<String>,
        file: Option<String>,
    }

    impl SessionCwdSource for MockSource {
        fn get_cwd(&self) -> Option<String> {
            self.cwd.clone()
        }
        fn get_session_file(&self) -> Option<String> {
            self.file.clone()
        }
    }

    #[test]
    fn no_issue_when_no_session_file() {
        let src = MockSource {
            cwd: Some("/nonexistent".into()),
            file: None,
        };
        assert!(get_missing_session_cwd_issue(&src, "/tmp").is_none());
    }

    #[test]
    fn no_issue_when_cwd_exists() {
        let src = MockSource {
            cwd: Some("/tmp".into()),
            file: Some("/tmp/session.json".into()),
        };
        assert!(get_missing_session_cwd_issue(&src, "/tmp").is_none());
    }

    #[test]
    fn error_format() {
        let issue = SessionCwdIssue {
            session_file: Some("/tmp/s.json".into()),
            session_cwd: "/gone".into(),
            fallback_cwd: "/tmp".into(),
        };
        let msg = format_missing_session_cwd_error(&issue);
        assert!(msg.contains("/gone"));
        assert!(msg.contains("/tmp/s.json"));
    }

    #[test]
    fn prompt_format() {
        let issue = SessionCwdIssue {
            session_file: None,
            session_cwd: "/gone".into(),
            fallback_cwd: "/here".into(),
        };
        let prompt = format_missing_session_cwd_prompt(&issue);
        assert!(prompt.contains("/gone"));
        assert!(prompt.contains("/here"));
    }
}
