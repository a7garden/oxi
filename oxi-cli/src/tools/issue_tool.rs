//! `issue` agent tool — agent-driven local issue management.
//!
//! Implements [`oxi_agent::AgentTool`] against the [`FileIssueStore`]. One
//! tool with an `action` discriminator (matches the pattern used by the
//! `github` tool, see `oxi-agent/src/tools/github.rs`).
//!
//! Actions: `list`, `read`, `create`, `update`, `start`, `release`, `close`,
//! `link_session`. See [`AgentTool::parameters_schema`] for the exact JSON
//! schema each action accepts.
//!
//! Design notes:
//! - The tool holds an `Arc<FileIssueStore>` so it can be cheaply cloned when
//!   the tool is registered in the live `ToolRegistry` (mirrors how
//!   `McpTool`/`WasmTool` hold their managers).
//! - The "current session id" used for assignment / session-linking is taken
//!   from `ToolContext.session_id`. The agent loop fills this in from the
//!   active session.

use std::sync::Arc;

use async_trait::async_trait;
use oxi_agent::{AgentTool, AgentToolResult, ToolContext};
use serde_json::{Value, json};

use crate::store::issues::{
    FileIssueStore, Issue, IssueError, IssueFilter, IssuePatch, Priority, Status,
};

/// The `issue` tool. One registration, multiple actions.
#[derive(Debug, Clone)]
pub struct IssueTool {
    store: Arc<FileIssueStore>,
}

impl IssueTool {
    /// Construct a new `issue` tool backed by `store`.
    pub fn new(store: FileIssueStore) -> Self {
        Self {
            store: Arc::new(store),
        }
    }
}

#[async_trait]
impl AgentTool for IssueTool {
    fn name(&self) -> &str {
        "issue"
    }

    fn label(&self) -> &str {
        "Issue"
    }

    fn description(&self) -> &str {
        "Manage local issues stored as markdown files in `.oxi/issues/`. \
         Before editing, call `start` to claim the issue — this prevents other \
         agents/sessions from concurrently working on the same issue. Always \
         call `list` first to see existing issues and avoid duplicates. \
         Use `release` to give up a claim, or `close` to finish the work."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "read", "create", "update", "reopen", "start", "release", "close", "link_session"],
                    "description": "Which issue operation to perform."
                },
                "id": {"type": "integer", "description": "Issue id (for read/update/start/release/close)."},
                "title": {"type": "string", "description": "Issue title (for create)."},
                "body": {"type": "string", "description": "Markdown body (for create/update)."},
                "priority": {"type": "string", "enum": ["low", "medium", "high", "critical"], "description": "Priority."},
                "labels": {"type": "array", "items": {"type": "string"}, "description": "String labels."},
                "status": {"type": "string", "enum": ["open", "closed"], "description": "Status filter (for list) or new status (for update)."},
                "label": {"type": "string", "description": "Filter list to issues with this label."},
                "text": {"type": "string", "description": "Substring filter on title (list)."},
                "content_hash": {"type": "string", "description": "Optional hash from the last read; if the file has changed, the write is rejected."}
            },
            "required": ["action"]
        })
    }

    fn essential(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, String> {
        let action = match params.get("action").and_then(|v| v.as_str()) {
            Some(a) => a.to_string(),
            None => return Ok(AgentToolResult::error("missing required field: action")),
        };

        let session = ctx.session_id.clone().unwrap_or_default();
        let result: Result<String, String> = match action.as_str() {
            "list" => self.list(params),
            "read" => self.read(params).await,
            "create" => self.create(params, &session).await,
            "update" => self.update(params, &session).await,
            "start" => self.start(params, &session).await,
            "release" => self.release(params, &session).await,
            "close" => self.close(params, &session).await,
            "reopen" => self.reopen(params).await,
            "link_session" => self.link_session(params, &session).await,
            other => Err(format!("unknown action: {other}")),
        };

        Ok(match result {
            Ok(text) => AgentToolResult::success(text),
            Err(e) => AgentToolResult::error(e),
        })
    }
}

