/// GitHub tool — unified GitHub integration via `gh` CLI.
///
/// Sub-commands:
///   search   — search repos, issues, code, commits (gh search repos/issues/code/commits)
///   issue    — list, view, create, close issues (gh issue)
///   pr       — list, view, create, merge PRs (gh pr)
///   repo     — view repo info (gh repo view)
///   run      — list/view workflow runs (gh run)
///
/// Prerequisites: `gh` CLI installed and authenticated (`gh auth status`).
/// Disable via `disabled_tools = ["github"]` or `OXI_DISABLED_TOOLS=github`.

use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use super::search_cache::{SearchCache, SearchResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Maximum results to return by default.
const DEFAULT_MAX_RESULTS: usize = 10;

// ── gh CLI helpers ────────────────────────────────────────────────

/// Check if `gh` CLI is available and authenticated.
async fn check_gh_auth() -> Result<(), ToolError> {
    let output = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await
        .map_err(|e| format!("gh CLI not found: {}. Install from https://cli.github.com", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "gh CLI not authenticated. Run `gh auth login`. Details: {}",
            stderr.chars().take(200).collect::<String>()
        ));
    }
    Ok(())
}

/// Run a `gh` command and return stdout as String.
async fn gh_exec(args: &[&str]) -> Result<String, ToolError> {
    let output = tokio::process::Command::new("gh")
        .args(args)
        .env("GH_FORMAT", "json")
        .output()
        .await
        .map_err(|e| format!("Failed to execute gh: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        return Err(format!(
            "gh {} failed (exit {}): {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            if stderr.is_empty() { &stdout } else { &stderr }
                .chars()
                .take(500)
                .collect::<String>(),
        ));
    }

    Ok(stdout.trim().to_string())
}

// ── Search ────────────────────────────────────────────────────────

async fn gh_search(params: &Value) -> Result<AgentToolResult, ToolError> {
    check_gh_auth().await?;

    let query = params["query"]
        .as_str()
        .ok_or_else(|| "Missing required parameter: query".to_string())?;

    let kind = params["kind"].as_str().unwrap_or("repos");
    let limit = params["limit"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_RESULTS as u64)
        .min(30) as usize;

    let json_fields = match kind {
        "repos" => "--json=name,fullName,url,description,language,stargazersCount,forksCount,issues,updatedAt,repositoryTopics,licenseInfo",
        "issues" => "--json=title,url,state,body,author,labels,createdAt,updatedAt,comments,number",
        "code" => "--json=path,repository,textMatches",
        "commits" => "--json=sha,url,message,author,date",
        _ => return Err(format!("Unknown search kind '{}'. Use: repos, issues, code, commits", kind)),
    };

    let output = gh_exec(&[
        "search", kind,
        query,
        "--limit", &limit.to_string(),
        json_fields,
    ]).await?;

    let items: Vec<Value> = serde_json::from_str(&output)
        .unwrap_or_else(|_| {
            serde_json::from_str::<Value>(&output)
                .map(|v| if v.is_array() { v.as_array().unwrap_or(&Vec::new()).clone() } else { vec![v] })
                .unwrap_or_default()
        });

    let text = format_search_results(kind, &items, query);

    Ok(AgentToolResult::success(text).with_metadata(json!({
        "action": "search",
        "kind": kind,
        "query": query,
        "results": items,
        "count": items.len(),
    })))
}

