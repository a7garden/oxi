//! Todo tool — phased task management with 7 ops.
//!
//! omp `tools/todo.ts` (938줄) 계약 이식:
//! - 7 ops: init, start, done, drop, rm, append, view
//! - 3상태 정규화 (in_progress는 한 phase에 하나)
//! - Markdown 라운드트립
//! - sub-agent 매칭 헬퍼 (⑥ 연동 후 활성화)

use std::fmt;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{AgentTool, AgentToolResult, ToolContext, ToolError};

// ── Types ─────────────────────────────────────────────────────────────

/// Status of a single todo task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task has not yet been started.
    Pending,
    /// Task is currently being worked on (at most one per phase after normalization).
    InProgress,
    /// Task has been finished.
    Completed,
    /// Task was cancelled or deemed unnecessary.
    Abandoned,
}

impl TodoStatus {
    /// Return a status-specific glyph for display.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Pending => "\u{2610}",    // ☐
            Self::InProgress => "\u{25B6}", // ▶
            Self::Completed => "\u{2611}",  // ☑
            Self::Abandoned => "\u{2717}",  // ✗
        }
    }

    /// Return the serialized snake_case name of this status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }
}

impl fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single task within a phase.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TodoItem {
    /// Human-readable description of the task.
    pub content: String,
    /// Current lifecycle status of the task.
    pub status: TodoStatus,
    /// Optional free-form notes attached to the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}

/// A named group of related tasks within a todo list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TodoPhase {
    /// Display name of the phase.
    pub name: String,
    /// Tasks belonging to this phase, in order.
    pub tasks: Vec<TodoItem>,
}

/// Operations that can be applied to a todo list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TodoOp {
    /// Initialize or replace the todo list.
    Init {
        /// Optional structured phase definitions.
        #[serde(default)]
        list: Option<Vec<InitListEntry>>,
        /// Optional flat list of task contents.
        #[serde(default)]
        items: Option<Vec<String>>,
    },
    /// Mark matching tasks as in progress.
    Start {
        /// Task content filter.
        #[serde(default)]
        task: Option<String>,
        /// Phase name filter.
        #[serde(default)]
        phase: Option<String>,
    },
    /// Mark matching tasks as completed.
    Done {
        /// Task content filter.
        #[serde(default)]
        task: Option<String>,
        /// Phase name filter.
        #[serde(default)]
        phase: Option<String>,
    },
    /// Mark matching tasks as abandoned.
    Drop {
        /// Task content filter.
        #[serde(default)]
        task: Option<String>,
        /// Phase name filter.
        #[serde(default)]
        phase: Option<String>,
    },
    /// Remove matching tasks entirely.
    Rm {
        /// Task content filter.
        #[serde(default)]
        task: Option<String>,
        /// Phase name filter.
        #[serde(default)]
        phase: Option<String>,
    },
    /// Append tasks to a phase, creating it if it does not exist.
    Append {
        /// Name of the target phase.
        phase: String,
        /// Task contents to append.
        items: Vec<String>,
    },
    /// Return the current state without modifying it.
    View,
}

/// A phase seed supplied to the `init` op.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InitListEntry {
    /// Display name of the phase.
    pub phase: String,
    /// Initial task contents for the phase.
    pub items: Vec<String>,
}

/// Describes a task that newly transitioned to completed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TodoCompletionTransition {
    /// Name of the phase containing the task.
    pub phase: String,
    /// Content of the completed task.
    pub content: String,
}

/// Result of applying a batch of todo ops.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TodoUpdateResult {
    /// Full phase list after the ops were applied.
    pub phases: Vec<TodoPhase>,
    /// Tasks that transitioned to completed during this update.
    pub completed_tasks: Vec<TodoCompletionTransition>,
    /// Non-fatal errors collected while applying the ops.
    pub errors: Vec<String>,
}

// ── Op dispatch (omp `applyEntry` 계약) ─────────────────────────────