impl IssueTool {
    fn list(&self, params: Value) -> Result<String, String> {
        let status = parse_status_opt(params.get("status"))?;
        let priority = parse_priority_opt(params.get("priority"))?;
        let label = params
            .get("label")
            .and_then(|v| v.as_str())
            .map(String::from);
        let text = params
            .get("text")
            .and_then(|v| v.as_str())
            .map(String::from);
        let filter = IssueFilter {
            status,
            priority,
            label,
            assigned_to_session: None,
            text,
        };
        let issues = self.store.list(&filter).map_err(|e| e.to_string())?;
        if issues.is_empty() {
            return Ok("no issues match the filter".to_string());
        }
        Ok(issues
            .iter()
            .map(format_issue_line)
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn read(&self, params: Value) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        self.store
            .read(id)
            .map(|(issue, hash)| format_issue_full(&issue, &hash))
            .map_err(|e| e.to_string())
    }

    async fn create(&self, params: Value, session: &str) -> Result<String, String> {
        let title = require_string(params.get("title"), "title")?;
        let body = params
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let priority = parse_priority_opt(params.get("priority"))?.unwrap_or(Priority::Medium);
        let labels = parse_labels(params.get("labels"))?;
        let session_opt = if session.is_empty() {
            None
        } else {
            Some(session)
        };
        let issue = self
            .store
            .create(title, body, priority, labels, session_opt)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "created issue #{}: {}",
            issue.meta.id, issue.meta.title
        ))
    }

    async fn update(&self, params: Value, session: &str) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        let agent_hash = hash_param(params.get("content_hash"));
        // IssuePatch makes absent vs [] unambiguous: labels absent → None (keep),
        // labels: [] → Some(vec![]) (clear). Resolves #3.
        let patch = IssuePatch {
            title: params
                .get("title")
                .and_then(|v| v.as_str())
                .map(String::from),
            body: params
                .get("body")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: parse_status_opt(params.get("status"))?,
            priority: parse_priority_opt(params.get("priority"))?,
            labels: params
                .get("labels")
                .map(|v| parse_labels(Some(v)))
                .transpose()?,
        };
        let caller = if session.is_empty() {
            None
        } else {
            Some(session.to_string())
        };
        let store = self.store.clone();
        cas_retry(&store, id, agent_hash, |hash| {
            let store = store.clone();
            let patch = patch.clone();
            let caller = caller.clone();
            async move { store.apply_patch(id, patch, caller, hash).await }
        })
        .await
        .map(|issue| format!("updated issue #{}", issue.meta.id))
        .map_err(|e| e.to_string())
    }

    async fn start(&self, params: Value, session: &str) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        if session.is_empty() {
            return Err("cannot start: no active session id in context".to_string());
        }
        let agent_hash = hash_param(params.get("content_hash"));
        let store = self.store.clone();
        let session = session.to_string();
        cas_retry(&store, id, agent_hash, |hash| {
            let store = store.clone();
            let session = session.clone();
            async move { store.start(id, &session, hash).await }
        })
        .await
        .map(|issue| format!("assigned issue #{} to session {}", issue.meta.id, session))
        .map_err(|e| e.to_string())
    }

    async fn release(&self, params: Value, session: &str) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        if session.is_empty() {
            return Err("cannot release: no active session id in context".to_string());
        }
        let agent_hash = hash_param(params.get("content_hash"));
        let store = self.store.clone();
        let session = session.to_string();
        cas_retry(&store, id, agent_hash, |hash| {
            let store = store.clone();
            let session = session.clone();
            async move { store.release(id, &session, hash).await }
        })
        .await
        .map(|_| format!("released issue #{id}"))
        .map_err(|e| e.to_string())
    }

    async fn close(&self, params: Value, session: &str) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        if session.is_empty() {
            return Err("cannot close: no active session id in context".to_string());
        }
        let agent_hash = hash_param(params.get("content_hash"));
        let store = self.store.clone();
        let session = session.to_string();
        cas_retry(&store, id, agent_hash, |hash| {
            let store = store.clone();
            let session = session.clone();
            async move { store.close(id, &session, hash).await }
        })
        .await
        .map(|issue| format!("closed issue #{}: {}", issue.meta.id, issue.meta.title))
        .map_err(|e| e.to_string())
    }

    async fn reopen(&self, params: Value) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        let agent_hash = hash_param(params.get("content_hash"));
        let store = self.store.clone();
        cas_retry(&store, id, agent_hash, |hash| {
            let store = store.clone();
            async move { store.reopen(id, hash).await }
        })
        .await
        .map(|issue| format!("reopened issue #{}: {}", issue.meta.id, issue.meta.title))
        .map_err(|e| e.to_string())
    }

    async fn link_session(&self, params: Value, session: &str) -> Result<String, String> {
        let id = require_u32(params.get("id"), "id")?;
        if session.is_empty() {
            return Err("cannot link_session: no active session id in context".to_string());
        }
        let agent_hash = hash_param(params.get("content_hash"));
        let store = self.store.clone();
        let session = session.to_string();
        cas_retry(&store, id, agent_hash, |hash| {
            let store = store.clone();
            let session = session.clone();
            async move { store.link_session(id, &session, hash).await }
        })
        .await
        .map(|_| format!("linked session to issue #{id}"))
        .map_err(|e| e.to_string())
    }
}