fn format_search_results(kind: &str, items: &[Value], query: &str) -> String {
    if items.is_empty() {
        return format!("No GitHub {} found for: {}", kind, query);
    }

    let mut out = format!("Found {} GitHub {} for '{}':\n\n", items.len(), kind, query);

    match kind {
        "repos" => {
            for (i, item) in items.iter().enumerate() {
                let name = item["fullName"].as_str().or_else(|| item["name"].as_str()).unwrap_or("?");
                let url = item["url"].as_str().unwrap_or("");
                let desc = item["description"].as_str().unwrap_or("").chars().take(150).collect::<String>();
                let stars = item["stargazersCount"].as_u64().unwrap_or(0);
                let lang = item["language"].as_str().unwrap_or("Unknown");
                let forks = item["forksCount"].as_u64().unwrap_or(0);
                let stars_str = if stars >= 1000 { format!("{:.1}k", stars as f64 / 1000.0) } else { stars.to_string() };
                let topics = item["repositoryTopics"].as_array()
                    .map(|arr| arr.iter().filter_map(|t| t["name"].as_str().or(t.as_str())).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                let license = item["licenseInfo"]["spdxId"].as_str().unwrap_or("");

                out.push_str(&format!(
                    "{}. **{}** ⭐{}\n   {}\n   {} | 🔀 {} forks\n",
                    i + 1, name, stars_str, url, lang, forks
                ));
                if !desc.is_empty() { out.push_str(&format!("   {}\n", desc)); }
                if !topics.is_empty() { out.push_str(&format!("   Topics: {}\n", topics)); }
                if !license.is_empty() { out.push_str(&format!("   License: {}\n", license)); }
                out.push('\n');
            }
        }
        "issues" => {
            for (i, item) in items.iter().enumerate() {
                let title = item["title"].as_str().unwrap_or("?");
                let url = item["url"].as_str().unwrap_or("");
                let state = item["state"].as_str().unwrap_or("OPEN");
                let number = item["number"].as_u64().unwrap_or(0);
                let labels = item["labels"].as_array()
                    .map(|arr| arr.iter().filter_map(|l| l["name"].as_str().or(l.as_str())).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "{}. #{} {} [{}] {}\n",
                    i + 1, number, title, state, url
                ));
                if !labels.is_empty() { out.push_str(&format!("   Labels: {}\n", labels)); }
                out.push('\n');
            }
        }
        "code" => {
            for (i, item) in items.iter().enumerate() {
                let path = item["path"].as_str().unwrap_or("?");
                let repo = item["repository"]["fullName"].as_str().or_else(|| item["repository"].as_str()).unwrap_or("?");
                out.push_str(&format!("{}. {} in {}\n", i + 1, path, repo));
                if let Some(matches) = item["textMatches"].as_array() {
                    for m in matches.iter().take(3) {
                        if let Some(frag) = m["fragment"].as_str() {
                            out.push_str(&format!("   > {}\n", frag.chars().take(120).collect::<String>()));
                        }
                    }
                }
                out.push('\n');
            }
        }
        "commits" => {
            for (i, item) in items.iter().enumerate() {
                let sha = item["sha"].as_str().unwrap_or("?").get(..7).unwrap_or("?");
                let msg = item["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
                let author = item["author"]["name"].as_str().or_else(|| item["author"].as_str()).unwrap_or("?");
                out.push_str(&format!("{}. {} {} — {}\n", i + 1, sha, msg, author));
                out.push('\n');
            }
        }
        _ => {
            for (i, item) in items.iter().enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, item));
            }
        }
    }

    out
}

// ── Issue ─────────────────────────────────────────────────────────

async fn gh_issue(params: &Value) -> Result<AgentToolResult, ToolError> {
    check_gh_auth().await?;

    let action = params["action"].as_str().unwrap_or("list");

    match action {
        "list" => {
            let limit = params["limit"].as_u64().unwrap_or(10).min(30);
            let state = params["state"].as_str().unwrap_or("open");
            let label = params["label"].as_str();

            let limit_str = limit.to_string();
            let mut args = vec!["issue", "list", "--state", state, "--limit", &limit_str,
                "--json", "number,title,url,state,labels,createdAt,updatedAt,author"];
            let label_arg;
            if let Some(l) = label {
                label_arg = format!("--label={}", l);
                args.push(&label_arg);
            }

            let output = gh_exec(&args).await?;
            let items: Vec<Value> = serde_json::from_str(&output).unwrap_or_default();
            let text = format_issue_list(&items);
            Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "issue", "sub": "list", "results": items })))
        }
        "view" => {
            let number = params["number"].as_u64()
                .ok_or_else(|| "Missing parameter: number".to_string())?;
            let output = gh_exec(&["issue", "view", &number.to_string(),
                "--json", "number,title,body,state,author,labels,comments,createdAt,updatedAt"]).await?;
            let issue: Value = serde_json::from_str(&output)
                .map_err(|e| format!("Parse error: {}", e))?;
            let text = format_issue_view(&issue);
            Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "issue", "sub": "view", "issue": issue })))
        }
        "create" => {
            let title = params["title"].as_str()
                .ok_or_else(|| "Missing parameter: title".to_string())?;
            let body = params["body"].as_str().unwrap_or("");
            let mut args = vec!["issue", "create", "--title", title];
            let body_arg;
            if !body.is_empty() {
                body_arg = format!("--body={}", body);
                args.push(&body_arg);
            }
            let output = gh_exec(&args).await?;
            Ok(AgentToolResult::success(format!("Created issue: {}", output)))
        }
        "close" => {
            let number = params["number"].as_u64()
                .ok_or_else(|| "Missing parameter: number".to_string())?;
            let output = gh_exec(&["issue", "close", &number.to_string()]).await?;
            Ok(AgentToolResult::success(format!("Closed issue: {}", output)))
        }
        other => Err(format!("Unknown issue action '{}'. Use: list, view, create, close", other)),
    }
}

