/// Subagent tool — delegate tasks to specialized agents
/// Spawns a separate `oxi --mode json` process for each invocation,
/// giving it an isolated context window.
/// Supports three modes:
///   - Single: { agent: "name", task: "..." }
///   - Parallel: { tasks: [{ agent, task }, ...] }
///   - Chain: { chain: [{ agent, task: "... {previous} ..." }, ...] }
///     Agent definitions are markdown files with YAML frontmatter,
///     discovered from `~/.oxi/agents/` (user) and `.oxi/agents/` (project).
use super::{AgentTool, AgentToolResult, ProgressCallback, ToolContext, ToolError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

// ── Constants ──────────────────────────────────────────────────────────

const MAX_PARALLEL_TASKS: usize = 8;
const MAX_CONCURRENCY: usize = 4;

// ── Progress callback type (reuse from tools.rs) ──────────────────────

type ProgressFn = ProgressCallback;

// ── Temp dir helper (no RAII — let OS clean up after subprocess exits) ──

fn create_system_prompt_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).map_err(|e| format!("Failed to create temp dir: {}", e))?;
    Ok(path)
}

// ── Agent Discovery ────────────────────────────────────────────────────

/// Agent scope for discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AgentScope {
    /// Only user-level agents (~/.oxi/agents/)
    #[default]
    User,
    /// Only project-level agents (.oxi/agents/)
    Project,
    /// Both user and project agents
    Both,
}

/// A discovered agent definition.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// pub.
    pub name: String,
    /// pub.
    pub description: String,
    /// pub.
    pub model: Option<String>,
    /// pub.
    pub tools: Option<Vec<String>>,
    /// pub.
    pub system_prompt: String,
    /// pub.
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

    // Project-level agents (walk up to .git boundary)
    if scope == AgentScope::Project || scope == AgentScope::Both {
        if let Some(project_dir) = find_project_agents_dir(cwd) {
            load_agents_from_dir(&project_dir, "project", &mut agents, &mut seen_names);
        }
    }

    agents
}

/// Walk up from `cwd` to find `.oxi/agents/`.
/// Stops at `.git` boundary (project root). Returns None if not found.
fn find_project_agents_dir(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let candidate = current.join(".oxi").join("agents");
        if candidate.is_dir() {
            return Some(candidate);
        }
        // .git marks project root — don't go higher
        if current.join(".git").exists() {
            return None;
        }
        current = current.parent()?;
    }
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
fn parse_agent_file(path: &Path) -> Result<AgentConfig, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read: {}", e))?;

    let (frontmatter, body) = parse_frontmatter(&content);

    let name = frontmatter.get("name").cloned().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let description = frontmatter.get("description").cloned().unwrap_or_default();

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
        source: String::new(),
    })
}

/// Parse YAML frontmatter from markdown content.
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
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        return (map, body);
    }
    (map, content.to_string())
}

// ── Result Types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
/// UsageStats.
pub struct UsageStats {
    /// pub.
    pub input_tokens: u64,
    /// pub.
    pub output_tokens: u64,
    /// pub.
    pub cache_read: u64,
    /// pub.
    pub cache_write: u64,
    /// pub.
    pub cost: f64,
    /// pub.
    pub turns: u32,
}