// ── Formatting helpers (shared with the CLI subcommand in Phase 1.5) ─────

/// Render an issue as a single summary line (id, status, priority, lock,
/// title, labels, assignee). Used by both the agent tool and the CLI.
pub fn format_issue_line(i: &Issue) -> String {
    let lock = if i.meta.assigned_to.is_some() {
        "🔒"
    } else {
        " "
    };
    let assignee = i
        .meta
        .assigned_to
        .as_ref()
        .map(|a| format!(" (assigned: {})", short_session(&a.session)))
        .unwrap_or_default();
    format!(
        "#{:<4} [{}] {:8} {}{} {}{}",
        i.meta.id,
        i.meta.status,
        i.meta.priority,
        lock,
        i.meta.title,
        i.meta.labels.join(","),
        assignee,
    )
}

/// Render a full issue view (summary line + meta + body). Used by both the
/// agent tool and the CLI.
pub fn format_issue_full(i: &Issue, hash: &str) -> String {
    let mut s = format_issue_line(i);
    s.push('\n');
    s.push_str(&format!("  id: {}\n", i.meta.id));
    s.push_str(&format!("  created: {}\n", i.meta.created_at));
    s.push_str(&format!("  updated: {}\n", i.meta.updated_at));
    if let Some(c) = i.meta.closed_at {
        s.push_str(&format!("  closed: {}\n", c));
    }
    s.push_str(&format!("  sessions: {:?}\n", i.meta.sessions));
    if let Some(a) = &i.meta.assigned_to {
        s.push_str(&format!(
            "  assigned: {} (since {})\n",
            short_session(&a.session),
            a.acquired_at
        ));
    }
    s.push_str(&format!("  content_hash: {}\n", hash));
    s.push('\n');
    s.push_str(&i.body);
    s
}

fn short_session(s: &str) -> String {
    if s.len() <= 8 {
        s.to_string()
    } else {
        format!("{}…", &s[..8])
    }
}

// ── Param parsers (centralized so each action doesn't repeat) ────────────

/// Maximum CAS attempts before giving up (#2). The first attempt uses the
/// agent's `content_hash` (fast path); on each [`IssueError::Conflict`] we
/// re-read a fresh hash and retry. So a stale hash from the agent still
/// succeeds as long as contention resolves within the bound. See
/// `docs/designs/2026-06-17-issue-system-hardening.md` §1.4 for why the hash
/// gate is *required* for safe automatic recovery.
const MAX_CAS_ATTEMPTS: u32 = 4;