fn format_issue_list(items: &[Value]) -> String {
    if items.is_empty() { return "No issues found.".to_string(); }
    let mut out = format!("{} issues:\n\n", items.len());
    for (i, item) in items.iter().enumerate() {
        let num = item["number"].as_u64().unwrap_or(0);
        let title = item["title"].as_str().unwrap_or("?");
        let state = item["state"].as_str().unwrap_or("OPEN");
        let url = item["url"].as_str().unwrap_or("");
        let labels = item["labels"].as_array()
            .map(|arr| arr.iter().filter_map(|l| l["name"].as_str().or(l.as_str())).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        out.push_str(&format!("{}. #{} {} [{}] {}\n", i + 1, num, title, state, url));
        if !labels.is_empty() { out.push_str(&format!("   Labels: {}\n", labels)); }
        out.push('\n');
    }
    out
}

fn format_issue_view(issue: &Value) -> String {
    let title = issue["title"].as_str().unwrap_or("?");
    let num = issue["number"].as_u64().unwrap_or(0);
    let state = issue["state"].as_str().unwrap_or("OPEN");
    let body = issue["body"].as_str().unwrap_or("");
    let url = issue["url"].as_str().unwrap_or("");
    let labels = issue["labels"].as_array()
        .map(|arr| arr.iter().filter_map(|l| l["name"].as_str().or(l.as_str())).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    let comments = issue["comments"].as_array().map(|a| a.len()).unwrap_or(0);

    let mut out = format!("#{} {} [{}]\n{}\n\n", num, title, state, url);
    if !labels.is_empty() { out.push_str(&format!("Labels: {}\n\n", labels)); }
    if !body.is_empty() { out.push_str(&format!("{}\n\n", body.chars().take(1000).collect::<String>())); }
    out.push_str(&format!("Comments: {}\n", comments));
    out
}

// ── PR ────────────────────────────────────────────────────────────

async fn gh_pr(params: &Value) -> Result<AgentToolResult, ToolError> {
    check_gh_auth().await?;

    let action = params["action"].as_str().unwrap_or("list");

    match action {
        "list" => {
            let limit = params["limit"].as_u64().unwrap_or(10).min(30);
            let state = params["state"].as_str().unwrap_or("open");
            let output = gh_exec(&["pr", "list", "--state", state, "--limit", &limit.to_string(),
                "--json", "number,title,url,state,author,createdAt,updatedAt,labels"]).await?;
            let items: Vec<Value> = serde_json::from_str(&output).unwrap_or_default();
            let text = format_pr_list(&items);
            Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "pr", "sub": "list", "results": items })))
        }
        "view" => {
            let number = params["number"].as_u64()
                .ok_or_else(|| "Missing parameter: number".to_string())?;
            let output = gh_exec(&["pr", "view", &number.to_string(),
                "--json", "number,title,body,state,author,labels,additions,deletions,commits,reviews,createdAt"]).await?;
            let pr: Value = serde_json::from_str(&output)
                .map_err(|e| format!("Parse error: {}", e))?;
            let text = format_pr_view(&pr);
            Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "pr", "sub": "view", "pr": pr })))
        }
        "create" => {
            let title = params["title"].as_str()
                .ok_or_else(|| "Missing parameter: title".to_string())?;
            let body = params["body"].as_str().unwrap_or("");
            let base = params["base"].as_str().unwrap_or("main");
            let head = params["head"].as_str().unwrap_or("");
            let mut args = vec!["pr", "create", "--title", title, "--base", base];
            let body_arg;
            let head_arg;
            if !body.is_empty() {
                body_arg = format!("--body={}", body);
                args.push(&body_arg);
            }
            if !head.is_empty() {
                head_arg = format!("--head={}", head);
                args.push(&head_arg);
            }
            let output = gh_exec(&args).await?;
            Ok(AgentToolResult::success(format!("Created PR: {}", output)))
        }
        "merge" => {
            let number = params["number"].as_u64()
                .ok_or_else(|| "Missing parameter: number".to_string())?;
            let strategy = params["strategy"].as_str().unwrap_or("merge");
            let output = gh_exec(&["pr", "merge", &number.to_string(), "--", strategy]).await?;
            Ok(AgentToolResult::success(format!("Merged PR: {}", output)))
        }
        other => Err(format!("Unknown PR action '{}'. Use: list, view, create, merge", other)),
    }
}

