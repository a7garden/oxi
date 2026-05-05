//! Subagent tool — delegate tasks to specialized agents
//!
//! Spawns a separate `oxi --mode json` process for each invocation,
//! giving it an isolated context window.
//!
//! Supports three modes:
//!   - Single: { agent: "name", task: "..." }
//!   - Parallel: { tasks: [{ agent, task }, ...] }
//!   - Chain: { chain: [{ agent, task: "... {previous} ..." }, ...] }
//!
//! Agent definitions are markdown files with YAML frontmatter,
//! discovered from `~/.oxi/agents/` (user) and `.oxi/agents/` (project).

use super::{AgentTool, AgentToolResult, ToolError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

// ── Constants ──────────────────────────────────────────────────────────

const MAX_PARALLEL_TASKS: usize = 8;
const MAX_CONCURRENCY: usize = 4;

// ── Agent Discovery ────────────────────────────────────────────────────

/// Agent scope for discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentScope {
    /// Only user-level agents (~/.oxi/agents/)
    User,
    /// Only project-level agents (.oxi/agents/)
    Project,
    /// Both user and project agents
    Both,
}

impl Default for AgentScope {
    fn default() -> Self {
        Self::User
    }
}

/// A discovered agent definition.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub system_prompt: String,
    pub source: String, // "user" or "project"
}

/// Discover agents from user and/or project directories.
pub fn discover_agents(cwd: &Path, scope: AgentScope) -> Vec<AgentConfig> {
    let mut agents = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // User-level agents
    if scope == AgentScope::User || scope == AgentScope::Both {
        if let Some(home) = dirs::home_dir() {
            let user_dir = home.join(".oxi").join("agents");
            load_agents_from_dir(&user_dir, "user", &mut agents, &mut seen_names);
        }
    }

    // Project-level agents
    if scope == AgentScope::Project || scope == AgentScope::Both {
        let project_dir = cwd.join(".oxi").join("agents");
        load_agents_from_dir(&project_dir, "project", &mut agents, &mut seen_names);
    }

    agents
}

fn load_agents_from_dir(
    dir: &Path,
    source: &str,
    agents: &mut Vec<AgentConfig>,
    seen: &mut std::collections::HashSet<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if name.is_empty() || seen.contains(&name) {
            continue;
        }

        match parse_agent_file(&path) {
            Ok(config) => {
                seen.insert(name.clone());
                let mut config = config;
                config.source = source.to_string();
                agents.push(config);
            }
            Err(e) => {
                tracing::warn!("Failed to parse agent {}: {}", path.display(), e);
            }
        }
    }
}

/// Parse an agent markdown file with optional YAML frontmatter.
///
/// Format:
/// ```markdown
/// ---
/// name: my-agent
/// description: What this agent does
/// tools: read, grep, find, ls
/// model: claude-haiku-4-5
/// ---
/// System prompt goes here.
/// ```
fn parse_agent_file(path: &Path) -> Result<AgentConfig, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read: {}", e))?;

    let (frontmatter, body) = parse_frontmatter(&content);

    let name = frontmatter
        .get("name")
        .cloned()
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    let description = frontmatter
        .get("description")
        .cloned()
        .unwrap_or_default();

    let model = frontmatter.get("model").cloned();

    let tools = frontmatter.get("tools").map(|s| {
        s.split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    });

    Ok(AgentConfig {
        name,
        description,
        model,
        tools,
        system_prompt: body.trim().to_string(),
        source: String::new(), // Set by caller
    })
}

/// Parse YAML frontmatter from markdown content.
/// Returns (frontmatter key-value pairs, remaining body).
fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();

    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (map, content.to_string());
    }

    let after_first = &trimmed[3..];
    if let Some(end_idx) = after_first.find("\n---") {
        let yaml = &after_first[..end_idx];
        let body = after_first[end_idx + 4..].to_string();

        for line in yaml.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                map.insert(key, value);
            }
        }

        return (map, body);
    }

    (map, content.to_string())
}

// ── Result Types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub turns: u32,
}

