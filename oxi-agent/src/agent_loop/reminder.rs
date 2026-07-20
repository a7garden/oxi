//! 컴팩션 reminder 시스템 (PR-C1).
//!
//! Grok 의 `ActiveAgentReminderState`
//! (`xai-grok-compaction/src/reminder.rs:87`) 패턴 차용. 컴팩션 시점에
//! 활성 todo / (향후) 서브에이전트 / (향후) background task 상태를 조회해
//! "Focus areas after compaction:" 섹션을 만든다.

use crate::tools::TodoStateProvider;
use crate::tools::todo::{TodoPhase, TodoStatus};

/// 컴팩션 시점에 활성 상태를 조회하기 위한 입력 묶음.
pub struct ActiveReminderInputs<'a> {
    /// Active todo snapshot source. `None` means no reminder section.
    pub todos: Option<&'a dyn TodoStateProvider>,
    /// Reserved for future sub-agent counts. PR-C4+.
    #[allow(dead_code)]
    pub subagent_state: Option<()>,
    /// Reserved for future background-task counts. PR-C4+.
    #[allow(dead_code)]
    pub background_tasks: Option<()>,
    /// Reserved for future keyword extraction from recent turns.
    #[allow(dead_code)]
    pub last_n_assistant_turns: usize,
}
/// Default initializer — every source `None`, turns count 0.
impl<'a> Default for ActiveReminderInputs<'a> {
    fn default() -> Self {
        Self {
            todos: None,
            subagent_state: None,
            background_tasks: None,
            last_n_assistant_turns: 0,
        }
    }
}

impl<'a> Clone for ActiveReminderInputs<'a> {
    fn clone(&self) -> Self {
        Self {
            todos: self.todos,
            subagent_state: self.subagent_state,
            background_tasks: self.background_tasks,
            last_n_assistant_turns: self.last_n_assistant_turns,
        }
    }
}

impl<'a> std::fmt::Debug for ActiveReminderInputs<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveReminderInputs")
            .field("todos", &self.todos.map(|p| p.get_phases()))
            .field("subagent_state", &self.subagent_state)
            .field("background_tasks", &self.background_tasks)
            .field("last_n_assistant_turns", &self.last_n_assistant_turns)
            .finish()
    }
}

/// Reminder 문자열 생성기.
#[derive(Debug, Default, Clone)]
pub struct CompactionReminderBuilder {
    max_chars: usize,
}

impl CompactionReminderBuilder {
    /// Build a default builder with 800-char cap.
    pub fn new() -> Self {
        Self { max_chars: 800 }
    }
    /// Override the cap. Pass 0 for unlimited.
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }
    /// Render the reminder text. Empty string when no active sources.
    pub fn build(&self, inputs: &ActiveReminderInputs<'_>) -> String {
        let mut sections = Vec::new();

        if let Some(todos) = inputs.todos {
            let phases = todos.get_phases();
            if !phases.is_empty() {
                let summary = todo_summary(&phases);
                if !summary.is_empty() {
                    sections.push(summary);
                }
            }
        }

        let joined = sections.join("\n");
        if self.max_chars == 0 || joined.len() <= self.max_chars {
            joined
        } else {
            let mut end = self.max_chars.saturating_sub(3);
            while end > 0 && !joined.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &joined[..end])
        }
    }
}

fn todo_summary(phases: &[TodoPhase]) -> String {
    let mut in_progress = 0usize;
    let mut pending = 0usize;
    let mut total = 0usize;

    for phase in phases {
        for task in &phase.tasks {
            total += 1;
            match task.status {
                TodoStatus::InProgress => in_progress += 1,
                TodoStatus::Pending => pending += 1,
                TodoStatus::Completed | TodoStatus::Abandoned => {}
            }
        }
    }

    if total == 0 {
        return String::new();
    }

    let actionable = in_progress + pending;
    if actionable == 0 {
        return String::new();
    }

    let mut parts = Vec::new();
    parts.push(format!(
        "Active todos: {} in progress, {} pending ({} phases)",
        in_progress,
        pending,
        phases.len(),
    ));

    let active_phase_names: Vec<&str> = phases
        .iter()
        .filter(|p| {
            p.tasks
                .iter()
                .any(|t| matches!(t.status, TodoStatus::InProgress))
        })
        .take(3)
        .map(|p| p.name.as_str())
        .collect();
    if !active_phase_names.is_empty() {
        parts.push(format!("Current phases: {}", active_phase_names.join(", ")));
    }

    parts.join("\n")
}