fn format_pr_list(items: &[Value]) -> String {
    if items.is_empty() { return "No pull requests found.".to_string(); }
    let mut out = format!("{} pull requests:\n\n", items.len());
    for (i, item) in items.iter().enumerate() {
        let num = item["number"].as_u64().unwrap_or(0);
        let title = item["title"].as_str().unwrap_or("?");
        let state = item["state"].as_str().unwrap_or("OPEN");
        let url = item["url"].as_str().unwrap_or("");
        out.push_str(&format!("{}. #{} {} [{}] {}\n\n", i + 1, num, title, state, url));
    }
    out
}

fn format_pr_view(pr: &Value) -> String {
    let title = pr["title"].as_str().unwrap_or("?");
    let num = pr["number"].as_u64().unwrap_or(0);
    let state = pr["state"].as_str().unwrap_or("OPEN");
    let url = pr["url"].as_str().unwrap_or("");
    let body = pr["body"].as_str().unwrap_or("");
    let additions = pr["additions"].as_u64().unwrap_or(0);
    let deletions = pr["deletions"].as_u64().unwrap_or(0);
    let commits = pr["commits"].as_u64().unwrap_or(0);

    let mut out = format!("#{} {} [{}]\n{}\n\n", num, title, state, url);
    out.push_str(&format!("+{} / -{} across {} commits\n\n", additions, deletions, commits));
    if !body.is_empty() {
        out.push_str(&format!("{}\n\n", body.chars().take(1000).collect::<String>()));
    }
    out
}

// ── Repo ──────────────────────────────────────────────────────────

async fn gh_repo(params: &Value) -> Result<AgentToolResult, ToolError> {
    check_gh_auth().await?;

    let repo = params["repo"].as_str().unwrap_or("");
    let output = gh_exec(&[
        "repo", "view", repo,
        "--json", "name,fullName,url,description,language,stargazersCount,forksCount,issues,defaultBranchRef,createdAt,updatedAt,repositoryTopics,licenseInfo",
    ]).await?;

    let info: Value = serde_json::from_str(&output)
        .map_err(|e| format!("Parse error: {}", e))?;

    let text = format_repo_view(&info);
    Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "repo", "repo": info })))
}

fn format_repo_view(info: &Value) -> String {
    let name = info["fullName"].as_str().unwrap_or("?");
    let desc = info["description"].as_str().unwrap_or("");
    let url = info["url"].as_str().unwrap_or("");
    let stars = info["stargazersCount"].as_u64().unwrap_or(0);
    let forks = info["forksCount"].as_u64().unwrap_or(0);
    let lang = info["language"].as_str().unwrap_or("Unknown");
    let default_branch = info["defaultBranchRef"]["name"].as_str().unwrap_or("main");
    let topics = info["repositoryTopics"].as_array()
        .map(|arr| arr.iter().filter_map(|t| t["name"].as_str().or(t.as_str())).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    let license = info["licenseInfo"]["spdxId"].as_str().unwrap_or("None");

    let stars_str = if stars >= 1000 { format!("{:.1}k", stars as f64 / 1000.0) } else { stars.to_string() };

    let mut out = format!("**{}** ⭐{}\n{}\n\n", name, stars_str, url);
    if !desc.is_empty() { out.push_str(&format!("{}\n\n", desc)); }
    out.push_str(&format!("Language: {} | Forks: {} | Branch: {} | License: {}\n", lang, forks, default_branch, license));
    if !topics.is_empty() { out.push_str(&format!("Topics: {}\n", topics)); }
    out
}

// ── Run (Actions) ─────────────────────────────────────────────────

async fn gh_run(params: &Value) -> Result<AgentToolResult, ToolError> {
    check_gh_auth().await?;

    let action = params["action"].as_str().unwrap_or("list");

    match action {
        "list" => {
            let limit = params["limit"].as_u64().unwrap_or(5).min(20);
            let output = gh_exec(&["run", "list", "--limit", &limit.to_string(),
                "--json", "databaseId,name,status,conclusion,headBranch,createdAt,event"]).await?;
            let items: Vec<Value> = serde_json::from_str(&output).unwrap_or_default();
            let text = format_run_list(&items);
            Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "run", "sub": "list", "results": items })))
        }
        "view" => {
            let id = params["id"].as_u64()
                .ok_or_else(|| "Missing parameter: id".to_string())?;
            let output = gh_exec(&["run", "view", &id.to_string(),
                "--json", "databaseId,name,status,conclusion,headBranch,createdAt,jobs"]).await?;
            let run: Value = serde_json::from_str(&output)
                .map_err(|e| format!("Parse error: {}", e))?;
            let text = format_run_view(&run);
            Ok(AgentToolResult::success(text).with_metadata(json!({ "action": "run", "sub": "view", "run": run })))
        }
        other => Err(format!("Unknown run action '{}'. Use: list, view", other)),
    }
}