/// Run an issue store mutation under bounded compare-and-set retry.
///
/// The store is deliberately strict: it returns raw [`IssueError::Conflict`]
/// and never retries on its own (principle: strictness in the store, recovery
/// in the tool). This helper owns the recovery — re-reading a fresh hash after
/// each conflict and retrying up to [`MAX_CAS_ATTEMPTS`] times.
async fn cas_retry<T, F, Fut>(
    store: &FileIssueStore,
    id: u32,
    agent_hash: Option<String>,
    mut op: F,
) -> Result<T, IssueError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<T, IssueError>> + Send,
    T: Send,
{
    let mut hash = agent_hash;
    for attempt in 0..MAX_CAS_ATTEMPTS {
        match op(hash.clone()).await {
            Ok(v) => return Ok(v),
            Err(IssueError::Conflict { .. }) if attempt + 1 < MAX_CAS_ATTEMPTS => {
                tracing::debug!(
                    id,
                    attempt = attempt + 1,
                    "issue CAS conflict, re-reading fresh hash"
                );
                hash = store.read(id).ok().map(|(_, h)| h);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(IssueError::Conflict { id })
}

/// Extract a non-empty `content_hash` from params (absent/empty → `None`).
fn hash_param(v: Option<&Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn require_string(v: Option<&Value>, name: &str) -> Result<String, String> {
    v.and_then(|x| x.as_str())
        .map(String::from)
        .ok_or_else(|| format!("missing required field: {name}"))
}

fn require_u32(v: Option<&Value>, name: &str) -> Result<u32, String> {
    v.and_then(|x| x.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("missing or invalid field: {name}"))
}

fn parse_status_opt(v: Option<&Value>) -> Result<Option<Status>, String> {
    let Some(v) = v else { return Ok(None) };
    let s = v
        .as_str()
        .ok_or_else(|| "status must be a string".to_string())?;
    match s {
        "open" => Ok(Some(Status::Open)),
        "closed" => Ok(Some(Status::Closed)),
        other => Err(format!("invalid status: {other}")),
    }
}

fn parse_priority_opt(v: Option<&Value>) -> Result<Option<Priority>, String> {
    let Some(v) = v else { return Ok(None) };
    let s = v
        .as_str()
        .ok_or_else(|| "priority must be a string".to_string())?;
    match s {
        "low" => Ok(Some(Priority::Low)),
        "medium" => Ok(Some(Priority::Medium)),
        "high" => Ok(Some(Priority::High)),
        "critical" => Ok(Some(Priority::Critical)),
        other => Err(format!("invalid priority: {other}")),
    }
}

fn parse_labels(v: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(v) = v else { return Ok(vec![]) };
    let arr = v
        .as_array()
        .ok_or_else(|| "labels must be an array of strings".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| "labels must be an array of strings".to_string())?;
        out.push(s.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! Phase 2 coverage for the tool-layer CAS retry helper (#2). The store
    //! is strict (raw Conflict, no retry); `cas_retry` owns recovery.
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, FileIssueStore) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".oxi").join("issues");
        std::fs::create_dir_all(&dir).unwrap();
        (tmp, FileIssueStore::open(dir).unwrap())
    }

    #[tokio::test]
    async fn cas_retry_recovers_from_stale_hash() {
        // The agent passes a stale/wrong content_hash. First attempt conflicts;
        // cas_retry re-reads a fresh hash and retries → succeeds. This is the
        // whole point of #2: a stale hash is advisory, not fatal.
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let id = 1;

        let result: Result<Issue, _> = cas_retry(
            &store,
            id,
            Some("deadbeefdeadbeef".to_string()), // deliberately wrong
            |hash| {
                let store = store.clone();
                async move {
                    store
                        .apply_patch(
                            id,
                            IssuePatch {
                                title: Some("Patched".into()),
                                ..Default::default()
                            },
                            None,
                            hash,
                        )
                        .await
                }
            },
        )
        .await;
        let issue = result.expect("cas_retry should recover from a stale hash");
        assert_eq!(issue.meta.title, "Patched");
    }

    #[tokio::test]
    async fn cas_retry_gives_up_after_bound() {
        // If contention never resolves, cas_retry surfaces Conflict after
        // MAX_CAS_ATTEMPTS — it never loops forever.
        let (_tmp, store) = tmp_store();
        store
            .create("T".into(), "b".into(), Priority::Low, vec![], None)
            .unwrap();
        let id = 1;

        let result: Result<Issue, _> = cas_retry(&store, id, None, |_hash| async move {
            Err(IssueError::Conflict { id })
        })
        .await;
        assert!(
            matches!(result, Err(IssueError::Conflict { id: 1 })),
            "must give up with Conflict after the bound, got: {result:?}"
        );
    }
}