impl Default for UsageStats {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            cost: 0.0,
            turns: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SingleResult {
    pub agent: String,
    pub agent_source: String,
    pub task: String,
    pub exit_code: i32,
    pub output: String,
    pub stderr: String,
    pub usage: UsageStats,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub error_message: Option<String>,
    pub step: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentMode {
    Single,
    Parallel,
    Chain,
}

#[derive(Debug, Clone)]
pub struct SubagentDetails {
    pub mode: SubagentMode,
    pub results: Vec<SingleResult>,
}

// ── Process Execution ──────────────────────────────────────────────────

/// Run a single agent process.
async fn run_single_agent(
    cwd: &Path,
    agents: &[AgentConfig],
    agent_name: &str,
    task: &str,
    agent_cwd: Option<&str>,
    step: Option<usize>,
    signal: Option<oneshot::Receiver<()>>,
) -> SingleResult {
    let agent = match agents.iter().find(|a| a.name == agent_name) {
        Some(a) => a,
        None => {
            let available = agents
                .iter()
                .map(|a| format!("\"{}\"", a.name))
                .collect::<Vec<_>>()
                .join(", ");
            return SingleResult {
                agent: agent_name.to_string(),
                agent_source: "unknown".to_string(),
                task: task.to_string(),
                exit_code: 1,
                output: String::new(),
                stderr: format!(
                    "Unknown agent: \"{}\". Available: {}",
                    agent_name, available
                ),
                usage: UsageStats::default(),
                model: None,
                stop_reason: None,
                error_message: Some(format!("Unknown agent: {}", agent_name)),
                step,
            };
        }
    };

    let mut result = SingleResult {
        agent: agent_name.to_string(),
        agent_source: agent.source.clone(),
        task: task.to_string(),
        exit_code: 0,
        output: String::new(),
        stderr: String::new(),
        usage: UsageStats::default(),
        model: agent.model.clone(),
        stop_reason: None,
        error_message: None,
        step,
    };

    // Build command
    let mut args = vec![
        "--mode".to_string(),
        "json".to_string(),
        "-p".to_string(),
        "--no-session".to_string(),
    ];

    if let Some(ref model) = agent.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }

    // Write system prompt to temp file if present
    let tmp_dir = std::env::temp_dir().join(format!("oxi-subagent-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let tmp_prompt_path = tmp_dir.join("system_prompt.md");
    if !agent.system_prompt.is_empty() {
        if std::fs::write(&tmp_prompt_path, &agent.system_prompt).is_ok() {
            args.push("--append-system-prompt".to_string());
            args.push(
                tmp_prompt_path
                    .to_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }

    // The task is the final argument
    args.push(format!("Task: {}", task));

    let working_dir = agent_cwd
        .map(|p| PathBuf::from(p))
        .unwrap_or_else(|| cwd.to_path_buf());

    let mut cmd = Command::new("oxi");
    cmd.args(&args)
        .current_dir(&working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            result.exit_code = 1;
            result.stderr = format!("Failed to spawn oxi: {}", e);
            result.error_message = Some(format!("Failed to spawn oxi: {}", e));
            return result;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Read stdout line by line (JSON events)
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    // Read stderr in background
    let stderr_handle = tokio::spawn(async move {
        let mut err = String::new();
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            err.push_str(&line);
            err.push('\n');
        }
        err
    });

    // Process JSON events
    let mut final_text = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let event: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = event["type"].as_str().unwrap_or("");

        match event_type {
            "text_delta" => {
                if let Some(text) = event["text"].as_str() {
                    final_text.push_str(text);
                }
            }
            "complete" => {
                result.stop_reason = Some("complete".to_string());
            }
            "error" => {
                let msg = event["message"]
                    .as_str()
                    .unwrap_or("Unknown error");
                result.error_message = Some(msg.to_string());
                result.stop_reason = Some("error".to_string());
            }
            _ => {}
        }
    }

    result.output = final_text;

    // Collect stderr
    if let Ok(err_output) = stderr_handle.await {
        result.stderr = err_output;
    }

    // Wait for process exit
    match child.wait().await {
        Ok(status) => {
            result.exit_code = status.code().unwrap_or(1);
        }
        Err(_) => {
            result.exit_code = 1;
        }
    }

    result
}

/// Run multiple tasks with concurrency limit.
async fn run_parallel(
    cwd: &Path,
    agents: &[AgentConfig],
    tasks: Vec<ParallelTask>,
    _signal: Option<oneshot::Receiver<()>>,
) -> Vec<SingleResult> {
    let n = tasks.len();
    if n == 0 {
        return vec![];
    }

    let limit = MAX_CONCURRENCY.min(n);
    let indexed_tasks: Vec<(usize, ParallelTask)> = tasks.into_iter().enumerate().collect();
    let mut all_results: Vec<Option<SingleResult>> = vec![None; n];

    for chunk in indexed_tasks.chunks(limit) {
        let mut handles = Vec::new();

        for (idx, task) in chunk {
            let idx = *idx;
            let agents = agents.to_vec();
            let cwd = cwd.to_path_buf();

            handles.push(tokio::spawn(async move {
                run_single_agent(
                    &cwd,
                    &agents,
                    &task.agent,
                    &task.task,
                    task.cwd.as_deref(),
                    None,
                    None,
                )
                .await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            if let Ok(result) = handle.await {
                all_results[chunk[i].0] = Some(result);
            }
        }
    }

    all_results
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.unwrap_or_else(|| SingleResult {
                agent: "unknown".to_string(),
                agent_source: "unknown".to_string(),
                task: "unknown".to_string(),
                exit_code: 1,
                output: String::new(),
                stderr: "Task did not complete".to_string(),
                usage: UsageStats::default(),
                model: None,
                stop_reason: Some("error".to_string()),
                error_message: Some("Task did not complete".to_string()),
                step: None,
            })
        })
        .collect()
}

// ── Parameter Types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ParallelTask {
    agent: String,
    task: String,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChainStep {
    agent: String,
    task: String,
    #[serde(default)]
    cwd: Option<String>,
}

// ── Tool Implementation ────────────────────────────────────────────────

pub struct SubagentTool {
    cwd: PathBuf,
}

impl SubagentTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }
}

#[async_trait]
impl AgentTool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn label(&self) -> &str {
        "Subagent"
    }

    fn description(&self) -> &str {
        "Delegate tasks to specialized subagents with isolated context. \
         Modes: single (agent + task), parallel (tasks array), chain (sequential with {previous} placeholder). \
         Agents are discovered from ~/.oxi/agents/ (user) and .oxi/agents/ (project)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "description": "Agent name for single mode"
                },
                "task": {
                    "type": "string",
                    "description": "Task to delegate (single mode)"
                },
                "tasks": {
                    "type": "array",
                    "description": "Array of {agent, task} for parallel execution (max 8)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent": { "type": "string" },
                            "task": { "type": "string" },
                            "cwd": { "type": "string" }
                        },
                        "required": ["agent", "task"]
                    }
                },
                "chain": {
                    "type": "array",
                    "description": "Array of {agent, task} for sequential execution. Use {previous} in task for prior output.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent": { "type": "string" },
                            "task": { "type": "string" },
                            "cwd": { "type": "string" }
                        },
                        "required": ["agent", "task"]
                    }
                },
                "agentScope": {
                    "type": "string",
                    "description": "Agent discovery scope: 'user' (default), 'project', or 'both'",
                    "enum": ["user", "project", "both"],
                    "default": "user"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for single mode"
                }
            }
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, ToolError> {
        let scope: AgentScope = params
            .get("agentScope")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(AgentScope::User);

        let agents = discover_agents(&self.cwd, scope);

        let has_chain = params["chain"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
        let has_tasks = params["tasks"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
        let has_single = params["agent"].is_string() && params["task"].is_string();

        let mode_count = [has_chain, has_tasks, has_single]
            .iter()
            .filter(|&&x| x)
            .count();

        if mode_count != 1 {
            let available = agents
                .iter()
                .map(|a| format!("{} ({})", a.name, a.source))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(AgentToolResult::error(format!(
                "Provide exactly one mode: agent+task, tasks, or chain.\nAvailable agents: {}",
                if available.is_empty() {
                    "none".to_string()
                } else {
                    available
                }
            )));
        }

        // ── Chain mode ──
        if has_chain {
            let steps: Vec<ChainStep> = serde_json::from_value(params["chain"].clone())
                .map_err(|e| format!("Invalid chain parameter: {}", e))?;

            let mut results = Vec::new();
            let mut previous_output = String::new();

            for (i, step) in steps.into_iter().enumerate() {
                let task = step.task.replace("{previous}", &previous_output);

                let result = run_single_agent(
                    &self.cwd,
                    &agents,
                    &step.agent,
                    &task,
                    step.cwd.as_deref(),
                    Some(i + 1),
                    None,
                )
                .await;

                let is_error = result.exit_code != 0
                    || result.stop_reason.as_deref() == Some("error")
                    || result.stop_reason.as_deref() == Some("aborted");

                if is_error {
                    let agent_name = result.agent.clone();
                    let error_msg = result
                        .error_message
                        .clone()
                        .unwrap_or_else(|| result.stderr.clone());
                    results.push(result);
                    let mut output = format!(
                        "Chain stopped at step {} ({}): {}",
                        i + 1,
                        agent_name,
                        error_msg
                    );
                    if !previous_output.is_empty() {
                        output.push_str(&format!(
                            "\n\nPrevious output:\n{}",
                            truncate_output(&previous_output, 500)
                        ));
                    }
                    return Ok(AgentToolResult::error(output));
                }

                previous_output = result.output.clone();
                results.push(result);
            }

            let output = results
                .last()
                .map(|r| r.output.clone())
                .unwrap_or_default();

            return Ok(AgentToolResult::success(if output.is_empty() {
                "(no output)".to_string()
            } else {
                output
            })
            .with_metadata(json!({
                "mode": "chain",
                "steps": results.len(),
                "results": results.iter().map(|r| json!({
                    "agent": r.agent,
                    "source": r.agent_source,
                    "exit_code": r.exit_code,
                    "step": r.step,
                })).collect::<Vec<_>>()
            })));
        }

        // ── Parallel mode ──
        if has_tasks {
            let tasks: Vec<ParallelTask> = serde_json::from_value(params["tasks"].clone())
                .map_err(|e| format!("Invalid tasks parameter: {}", e))?;

            if tasks.len() > MAX_PARALLEL_TASKS {
                return Ok(AgentToolResult::error(format!(
                    "Too many parallel tasks ({}). Max is {}.",
                    tasks.len(),
                    MAX_PARALLEL_TASKS
                )));
            }

            let results = run_parallel(&self.cwd, &agents, tasks, None).await;

            let success_count = results.iter().filter(|r| r.exit_code == 0).count();
            let summaries: Vec<String> = results
                .iter()
                .map(|r| {
                    let preview = truncate_output(&r.output, 100);
                    format!(
                        "[{}] {}: {}",
                        r.agent,
                        if r.exit_code == 0 {
                            "completed"
                        } else {
                            "failed"
                        },
                        if preview.is_empty() {
                            "(no output)"
                        } else {
                            &preview
                        }
                    )
                })
                .collect();

            return Ok(AgentToolResult::success(format!(
                "Parallel: {}/{} succeeded\n\n{}",
                success_count,
                results.len(),
                summaries.join("\n\n")
            ))
            .with_metadata(json!({
                "mode": "parallel",
                "results": results.iter().map(|r| json!({
                    "agent": r.agent,
                    "source": r.agent_source,
                    "exit_code": r.exit_code,
                })).collect::<Vec<_>>()
            })));
        }

        // ── Single mode ──
        if has_single {
            let agent_name = params["agent"].as_str().unwrap();
            let task = params["task"].as_str().unwrap();
            let agent_cwd = params["cwd"].as_str();

            let result = run_single_agent(
                &self.cwd,
                &agents,
                agent_name,
                task,
                agent_cwd,
                None,
                None,
            )
            .await;

            let is_error = result.exit_code != 0
                || result.stop_reason.as_deref() == Some("error")
                || result.stop_reason.as_deref() == Some("aborted");

            if is_error {
                let error_msg = result
                    .error_message
                    .as_deref()
                    .unwrap_or(&result.stderr);
                return Ok(AgentToolResult::error(format!(
                    "Agent {}: {}",
                    result.stop_reason.as_deref().unwrap_or("failed"),
                    error_msg
                )));
            }

            return Ok(AgentToolResult::success(if result.output.is_empty() {
                "(no output)".to_string()
            } else {
                result.output.clone()
            })
            .with_metadata(json!({
                "mode": "single",
                "agent": result.agent,
                "source": result.agent_source,
            })));
        }

        Ok(AgentToolResult::error("Invalid parameters".to_string()))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn truncate_output(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}...", &text[..max_chars])
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_with_yaml() {
        let content = "---\nname: scout\ndescription: Fast recon\nmodel: haiku\ntools: read, grep\n---\nYou are a scout agent.";
        let (fm, body) = parse_frontmatter(content);
        assert_eq!(fm.get("name").unwrap(), "scout");
        assert_eq!(fm.get("description").unwrap(), "Fast recon");
        assert_eq!(fm.get("model").unwrap(), "haiku");
        assert_eq!(fm.get("tools").unwrap(), "read, grep");
        assert!(body.trim().starts_with("You are a scout agent."));
    }

    #[test]
    fn test_parse_frontmatter_no_yaml() {
        let content = "Just a plain system prompt.";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.is_empty());
        assert_eq!(body.trim(), "Just a plain system prompt.");
    }

    #[test]
    fn test_parse_agent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("scout.md");
        std::fs::write(
            &file_path,
            "---\nname: scout\ndescription: Fast recon\n---\nYou are a scout.",
        )
        .unwrap();

        let config = parse_agent_file(&file_path).unwrap();
        assert_eq!(config.name, "scout");
        assert_eq!(config.description, "Fast recon");
        assert_eq!(config.system_prompt, "You are a scout.");
    }

    #[test]
    fn test_parse_agent_file_no_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("worker.md");
        std::fs::write(&file_path, "You are a worker agent.").unwrap();

        let config = parse_agent_file(&file_path).unwrap();
        assert_eq!(config.name, "worker");
        assert_eq!(config.system_prompt, "You are a worker agent.");
    }

    #[test]
    fn test_discover_agents_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let agents = discover_agents(tmp.path(), AgentScope::User);
        assert!(agents.is_empty());
    }

    #[test]
    fn test_discover_agents_with_files() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join(".oxi").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        std::fs::write(
            agents_dir.join("scout.md"),
            "---\nname: scout\ndescription: Recon\n---\nBe a scout.",
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("worker.md"),
            "---\nname: worker\n---\nBe a worker.",
        )
        .unwrap();
        // Non-md file should be ignored
        std::fs::write(agents_dir.join("ignore.txt"), "ignore me").unwrap();

        let agents = discover_agents(tmp.path(), AgentScope::Project);
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().any(|a| a.name == "scout"));
        assert!(agents.iter().any(|a| a.name == "worker"));
    }

    #[test]
    fn test_agent_scope_default() {
        assert_eq!(AgentScope::default(), AgentScope::User);
    }

    #[test]
    fn test_tools_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("agent.md");
        std::fs::write(
            &file_path,
            "---\ntools: read, grep, find, ls\n---\nSystem prompt.",
        )
        .unwrap();

        let config = parse_agent_file(&file_path).unwrap();
        let tools = config.tools.unwrap();
        assert_eq!(tools, vec!["read", "grep", "find", "ls"]);
    }

    #[test]
    fn test_schema_structure() {
        let tool = SubagentTool::new(".");
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent"].is_object());
        assert!(schema["properties"]["tasks"].is_object());
        assert!(schema["properties"]["chain"].is_object());
        assert!(schema["properties"]["agentScope"].is_object());
    }

    #[test]
    fn test_truncate_output() {
        assert_eq!(truncate_output("hello", 10), "hello");
        assert_eq!(truncate_output("hello world foo", 5), "hello...");
    }
}