fn format_run_list(items: &[Value]) -> String {
    if items.is_empty() { return "No workflow runs found.".to_string(); }
    let mut out = format!("{} workflow runs:\n\n", items.len());
    for (i, item) in items.iter().enumerate() {
        let name = item["name"].as_str().unwrap_or("?");
        let status = item["status"].as_str().unwrap_or("?");
        let conclusion = item["conclusion"].as_str().unwrap_or("in progress");
        let branch = item["headBranch"].as_str().unwrap_or("?");
        let id = item["databaseId"].as_u64().unwrap_or(0);
        out.push_str(&format!("{}. {} — {} ({}) branch: {} id: {}\n",
            i + 1, name, status, conclusion, branch, id));
    }
    out
}

fn format_run_view(run: &Value) -> String {
    let name = run["name"].as_str().unwrap_or("?");
    let status = run["status"].as_str().unwrap_or("?");
    let conclusion = run["conclusion"].as_str().unwrap_or("in progress");
    let branch = run["headBranch"].as_str().unwrap_or("?");
    let id = run["databaseId"].as_u64().unwrap_or(0);

    let mut out = format!("**{}** — {} ({})\nBranch: {} | ID: {}\n\n", name, status, conclusion, branch, id);
    if let Some(jobs) = run["jobs"].as_array() {
        out.push_str(&format!("Jobs ({}):\n", jobs.len()));
        for job in jobs {
            let jname = job["name"].as_str().unwrap_or("?");
            let jstatus = job["status"].as_str().unwrap_or("?");
            let jconclusion = job["conclusion"].as_str().unwrap_or("in progress");
            out.push_str(&format!("  - {} — {} ({})\n", jname, jstatus, jconclusion));
        }
    }
    out
}

// ── GitHubTool ────────────────────────────────────────────────────

/// Unified GitHub tool using `gh` CLI.
pub struct GitHubTool {
    cache: Arc<SearchCache>,
}

