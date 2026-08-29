//! Markdown + YAML frontmatter serialization for issues.
//!
//! See [`parse_issue`] / [`serialize_issue`] for the on-disk format.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::issues::types::{Issue, IssueMeta, Priority, Status};

const FRONTMATTER_DELIM: &str = "---";

/// Parse a markdown-with-frontmatter file into an [`Issue`].
///
/// Format:
/// ```text
/// ---
/// <yaml>
/// ---
/// <markdown body>
/// ```
///
/// A missing closing delimiter is treated as "the rest is body". Missing
/// frontmatter entirely yields an empty meta (caller decides whether that's
/// an error).
pub fn parse_issue(raw: &str, path: Option<PathBuf>) -> Result<Issue> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);

    // Split off the opening delimiter.
    // No leading frontmatter delimiter → synthesize an empty meta and treat
    // the whole input as body.
    let after_open = match raw.strip_prefix(FRONTMATTER_DELIM) {
        Some(rest) => rest,
        None => {
            return Ok(Issue {
                meta: empty_meta(),
                body: raw.to_string(),
                path,
            });
        }
    };

    // Robust line-based scan for the closing `---` delimiter. Everything
    // between the opening and closing lines is YAML; everything after is body.
    let mut yaml = String::new();
    let mut body = String::new();
    let mut closed = false;
    for line in after_open.split_inclusive('\n') {
        if !closed && line.trim_end() == FRONTMATTER_DELIM {
            closed = true;
            continue;
        }
        if !closed {
            yaml.push_str(line);
        } else {
            body.push_str(line);
        }
    }

    let meta: IssueMeta =
        serde_yaml::from_str(&yaml).context("failed to parse issue frontmatter")?;
    Ok(Issue { meta, body, path })
}

/// Serialize an issue back to the markdown-with-frontmatter form.
pub fn serialize_issue(issue: &Issue) -> Result<String> {
    let yaml = serde_yaml::to_string(&issue.meta).context("failed to serialize frontmatter")?;
    // serde_yaml emits a trailing newline; the `---` document markers are
    // *not* added by serde_yaml, so we wrap manually.
    let body = if issue.body.is_empty() {
        String::new()
    } else if issue.body.ends_with('\n') {
        issue.body.clone()
    } else {
        format!("{}\n", issue.body)
    };
    Ok(format!(
        "{open}\n{yaml}{close}\n{body}",
        open = FRONTMATTER_DELIM,
        close = FRONTMATTER_DELIM
    ))
}

/// Compute a content hash used for optimistic concurrency (same idea as the
/// `edit` tool's `expected_hash`). Uses the std default hasher for zero deps.
pub fn content_hash(raw: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ============================================================================
// Project-root discovery
// ============================================================================

/// Walk up from `start` looking for a `.oxicode/` directory. Returns the path to
/// `<root>/.oxicode/issues`. If no `.oxicode/` exists, returns `<start>/.oxicode/issues`
/// (lazily created on first write).
///
/// Mirrors the walk in `Settings::find_project_settings`.
pub fn issues_dir(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".oxicode").is_dir() {
            return dir.join(".oxicode").join("issues");
        }
        if !dir.pop() {
            break;
        }
    }
    start.join(".oxicode").join("issues")
}

/// Filename for an issue: zero-padded 4-digit id + slugified title.
pub fn issue_filename(id: u32, title: &str) -> String {
    let slug = slugify(title);
    if slug.is_empty() {
        format!("{:04}.md", id)
    } else {
        format!("{:04}-{}.md", id, slug)
    }
}

/// Construct an empty placeholder meta (used when a file has no frontmatter).
fn empty_meta() -> IssueMeta {
    let now = Utc::now();
    IssueMeta {
        id: 0,
        title: String::new(),
        status: Status::default(),
        priority: Priority::default(),
        labels: vec![],
        assignee: None,
        created_at: now,
        updated_at: now,
        closed_at: None,
        sessions: vec![],
        assigned_to: None,
        github: None,
    }
}
/// Slugify a title for use in a filename: lowercase, [a-z0-9-] only.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Fix Login Bug!"), "fix-login-bug");
        assert_eq!(slugify("   spaces   "), "spaces");
        assert_eq!(slugify("a__b"), "a-b");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn issue_filename_format() {
        assert_eq!(issue_filename(12, "Fix Login"), "0012-fix-login.md");
        assert_eq!(issue_filename(1, ""), "0001.md");
    }
}