/// Apply a single op to the phases vec. Errors are collected, not fatal.
fn apply_entry(phases: &mut Vec<TodoPhase>, op: &TodoOp, errors: &mut Vec<String>) {
    match op {
        TodoOp::Init { list, items } => {
            *phases = init_phases(list.as_deref(), items.as_deref(), errors);
        }
        TodoOp::Start { task, phase } => {
            let targets = resolve_targets(phases, task.as_deref(), phase.as_deref(), errors);
            for (phase_idx, task_idx) in targets {
                phases[phase_idx].tasks[task_idx].status = TodoStatus::InProgress;
            }
        }
        TodoOp::Done { task, phase } => {
            transition_status(
                phases,
                task.as_deref(),
                phase.as_deref(),
                TodoStatus::Completed,
                errors,
            );
        }
        TodoOp::Drop { task, phase } => {
            transition_status(
                phases,
                task.as_deref(),
                phase.as_deref(),
                TodoStatus::Abandoned,
                errors,
            );
        }
        TodoOp::Rm { task, phase } => {
            remove_tasks(phases, task.as_deref(), phase.as_deref(), errors);
        }
        TodoOp::Append { phase, items } => {
            append_items(phases, phase, items);
        }
        TodoOp::View => {} // read-only
    }
}

const DEFAULT_INIT_PHASE: &str = "Tasks";

fn init_phases(
    list: Option<&[InitListEntry]>,
    items: Option<&[String]>,
    errors: &mut Vec<String>,
) -> Vec<TodoPhase> {
    if let Some(list) = list {
        list.iter()
            .map(|entry| TodoPhase {
                name: entry.phase.clone(),
                tasks: entry
                    .items
                    .iter()
                    .map(|c| TodoItem {
                        content: c.clone(),
                        status: TodoStatus::Pending,
                        notes: None,
                    })
                    .collect(),
            })
            .collect()
    } else if let Some(items) = items {
        vec![TodoPhase {
            name: DEFAULT_INIT_PHASE.into(),
            tasks: items
                .iter()
                .map(|c| TodoItem {
                    content: c.clone(),
                    status: TodoStatus::Pending,
                    notes: None,
                })
                .collect(),
        }]
    } else {
        errors.push("init requires either 'list' or 'items'".into());
        Vec::new()
    }
}

fn resolve_targets(
    phases: &[TodoPhase],
    task: Option<&str>,
    phase: Option<&str>,
    errors: &mut Vec<String>,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (pi, p) in phases.iter().enumerate() {
        if phase.is_some_and(|phase_name| p.name != phase_name) {
            continue;
        }
        for (ti, t) in p.tasks.iter().enumerate() {
            if task.is_some_and(|task_content| t.content != task_content) {
                continue;
            }
            out.push((pi, ti));
        }
    }
    if out.is_empty() {
        let target = match (phase, task) {
            (Some(p), Some(t)) => format!("phase '{}' task '{}'", p, t),
            (Some(p), None) => format!("phase '{}'", p),
            (None, Some(t)) => format!("task '{}'", t),
            (None, None) => "any task".to_string(),
        };
        errors.push(format!("No matching {} found", target));
    }
    out
}

fn transition_status(
    phases: &mut [TodoPhase],
    task: Option<&str>,
    phase: Option<&str>,
    new_status: TodoStatus,
    errors: &mut Vec<String>,
) {
    let targets = resolve_targets(phases, task, phase, errors);
    for (pi, ti) in targets {
        phases[pi].tasks[ti].status = new_status;
    }
}

fn append_items(phases: &mut Vec<TodoPhase>, phase_name: &str, items: &[String]) {
    let phase = if let Some(p) = phases.iter_mut().find(|p| p.name == phase_name) {
        p
    } else {
        phases.push(TodoPhase {
            name: phase_name.into(),
            tasks: Vec::new(),
        });
        match phases.last_mut() {
            Some(last) => last,
            None => return,
        }
    };
    for content in items {
        phase.tasks.push(TodoItem {
            content: content.clone(),
            status: TodoStatus::Pending,
            notes: None,
        });
    }
}