impl GitHubTool {
    /// Create a new GitHubTool with the given search cache.
    pub fn new(cache: Arc<SearchCache>) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl AgentTool for GitHubTool {
    fn name(&self) -> &str {
        "github"
    }

    fn label(&self) -> &str {
        "GitHub"
    }

    fn description(&self) -> &str {
        "GitHub integration via gh CLI. Actions: search (repos/issues/code/commits), issue (list/view/create/close), pr (list/view/create/merge), repo (view info), run (workflow runs). Requires gh CLI installed and authenticated."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Top-level action: search, issue, pr, repo, run",
                    "enum": ["search", "issue", "pr", "repo", "run"],
                    "default": "search"
                },
                "query": {
                    "type": "string",
                    "description": "Search query (for action=search)"
                },
                "kind": {
                    "type": "string",
                    "description": "Search kind (for action=search): repos, issues, code, commits",
                    "enum": ["repos", "issues", "code", "commits"],
                    "default": "repos"
                },
                "number": {
                    "type": "integer",
                    "description": "Issue/PR number (for issue view/close, pr view/merge)"
                },
                "title": {
                    "type": "string",
                    "description": "Title (for issue create, pr create)"
                },
                "body": {
                    "type": "string",
                    "description": "Body text (for issue create, pr create)"
                },
                "state": {
                    "type": "string",
                    "description": "Filter by state: open, closed, all",
                    "enum": ["open", "closed", "all"],
                    "default": "open"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 10, max 30)",
                    "default": 10
                },
                "repo": {
                    "type": "string",
                    "description": "Repository (owner/repo format, for action=repo)"
                },
                "base": {
                    "type": "string",
                    "description": "Base branch (for pr create, default: main)"
                },
                "head": {
                    "type": "string",
                    "description": "Head branch (for pr create)"
                },
                "strategy": {
                    "type": "string",
                    "description": "Merge strategy (for pr merge): merge, squash, rebase",
                    "enum": ["merge", "squash", "rebase"],
                    "default": "merge"
                },
                "id": {
                    "type": "integer",
                    "description": "Workflow run ID (for action=run view)"
                },
                "label": {
                    "type": "string",
                    "description": "Filter by label (for issue list)"
                },
                "language": {
                    "type": "string",
                    "description": "Filter by language (for search repos)"
                }
            },
            "required": []
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let action = params["action"].as_str().unwrap_or("search");

        match action {
            "search" => {
                let result = gh_search(&params).await?;
                // Cache search results
                if let Some(query) = params["query"].as_str() {
                    let kind = params["kind"].as_str().unwrap_or("repos");
                    let search_id = self.cache.insert(
                        &format!("github:{}:{}", kind, query),
                        vec![SearchResult {
                            title: format!("GitHub {} search: {}", kind, query),
                            url: String::new(),
                            snippet: result.output.chars().take(200).collect(),
                            engines: vec!["GitHub".to_string()],
                            score: 0.0,
                        }],
                    );
                    return Ok(result.with_metadata(json!({
                        "searchId": search_id,
                    })));
                }
                Ok(result)
            }
            "issue" => gh_issue(&params).await,
            "pr" => gh_pr(&params).await,
            "repo" => gh_repo(&params).await,
            "run" => gh_run(&params).await,
            other => Err(format!(
                "Unknown action '{}'. Use: search, issue, pr, repo, run",
                other
            )),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_search_repos_empty() {
        let text = format_search_results("repos", &[], "test");
        assert!(text.contains("No GitHub repos"));
    }

    #[test]
    fn test_format_search_repos() {
        let items = vec![json!({
            "fullName": "rust-lang/rust",
            "url": "https://github.com/rust-lang/rust",
            "description": "Empowering everyone to build reliable and efficient software.",
            "language": "Rust",
            "stargazersCount": 95000,
            "forksCount": 12000,
            "repositoryTopics": [{"name": "programming-language"}, {"name": "systems"}],
            "licenseInfo": {"spdxId": "MIT/Apache-2.0"}
        })];
        let text = format_search_results("repos", &items, "rust");
        assert!(text.contains("**rust-lang/rust**"));
        assert!(text.contains("95.0k"));
        assert!(text.contains("programming-language, systems"));
    }

    #[test]
    fn test_format_issue_list() {
        let items = vec![json!({
            "number": 42,
            "title": "Bug in parser",
            "state": "OPEN",
            "url": "https://github.com/test/repo/issues/42",
            "labels": [{"name": "bug"}]
        })];
        let text = format_issue_list(&items);
        assert!(text.contains("#42"));
        assert!(text.contains("Bug in parser"));
        assert!(text.contains("bug"));
    }

    #[test]
    fn test_format_pr_list() {
        let items = vec![json!({
            "number": 7,
            "title": "Fix typo",
            "state": "OPEN",
            "url": "https://github.com/test/repo/pull/7"
        })];
        let text = format_pr_list(&items);
        assert!(text.contains("#7"));
        assert!(text.contains("Fix typo"));
    }

    #[test]
    fn test_format_repo_view() {
        let info = json!({
            "fullName": "test/repo",
            "url": "https://github.com/test/repo",
            "description": "A test repo",
            "language": "Rust",
            "stargazersCount": 1500,
            "forksCount": 100,
            "defaultBranchRef": {"name": "main"},
            "repositoryTopics": [{"name": "test"}],
            "licenseInfo": {"spdxId": "MIT"}
        });
        let text = format_repo_view(&info);
        assert!(text.contains("**test/repo**"));
        assert!(text.contains("1.5k"));
        assert!(text.contains("MIT"));
    }

    #[test]
    fn test_format_run_list() {
        let items = vec![json!({
            "databaseId": 12345,
            "name": "CI",
            "status": "completed",
            "conclusion": "success",
            "headBranch": "main"
        })];
        let text = format_run_list(&items);
        assert!(text.contains("CI"));
        assert!(text.contains("success"));
    }

    #[test]
    fn test_schema() {
        let cache = Arc::new(SearchCache::new());
        let tool = GitHubTool::new(cache);
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["query"].is_object());
        assert_eq!(tool.name(), "github");
    }
}