#[derive(Debug, Clone)]
/// SingleResult.
pub struct SingleResult {
    /// pub.
    pub agent: String,
    /// pub.
    pub agent_source: String,
    /// pub.
    pub task: String,
    /// pub.
    pub exit_code: i32,
    /// pub.
    pub output: String,
    /// pub.
    pub stderr: String,
    /// pub.
    pub usage: UsageStats,
    /// pub.
    pub model: Option<String>,
    /// pub.
    pub stop_reason: Option<String>,
    /// pub.
    pub error_message: Option<String>,
    /// pub.
    pub step: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// SubagentMode.
pub enum SubagentMode {
    /// single variant.
    Single,
    /// parallel variant.
    Parallel,
    /// chain variant.
    Chain,
}

#[derive(Debug, Clone)]
/// SubagentDetails.
pub struct SubagentDetails {
    /// pub.
    pub mode: SubagentMode,
    /// pub.
    pub results: Vec<SingleResult>,
}

// ── JSON line processing ───────────────────────────────────────────────

fn process_json_line(
    line: &str,
    result: &mut SingleResult,
    text: &mut String,
    _on_progress: &Option<ProgressFn>,
) {
    let event: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return,
    };
    match event["type"].as_str().unwrap_or("") {
        "text_delta" => {
            if let Some(t) = event["text"].as_str() {
                text.push_str(t);
            }
        }
        "usage" => {
            result.usage.input_tokens += event["input_tokens"].as_u64().unwrap_or(0);
            result.usage.output_tokens += event["output_tokens"].as_u64().unwrap_or(0);
            result.usage.turns += 1;
        }
        "complete" => {
            result.stop_reason = Some("complete".to_string());
        }
        "error" => {
            result.error_message = Some(
                event["message"]
                    .as_str()
                    .unwrap_or("Unknown error")
                    .to_string(),
            );
            result.stop_reason = Some("error".to_string());
        }
        _ => {}
    }
}

// ── Process Execution ──────────────────────────────────────────────────

/// Build command-line arguments for launching a subagent process.
fn build_agent_args(agent: &AgentConfig, tmp_dir: &Path, task: &str) -> Vec<String> {
    let mut args = vec!["--mode".to_string(), "json".to_string(), "-p".to_string()];

    if let Some(ref model) = agent.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }

    if let Some(ref agent_tools) = agent.tools {
        if !agent_tools.is_empty() {
            args.push("--tools".to_string());
            args.push(agent_tools.join(","));
        }
    }

    if !agent.system_prompt.is_empty()
        && std::fs::write(tmp_dir.join("system_prompt.md"), &agent.system_prompt).is_ok()
    {
        args.push("--append-system-prompt".to_string());
        args.push(
            tmp_dir
                .join("system_prompt.md")
                .to_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    args.push(format!("Task: {}", task));
    args
}

/// Gracefully terminate a child process (SIGTERM → wait → SIGKILL).
async fn terminate_child(
    child: &mut tokio::process::Child,
    stderr_handle: tokio::task::JoinHandle<String>,
    result: &mut SingleResult,
) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        let deadline = tokio::time::sleep(std::time::Duration::from_secs(5));
        tokio::pin!(deadline);
        tokio::select! {
            _ = &mut deadline => { let _ = child.start_kill(); }
            _ = child.wait() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
    }

    // Collect stderr with short timeout
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        if let Ok(err) = stderr_handle.await {
            result.stderr = err;
        }
    })
    .await;
}