fn remove_tasks(
    phases: &mut Vec<TodoPhase>,
    task: Option<&str>,
    phase: Option<&str>,
    errors: &mut Vec<String>,
) {
    if task.is_none() && phase.is_none() {
        // 둘 다 생략 → 전체 삭제
        phases.clear();
        return;
    }
    let mut errors_local = Vec::new();
    let targets = resolve_targets(phases, task, phase, &mut errors_local);
    errors.extend(errors_local);
    // 역순 제거 (인덱스 보존)
    let mut to_remove: Vec<(usize, usize)> = targets;
    to_remove.sort_by(|a, b| b.cmp(a));
    for (pi, ti) in to_remove {
        if pi < phases.len() && ti < phases[pi].tasks.len() {
            phases[pi].tasks.remove(ti);
        }
    }
    // 빈 phase 제거
    phases.retain(|p| !p.tasks.is_empty());
}

// ── 정규화 & 완료 전환 ──────────────────────────────────────────────

/// 한 phase에 in_progress task가 2개 이상이면 첫 번째만 유지.
/// omp `normalizeInProgressTask` 계약.
fn normalize_in_progress(phases: &mut [TodoPhase]) {
    let mut found = false;
    for phase in phases.iter_mut().rev() {
        for task in &mut phase.tasks {
            if task.status == TodoStatus::InProgress {
                if found {
                    task.status = TodoStatus::Pending;
                } else {
                    found = true;
                }
            }
        }
    }
}

/// 이전/이후 phase 배열을 비교해 새로 Completed가 된 task 목록.
/// TUI 스트라이크루 애니메이션 트리거용.
fn get_completion_transitions(
    previous: &[TodoPhase],
    updated: &[TodoPhase],
) -> Vec<TodoCompletionTransition> {
    let mut out = Vec::new();
    for new_phase in updated {
        let old_phase = previous.iter().find(|p| p.name == new_phase.name);
        for new_task in &new_phase.tasks {
            if new_task.status != TodoStatus::Completed {
                continue;
            }
            let was_completed = old_phase
                .and_then(|p| p.tasks.iter().find(|t| t.content == new_task.content))
                .is_some_and(|t| t.status == TodoStatus::Completed);
            if !was_completed {
                out.push(TodoCompletionTransition {
                    phase: new_phase.name.clone(),
                    content: new_task.content.clone(),
                });
            }
        }
    }
    out
}

/// todo 내용과 서브에이전트 설명이 같은 작업을 가리키는지.
/// 6자 이상 중복 정규화 매칭 (omp TODO_DESCRIPTION_MIN_OVERLAP).
pub fn todo_matches_any_description(content: &str, descriptions: &[String]) -> bool {
    let normalized = normalize_for_match(content);
    if normalized.len() < 6 {
        return false;
    }
    descriptions.iter().any(|d| {
        let d_norm = normalize_for_match(d);
        d_norm.contains(&normalized) || normalized.contains(&d_norm)
    })
}

fn normalize_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(lc);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

// ── Markdown 라운드트립 ──────────────────────────────────────────────

/// phases → Markdown 체크리스트. 다중 phase면 로마 숫자 헤더.
pub fn phases_to_markdown(phases: &[TodoPhase]) -> String {
    let mut out = String::new();
    for (i, phase) in phases.iter().enumerate() {
        if phases.len() > 1 {
            out.push_str(&format!("{}. {}\n", roman_numeral(i + 1), phase.name));
        }
        for task in &phase.tasks {
            let marker = match task.status {
                TodoStatus::Completed => "- [x]",
                TodoStatus::Abandoned => "- [-]",
                _ => "- [ ]",
            };
            out.push_str(&format!("  {} {}\n", marker, task.content));
        }
    }
    out
}