/// `compaction_instruction` (정적) + reminder (동적) 결합.
pub fn combine_instruction(static_part: Option<String>, reminder: String) -> Option<String> {
    let dynamic = if reminder.is_empty() {
        None
    } else {
        Some(reminder)
    };
    match (static_part, dynamic) {
        (None, None) => None,
        (Some(s), None) => Some(s),
        (None, Some(d)) => Some(d),
        (Some(s), Some(d)) => Some(format!("{}\n\nFocus areas after compaction:\n{}", s, d)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolError;
    use crate::tools::todo::{TodoItem, TodoOp, TodoPhase, TodoStatus, TodoUpdateResult};

    #[derive(Debug)]
    struct StubTodos(Vec<TodoPhase>);

    impl TodoStateProvider for StubTodos {
        fn get_phases(&self) -> Vec<TodoPhase> {
            self.0.clone()
        }
        fn apply_ops<'a>(
            &'a self,
            _ops: Vec<TodoOp>,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<TodoUpdateResult, ToolError>> + Send + 'a>>
        {
            Box::pin(async move { Err("stub does not accept ops".to_string()) })
        }
    }

    fn phase<S, I>(name: &str, tasks: I) -> TodoPhase
    where
        I: IntoIterator<Item = (S, TodoStatus)>,
        S: Into<String>,
    {
        TodoPhase {
            name: name.to_string(),
            tasks: tasks
                .into_iter()
                .map(|(content, status)| TodoItem {
                    content: content.into(),
                    status,
                    notes: None,
                })
                .collect(),
        }
    }

    #[test]
    fn build_returns_empty_when_no_inputs() {
        let b = CompactionReminderBuilder::new();
        let inputs = ActiveReminderInputs::default();
        assert_eq!(b.build(&inputs), "");
    }

    #[test]
    fn build_returns_empty_when_todo_provider_is_none() {
        let b = CompactionReminderBuilder::new();
        let inputs = ActiveReminderInputs {
            todos: None,
            ..Default::default()
        };
        assert_eq!(b.build(&inputs), "");
    }

    #[test]
    fn build_returns_empty_when_all_tasks_completed() {
        let provider = StubTodos(vec![phase(
            "phase-a",
            vec![
                ("done-1", TodoStatus::Completed),
                ("done-2", TodoStatus::Completed),
            ],
        )]);
        let b = CompactionReminderBuilder::new();
        let inputs = ActiveReminderInputs {
            todos: Some(&provider),
            ..Default::default()
        };
        assert_eq!(b.build(&inputs), "");
    }

    #[test]
    fn build_lists_active_todos() {
        let provider = StubTodos(vec![
            phase(
                "phase-a",
                vec![
                    ("working", TodoStatus::InProgress),
                    ("todo", TodoStatus::Pending),
                    ("done", TodoStatus::Completed),
                ],
            ),
            phase("phase-b", vec![("waiting", TodoStatus::Pending)]),
        ]);
        let b = CompactionReminderBuilder::new();
        let inputs = ActiveReminderInputs {
            todos: Some(&provider),
            ..Default::default()
        };
        let out = b.build(&inputs);
        assert!(out.contains("Active todos: 1 in progress, 2 pending"));
        assert!(out.contains("2 phases"));
        assert!(out.contains("phase-a"));
        assert!(
            !out.contains("phase-b"),
            "non-active phase should be omitted: {out}"
        );
    }

    #[test]
    fn max_chars_caps_output() {
        let provider = StubTodos(vec![phase(
            "long-phase",
            (0..50)
                .map(|i| (format!("task-{i}"), TodoStatus::Pending))
                .collect::<Vec<_>>(),
        )]);
        let b = CompactionReminderBuilder::new().with_max_chars(30);
        let inputs = ActiveReminderInputs {
            todos: Some(&provider),
            ..Default::default()
        };
        let out = b.build(&inputs);
        assert!(out.len() <= 30, "output not capped: {} chars", out.len());
        assert!(out.ends_with("..."));
    }

    #[test]
    fn combine_instruction_handles_all_combinations() {
        assert_eq!(combine_instruction(None, String::new()), None);
        assert_eq!(
            combine_instruction(Some("static".into()), String::new()),
            Some("static".into())
        );
        assert_eq!(
            combine_instruction(None, "dynamic".into()),
            Some("dynamic".into())
        );
        assert_eq!(
            combine_instruction(Some("static".into()), "dynamic".into()),
            Some("static\n\nFocus areas after compaction:\ndynamic".into())
        );
    }
}