/// Run a single agent process with abort support.
#[allow(clippy::too_many_arguments)]
async fn run_single_agent(
    cwd: &Path,
    agents: &[AgentConfig],
    agent_name: &str,
    task: &str,
    agent_cwd: Option<&str>,
    step: Option<usize>,
    signal: Option<oneshot::Receiver<()>>,
    on_progress: Option<ProgressFn>,
    binary_path: &Path,
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

    // Notify progress
    if let Some(ref cb) = on_progress {
        cb(format!("[{}] running...", agent_name));
    }

    // Build command args
    let tmp_dir = match create_system_prompt_temp_dir("oxi-subagent") {
        Ok(tmp) => Some(tmp),
        Err(e) => {
            result.exit_code = 1;
            result.stderr = e.clone();
            result.error_message = Some(e);
            return result;
        }
    };

    let args = match tmp_dir {
        Some(ref tmp) => build_agent_args(agent, tmp, task),
        None => vec![
            "--mode".to_string(),
            "json".to_string(),
            "-p".to_string(),
            format!("Task: {}", task),
        ],
    };

    let working_dir = agent_cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());

    let mut cmd = tokio::process::Command::new(binary_path);
    cmd.args(&args)
        .current_dir(&working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            result.exit_code = 1;
            result.stderr = format!("Failed to spawn: {}", e);
            result.error_message = Some(format!("Failed to spawn: {}", e));
            return result;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped but missing");
    let stderr = child.stderr.take().expect("stderr piped but missing");

    // Spawn stdout reader → channel
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let _reader_handle = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    // Spawn stderr reader
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

    // Main loop: select between stdout lines and abort signal
    let mut final_text = String::new();
    let mut signal_rx = signal;
    let mut aborted = false;

    loop {
        tokio::select! {
            line = line_rx.recv() => {
                match line {
                    Some(line) => {
                        process_json_line(&line, &mut result, &mut final_text, &on_progress);
                    }
                    None => break, // stdout EOF
                }
            }
            _ = async {
                match &mut signal_rx {
                    Some(rx) => { let _ = rx.await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                aborted = true;
                break;
            }
        }
    }

    if aborted {
        result.stop_reason = Some("aborted".into());
        result.error_message = Some("Aborted by user".into());
        terminate_child(&mut child, stderr_handle, &mut result).await;
    } else {
        // Normal completion
        if let Ok(err_output) = stderr_handle.await {
            result.stderr = err_output;
        }
        match child.wait().await {
            Ok(status) => result.exit_code = status.code().unwrap_or(1),
            Err(_) => result.exit_code = 1,
        }
    }

    result.output = final_text;

    if let Some(ref cb) = on_progress {
        let status = if result.exit_code == 0 {
            "done"
        } else {
            "failed"
        };
        cb(format!("[{}] {}", agent_name, status));
    }

    result
}

/// Run multiple tasks with concurrency limit.
async fn run_parallel(
    cwd: &Path,
    agents: &[AgentConfig],
    tasks: Vec<ParallelTask>,
    binary_path: PathBuf,
    on_progress: Option<ProgressFn>,
) -> Vec<SingleResult> {
    let n = tasks.len();
    if n == 0 {
        return vec![];
    }

    let limit = MAX_CONCURRENCY.min(n);
    let indexed_tasks: Vec<(usize, ParallelTask)> = tasks.into_iter().enumerate().collect();
    let mut all_results: Vec<Option<SingleResult>> = vec![None; n];

    let mut i = 0;
    while i < indexed_tasks.len() {
        let end = (i + limit).min(indexed_tasks.len());
        let chunk: Vec<_> = indexed_tasks[i..end].to_vec();
        let mut handles = Vec::new();

        for (idx, task) in chunk {
            let agents = agents.to_vec();
            let cwd = cwd.to_path_buf();
            let bp = binary_path.clone();
            let progress = on_progress.clone();

            handles.push((
                idx,
                tokio::spawn(async move {
                    run_single_agent(
                        &cwd,
                        &agents,
                        &task.agent,
                        &task.task,
                        task.cwd.as_deref(),
                        None,
                        None,
                        progress,
                        &bp,
                    )
                    .await
                }),
            ));
        }

        for (idx, handle) in handles {
            if let Ok(r) = handle.await {
                all_results[idx] = Some(r);
            }
        }

        i = end;
    }

    all_results
        .into_iter()
        .map(|r| {
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

#[derive(Debug, Deserialize, Clone)]
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

/// SubagentTool.
pub struct SubagentTool {
    /// Explicit working directory override. If None, uses ToolContext.root() at runtime.
    cwd: Option<PathBuf>,
    binary_path: Option<PathBuf>,
    progress_callback: parking_lot::Mutex<Option<ProgressFn>>,
}

impl Default for SubagentTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTool {
    /// Create with no explicit root (uses ToolContext.root() at runtime).
    pub fn new() -> Self {
        Self {
            cwd: None,
            binary_path: None,
            progress_callback: parking_lot::Mutex::new(None),
        }
    }

    /// Create with an explicit working directory (overrides ToolContext).
    pub fn with_cwd(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: Some(cwd.into()),
            binary_path: None,
            progress_callback: parking_lot::Mutex::new(None),
        }
    }

    fn get_binary(&self) -> PathBuf {
        self.binary_path
            .clone()
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("oxi"))
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

    fn on_progress(&self, callback: ProgressCallback) {
        *self.progress_callback.lock() = Some(callback);
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // Use explicit cwd if set, else ctx.root()
        let effective_cwd = self.cwd.as_deref().unwrap_or(ctx.root());

        let scope: AgentScope = params
            .get("agentScope")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(AgentScope::User);

        let agents = discover_agents(effective_cwd, scope);
        let binary = self.get_binary();
        let progress = self.progress_callback.lock().clone();

        let has_chain = params["chain"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let has_tasks = params["tasks"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false);
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
            return execute_chain_mode(effective_cwd, &agents, params, &binary, progress, signal)
                .await;
        }

        // ── Parallel mode ──
        if has_tasks {
            return execute_parallel_mode(effective_cwd, &agents, params, &binary, progress).await;
        }

        // ── Single mode ──
        if has_single {
            return execute_single_mode(effective_cwd, &agents, params, &binary, progress, signal)
                .await;
        }

        Ok(AgentToolResult::error("Invalid parameters".to_string()))
    }
}

/// Execute chain mode: sequential agents where each step can reference {previous} output.
async fn execute_chain_mode(
    cwd: &Path,
    agents: &[AgentConfig],
    params: Value,
    binary: &Path,
    progress: Option<ProgressFn>,
    signal: Option<oneshot::Receiver<()>>,
) -> Result<AgentToolResult, ToolError> {
    let steps: Vec<ChainStep> = serde_json::from_value(params["chain"].clone())
        .map_err(|e| format!("Invalid chain parameter: {}", e))?;
    let total = steps.len();
    let mut results = Vec::new();
    let mut previous_output = String::new();
    let mut abort_signal = signal;

    for (i, step) in steps.into_iter().enumerate() {
        let task = step.task.replace("{previous}", &previous_output);
        let step_signal = if i == total - 1 {
            abort_signal.take()
        } else {
            None
        };

        let result = run_single_agent(
            cwd,
            agents,
            &step.agent,
            &task,
            step.cwd.as_deref(),
            Some(i + 1),
            step_signal,
            progress.clone(),
            binary,
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
            return Ok(AgentToolResult::error(format!(
                "Chain stopped at step {}/{} ({}): {}",
                i + 1,
                total,
                agent_name,
                error_msg
            )));
        }

        previous_output = result.output.clone();
        results.push(result);
    }

    let output = results.last().map(|r| r.output.clone()).unwrap_or_default();
    Ok(AgentToolResult::success(if output.is_empty() {
        "(no output)".to_string()
    } else {
        output
    })
    .with_metadata(json!({
        "mode": "chain",
        "steps": results.len(),
    })))
}

/// Execute parallel mode: multiple agents running concurrently.
async fn execute_parallel_mode(
    cwd: &Path,
    agents: &[AgentConfig],
    params: Value,
    binary: &Path,
    progress: Option<ProgressFn>,
) -> Result<AgentToolResult, ToolError> {
    let tasks: Vec<ParallelTask> = serde_json::from_value(params["tasks"].clone())
        .map_err(|e| format!("Invalid tasks parameter: {}", e))?;

    if tasks.len() > MAX_PARALLEL_TASKS {
        return Ok(AgentToolResult::error(format!(
            "Too many parallel tasks ({}). Max is {}.",
            tasks.len(),
            MAX_PARALLEL_TASKS
        )));
    }

    let results = run_parallel(cwd, agents, tasks, binary.to_path_buf(), progress).await;

    let success_count = results.iter().filter(|r| r.exit_code == 0).count();
    let summaries: Vec<String> = results
        .iter()
        .map(|r| {
            let _preview = truncate_output(&r.output, 100);
            format!(
                "[{}]: {}",
                r.agent,
                if r.exit_code == 0 {
                    "completed"
                } else {
                    "failed"
                },
            )
        })
        .collect();

    Ok(AgentToolResult::success(format!(
        "Parallel: {}/{} succeeded\n\n{}",
        success_count,
        results.len(),
        summaries.join("\n\n")
    ))
    .with_metadata(json!({
        "mode": "parallel",
        "results": results.iter().map(|r| json!({
            "agent": r.agent,
            "exit_code": r.exit_code,
        })).collect::<Vec<_>>()
    })))
}

/// Execute single mode: one agent, one task.
async fn execute_single_mode(
    cwd: &Path,
    agents: &[AgentConfig],
    params: Value,
    binary: &Path,
    progress: Option<ProgressFn>,
    signal: Option<oneshot::Receiver<()>>,
) -> Result<AgentToolResult, ToolError> {
    let agent_name = params["agent"]
        .as_str()
        .ok_or("Missing required parameter: agent")?;
    let task = params["task"]
        .as_str()
        .ok_or("Missing required parameter: task")?;
    let agent_cwd = params["cwd"].as_str();

    let result = run_single_agent(
        cwd, agents, agent_name, task, agent_cwd, None, signal, progress, binary,
    )
    .await;

    let is_error = result.exit_code != 0
        || result.stop_reason.as_deref() == Some("error")
        || result.stop_reason.as_deref() == Some("aborted");

    if is_error {
        let error_msg = result.error_message.as_deref().unwrap_or(&result.stderr);
        return Ok(AgentToolResult::error(format!(
            "Agent {}: {}",
            result.stop_reason.as_deref().unwrap_or("failed"),
            error_msg
        )));
    }

    Ok(AgentToolResult::success(if result.output.is_empty() {
        "(no output)".to_string()
    } else {
        result.output.clone()
    })
    .with_metadata(json!({
        "mode": "single",
        "agent": result.agent,
        "source": result.agent_source,
        "usage": {
            "input_tokens": result.usage.input_tokens,
            "output_tokens": result.usage.output_tokens,
            "turns": result.usage.turns,
        },
    })))
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
        std::fs::write(agents_dir.join("ignore.txt"), "ignore me").unwrap();
        let agents = discover_agents(tmp.path(), AgentScope::Project);
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().any(|a| a.name == "scout"));
        assert!(agents.iter().any(|a| a.name == "worker"));
    }

    #[test]
    fn test_find_project_agents_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join(".oxi").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let sub = tmp.path().join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        // From subdirectory, should walk up to find .oxi/agents
        assert_eq!(find_project_agents_dir(&sub), Some(agents_dir));
    }

    #[test]
    fn test_find_project_agents_dir_stops_at_git() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        // No .oxi/agents, .git exists → None
        assert_eq!(find_project_agents_dir(tmp.path()), None);
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
        let tool = SubagentTool::new();
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

    #[test]
    fn test_process_json_line_text_delta() {
        let mut result = SingleResult {
            agent: "test".into(),
            agent_source: "user".into(),
            task: "t".into(),
            exit_code: 0,
            output: String::new(),
            stderr: String::new(),
            usage: UsageStats::default(),
            model: None,
            stop_reason: None,
            error_message: None,
            step: None,
        };
        let mut text = String::new();
        process_json_line(
            r#"{"type":"text_delta","text":"hello"}"#,
            &mut result,
            &mut text,
            &None,
        );
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_process_json_line_usage() {
        let mut result = SingleResult {
            agent: "test".into(),
            agent_source: "user".into(),
            task: "t".into(),
            exit_code: 0,
            output: String::new(),
            stderr: String::new(),
            usage: UsageStats::default(),
            model: None,
            stop_reason: None,
            error_message: None,
            step: None,
        };
        let mut text = String::new();
        process_json_line(
            r#"{"type":"usage","input_tokens":100,"output_tokens":50}"#,
            &mut result,
            &mut text,
            &None,
        );
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 50);
        assert_eq!(result.usage.turns, 1);
    }
}