const ROMAN_PAIRS: &[(u32, &str)] = &[
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

fn roman_numeral(mut n: usize) -> String {
    let mut out = String::new();
    for &(value, sym) in ROMAN_PAIRS {
        while n >= value as usize {
            out.push_str(sym);
            n -= value as usize;
        }
    }
    out
}

/// Markdown 체크리스트 → phases. 헤더 (`## Phase` 또는 `N. Phase`)와 체크박스 파싱.
/// omp `markdownToPhases` 계약.
pub fn markdown_to_phases(md: &str) -> Result<Vec<TodoPhase>, String> {
    let mut phases: Vec<TodoPhase> = Vec::new();
    let mut current_phase: Option<TodoPhase> = None;

    for line in md.lines() {
        let trimmed = line.trim_end();
        if let Some(name) = parse_phase_header(trimmed) {
            if let Some(p) = current_phase.take() {
                phases.push(p);
            }
            current_phase = Some(TodoPhase {
                name,
                tasks: Vec::new(),
            });
        } else if let Some((status, content)) = parse_task_line(trimmed) {
            let target = current_phase.get_or_insert_with(|| TodoPhase {
                name: DEFAULT_INIT_PHASE.into(),
                tasks: Vec::new(),
            });
            target.tasks.push(TodoItem {
                content,
                status,
                notes: None,
            });
        }
    }
    if let Some(p) = current_phase {
        phases.push(p);
    }
    Ok(phases)
}

fn parse_phase_header(line: &str) -> Option<String> {
    let t = line.trim();
    // ## Phase Name
    if let Some(rest) = t.strip_prefix("## ") {
        return Some(rest.trim().to_string());
    }
    // I. Phase Name  /  II. Phase Name
    for prefix_len in 1..=6 {
        if t.len() <= prefix_len {
            break;
        }
        let prefix = &t[..prefix_len];
        if prefix.ends_with('.')
            && prefix[..prefix_len - 1]
                .chars()
                .all(|c| c.is_ascii_uppercase())
        {
            let rest = t[prefix_len..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn parse_task_line(line: &str) -> Option<(TodoStatus, String)> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("- [x] ") {
        return Some((TodoStatus::Completed, rest.to_string()));
    }
    if let Some(rest) = t.strip_prefix("- [X] ") {
        return Some((TodoStatus::Completed, rest.to_string()));
    }
    if let Some(rest) = t.strip_prefix("- [-] ") {
        return Some((TodoStatus::Abandoned, rest.to_string()));
    }
    if let Some(rest) = t.strip_prefix("- [ ] ") {
        return Some((TodoStatus::Pending, rest.to_string()));
    }
    None
}

// ── 요약 포맷 ────────────────────────────────────────────────────────

/// Render a human-readable summary of the todo list for display.
pub fn format_summary(phases: &[TodoPhase], errors: &[String], read_only: bool) -> String {
    let total: usize = phases.iter().map(|p| p.tasks.len()).sum();
    let done: usize = phases
        .iter()
        .map(|p| {
            p.tasks
                .iter()
                .filter(|t| t.status == TodoStatus::Completed)
                .count()
        })
        .sum();

    let mut out = if read_only {
        format!(
            "\u{1F4CB} Todo list (read-only) — {}/{} done\n\n",
            done, total
        )
    } else if errors.is_empty() {
        format!("\u{2713} Todo updated — {}/{} done\n\n", done, total)
    } else {
        format!(
            "\u{26A0} Todo updated with {} error(s) — {}/{} done\n\n",
            errors.len(),
            done,
            total
        )
    };

    for (i, phase) in phases.iter().enumerate() {
        if phases.len() > 1 {
            out.push_str(&format!("{}. {}\n", roman_numeral(i + 1), phase.name));
        }
        for task in &phase.tasks {
            out.push_str(&format!("  {} {}\n", task.status.icon(), task.content));
        }
    }

    for err in errors {
        out.push_str(&format!("  \u{26A0} {}\n", err));
    }

    out
}

// ── Apply ops helper ─────────────────────────────────────────────────

/// Apply a sequence of ops, returning the result + transitions + errors.
pub fn apply_ops(phases: &mut Vec<TodoPhase>, ops: &[TodoOp]) -> TodoUpdateResult {
    let old_phases = phases.clone();
    let mut errors = Vec::new();
    for op in ops {
        apply_entry(phases, op, &mut errors);
    }
    normalize_in_progress(phases);
    let completed_tasks = get_completion_transitions(&old_phases, phases);
    TodoUpdateResult {
        phases: phases.clone(),
        completed_tasks,
        errors,
    }
}

/// Trait abstracting where todo state lives. Implemented by hosts
/// (e.g. `oxi-cli::store::TodoState`) so the stateless `TodoTool` and
/// the TUI sticky panel share one source of truth.
pub trait TodoStateProvider: Send + Sync {
    /// Snapshot of the current phases (cheap clone expected).
    fn get_phases(&self) -> Vec<TodoPhase>;

    /// Apply a batch of ops asynchronously. The returned future is
    /// `Send` so it can be driven from a worker task.
    fn apply_ops<'a>(
        &'a self,
        ops: Vec<TodoOp>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TodoUpdateResult, ToolError>> + Send + 'a>,
    >;
}
// ── TodoTool (AgentTool 구현) ─────────────────────────────────────────

/// `todo` agent tool. 상태 비저장 (상태는 `TodoStateProvider`가 보유).
pub struct TodoTool;

#[async_trait]
impl AgentTool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn label(&self) -> &str {
        "Todo"
    }

    fn essential(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Phased todo list manager. Use init to create a plan, start/done/drop \
         to transition tasks, append to add, rm to remove, view to read. \
         Tasks should be 5-10 words describing WHAT not HOW."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ops": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {
                                "type": "string",
                                "enum": ["init", "start", "done", "drop", "rm", "append", "view"]
                            },
                            "task": {"type": "string", "description": "Task content (verbatim)"},
                            "phase": {"type": "string", "description": "Phase name"},
                            "items": {"type": "array", "items": {"type": "string"}},
                            "list": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "phase": {"type": "string"},
                                        "items": {"type": "array", "items": {"type": "string"}}
                                    }
                                }
                            }
                        },
                        "required": ["op"]
                    }
                }
            },
            "required": ["ops"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // v2: 능력 특성 주입 (ToolContext.todo)
        let provider = ctx.todo.as_ref().ok_or("Todo not configured")?;

        let ops_value = params
            .get("ops")
            .cloned()
            .ok_or_else(|| "Missing required parameter: ops".to_string())?;

        let ops: Vec<TodoOp> =
            serde_json::from_value(ops_value).map_err(|e| format!("Invalid ops format: {}", e))?;

        let result = provider.apply_ops(ops).await?;

        let summary = format_summary(&result.phases, &result.errors, false);
        Ok(AgentToolResult::success(summary))
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.into(),
            status,
            notes: None,
        }
    }

    #[test]
    fn init_with_phased_list() {
        let mut phases = vec![];
        let mut errors = vec![];
        apply_entry(
            &mut phases,
            &TodoOp::Init {
                list: Some(vec![
                    InitListEntry {
                        phase: "A".into(),
                        items: vec!["a1".into(), "a2".into()],
                    },
                    InitListEntry {
                        phase: "B".into(),
                        items: vec!["b1".into()],
                    },
                ]),
                items: None,
            },
            &mut errors,
        );
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].name, "A");
        assert_eq!(phases[0].tasks.len(), 2);
        assert_eq!(phases[1].name, "B");
        assert!(errors.is_empty());
    }

    #[test]
    fn init_with_flat_items_uses_default_phase() {
        let mut phases = vec![];
        let mut errors = vec![];
        apply_entry(
            &mut phases,
            &TodoOp::Init {
                list: None,
                items: Some(vec!["task1".into(), "task2".into()]),
            },
            &mut errors,
        );
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].name, "Tasks");
        assert_eq!(phases[0].tasks.len(), 2);
    }

    #[test]
    fn init_without_list_or_items_errors() {
        let mut phases = vec![];
        let mut errors = vec![];
        apply_entry(
            &mut phases,
            &TodoOp::Init {
                list: None,
                items: None,
            },
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn start_normalizes_other_in_progress() {
        let mut phases = vec![TodoPhase {
            name: "A".into(),
            tasks: vec![
                make_task("a1", TodoStatus::Pending),
                make_task("a2", TodoStatus::Pending),
            ],
        }];

        let result = apply_ops(
            &mut phases,
            &[
                TodoOp::Start {
                    task: Some("a1".into()),
                    phase: None,
                },
                TodoOp::Start {
                    task: Some("a2".into()),
                    phase: None,
                },
            ],
        );
        assert!(result.errors.is_empty());
        // omp 동작: 단일 phase에서 첫 task가 in_progress 유지, 이후는 pending으로 리셋.
        let a1 = phases[0].tasks.iter().find(|t| t.content == "a1").unwrap();
        let a2 = phases[0].tasks.iter().find(|t| t.content == "a2").unwrap();
        assert_eq!(a1.status, TodoStatus::InProgress);
        assert_eq!(a2.status, TodoStatus::Pending);
    }

    #[test]
    fn completion_transition_detects_newly_completed() {
        let old = vec![TodoPhase {
            name: "A".into(),
            tasks: vec![make_task("a1", TodoStatus::InProgress)],
        }];
        let updated = vec![TodoPhase {
            name: "A".into(),
            tasks: vec![make_task("a1", TodoStatus::Completed)],
        }];
        let transitions = get_completion_transitions(&old, &updated);
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].content, "a1");
    }

    #[test]
    fn completion_transition_excludes_already_completed() {
        let old = vec![TodoPhase {
            name: "A".into(),
            tasks: vec![make_task("a1", TodoStatus::Completed)],
        }];
        let updated = old.clone();
        let transitions = get_completion_transitions(&old, &updated);
        assert!(transitions.is_empty());
    }

    #[test]
    fn todo_matches_subagent_description() {
        // 동일 substring 매칭: 길이 ≥ 6.
        assert!(todo_matches_any_description(
            "implement authentication module",
            &["authentication module".into()]
        ));
        assert!(!todo_matches_any_description(
            "fix",
            &["fix the bug".into()] // 6자 미만 정규화 → 매칭 안 됨
        ));
        assert!(!todo_matches_any_description(
            "implement auth",
            &["authentication module".into()] // 서로 substring 아님
        ));
    }

    #[test]
    fn markdown_roundtrip_preserves_state() {
        let phases = vec![TodoPhase {
            name: "Test".into(),
            tasks: vec![make_task("Run tests", TodoStatus::Completed)],
        }];
        let md = phases_to_markdown(&phases);
        let parsed = markdown_to_phases(&md).unwrap();
        assert_eq!(parsed[0].tasks[0].status, TodoStatus::Completed);
    }

    #[test]
    fn roman_numeral_correct() {
        assert_eq!(roman_numeral(1), "I");
        assert_eq!(roman_numeral(4), "IV");
        assert_eq!(roman_numeral(9), "IX");
        assert_eq!(roman_numeral(42), "XLII");
        assert_eq!(roman_numeral(1994), "MCMXCIV");
    }

    #[test]
    fn append_creates_phase_if_missing() {
        let mut phases = vec![];
        let mut errors = vec![];
        apply_entry(
            &mut phases,
            &TodoOp::Append {
                phase: "New".into(),
                items: vec!["a".into(), "b".into()],
            },
            &mut errors,
        );
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].name, "New");
        assert_eq!(phases[0].tasks.len(), 2);
    }

    #[test]
    fn rm_with_neither_clears_all() {
        let mut phases = vec![TodoPhase {
            name: "X".into(),
            tasks: vec![make_task("a", TodoStatus::Pending)],
        }];
        let mut errors = vec![];
        apply_entry(
            &mut phases,
            &TodoOp::Rm {
                task: None,
                phase: None,
            },
            &mut errors,
        );
        assert!(phases.is_empty());
    }

    #[test]
    fn done_marks_completed() {
        let mut phases = vec![TodoPhase {
            name: "A".into(),
            tasks: vec![make_task("a1", TodoStatus::Pending)],
        }];
        let result = apply_ops(
            &mut phases,
            &[TodoOp::Done {
                task: Some("a1".into()),
                phase: None,
            }],
        );
        assert!(result.errors.is_empty());
        assert_eq!(phases[0].tasks[0].status, TodoStatus::Completed);
        assert_eq!(result.completed_tasks.len(), 1);
    }

    #[test]
    fn drop_marks_abandoned() {
        let mut phases = vec![TodoPhase {
            name: "A".into(),
            tasks: vec![make_task("a1", TodoStatus::Pending)],
        }];
        let result = apply_ops(
            &mut phases,
            &[TodoOp::Drop {
                task: Some("a1".into()),
                phase: None,
            }],
        );
        assert!(result.errors.is_empty());
        assert_eq!(phases[0].tasks[0].status, TodoStatus::Abandoned);
    }
}
