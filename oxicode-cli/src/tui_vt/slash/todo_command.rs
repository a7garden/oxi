//! `/todo` slash command — show, mutate, expand/collapse, export/import,
//! and copy the todo list.
//!
//! Mutations route through the same `TodoStateProvider::apply_ops` the
//! `todo` agent tool uses — a single source of truth. The command runs on
//! the TUI's tokio runtime, so async `apply_ops` goes through `tokio::spawn`
//! and replies land on the transcript via the cloned `InlineHandle`
//! (mirrors `MemoryCommand` in `registry.rs`).

use oxicode_agent::tools::TodoStateProvider;
use oxicode_agent::tools::todo::{
    TodoOp, find_phase_fuzzy, find_task_fuzzy, markdown_to_phases, phases_to_markdown,
    tokenize_quoted,
};
use oxicode_vtui::tui::core::{InlineHandle, InlineMessageKind};

use crate::tui_vt::main_loop::plain_segment;
use crate::tui_vt::slash::registry::{SlashCommand, SlashCtx, SlashOutcome};

const TODO_USAGE: &str = "Usage: /todo <verb> [args]\n\
  /todo                              Show current todos\n\
  /todo expand                       Expand the sticky HUD\n\
  /todo collapse                     Collapse the sticky HUD\n\
  /todo copy                         Copy todos as Markdown to clipboard\n\
  /todo export [<path>]              Write todos to file (default: TODO.md)\n\
  /todo import [<path>]              Replace todos from file (default: TODO.md)\n\
  /todo append [<phase>] <task...>   Append a task; phase fuzzy-matched or auto-created\n\
  /todo start  <task>                Mark task in_progress (fuzzy content match)\n\
  /todo done   [<task|phase>]        Mark task/phase/all completed\n\
  /todo drop   [<task|phase>]        Mark task/phase/all abandoned\n\
  /todo rm     [<task|phase>]        Remove task/phase/all";

fn append_reply(handle: &InlineHandle, kind: InlineMessageKind, text: String) {
    for line in text.split('\n') {
        handle.append_line(kind, vec![plain_segment(line.to_string())]);
    }
}

/// Spawn an async `apply_ops` and reply once it resolves. `success` is shown
/// on `Ok`; a wrapped error on `Err`.
fn spawn_apply(
    provider: std::sync::Arc<dyn TodoStateProvider>,
    ops: Vec<TodoOp>,
    handle: InlineHandle,
    success: String,
) {
    tokio::spawn(async move {
        match provider.apply_ops(ops).await {
            Ok(_) => append_reply(&handle, InlineMessageKind::Info, success),
            Err(e) => append_reply(&handle, InlineMessageKind::Error, e),
        }
    });
}

fn show_current(provider: &std::sync::Arc<dyn TodoStateProvider>, ctx: &mut SlashCtx<'_>) {
    let phases = provider.get_phases();
    if phases.is_empty() {
        ctx.reply(
            InlineMessageKind::Info,
            "No todos. Use /todo append <task> to start one.",
        );
    } else {
        ctx.reply(
            InlineMessageKind::Info,
            phases_to_markdown(&phases).trim_end(),
        );
    }
}

/// Build the append op + reply text from the trailing args.
fn append_op(rest: &str) -> (TodoOp, String) {
    let tokens = tokenize_quoted(rest);
    let (phase_name, content) = if tokens.len() == 1 {
        (None, tokens[0].clone())
    } else {
        (Some(tokens[0].clone()), tokens[1..].join(" "))
    };
    let target = phase_name.unwrap_or_else(|| "Todos".to_string());
    (
        TodoOp::Append {
            phase: target.clone(),
            items: vec![content.clone()],
        },
        format!("Appended to {target}: {content}"),
    )
}

/// `TodoOp::Start` on the fuzzy-matched task, or an error.
fn start_op(
    rest: &str,
    phases: &[oxicode_agent::tools::todo::TodoPhase],
) -> Result<(TodoOp, String), String> {
    if rest.is_empty() {
        return Err("Usage: /todo start <task>".to_string());
    }
    match find_task_fuzzy(phases, rest) {
        Some((task, _)) => {
            let content = task.content.clone();
            Ok((
                TodoOp::Start {
                    task: Some(content.clone()),
                    phase: None,
                },
                format!("Started: {content}"),
            ))
        }
        None => Err(format!(
            "No task matched \"{rest}\". Use /todo to list current tasks."
        )),
    }
}

/// `TodoOp` + reply text for a done/drop/rm mutation, resolved against the
/// current phases (task match first, then phase, then all).
fn mutate_op(
    verb: &str,
    rest: &str,
    phases: &[oxicode_agent::tools::todo::TodoPhase],
) -> Result<(TodoOp, String), String> {
    let make_op = |task: Option<String>, phase: Option<String>| match verb {
        "done" => TodoOp::Done { task, phase },
        "drop" => TodoOp::Drop { task, phase },
        _ => TodoOp::Rm { task, phase },
    };
    let label = match verb {
        "done" => "Marked completed",
        "drop" => "Marked abandoned",
        _ => "Removed",
    };
    if rest.is_empty() {
        return Ok((
            make_op(None, None),
            match verb {
                "done" => "Marked all tasks completed.".to_string(),
                "drop" => "Marked all tasks abandoned.".to_string(),
                _ => "Cleared all todos.".to_string(),
            },
        ));
    }
    if let Some((task, _)) = find_task_fuzzy(phases, rest) {
        let content = task.content.clone();
        return Ok((
            make_op(Some(content.clone()), None),
            format!("{label}: {content}"),
        ));
    }
    if let Some(phase) = find_phase_fuzzy(phases, rest) {
        let name = phase.name.clone();
        return Ok((
            make_op(None, Some(name.clone())),
            format!("{label} phase: {name}"),
        ));
    }
    Err(format!("No task or phase matched \"{rest}\"."))
}

/// Copy text to the system clipboard with a platform shell-out — no new
/// dependency (`pbcopy`/`xclip -selection clipboard`/`clip`).
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("pbcopy");
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard"]);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = Command::new("clip");

    let mut child = cmd.stdin(Stdio::piped()).spawn()?;
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(text.as_bytes())?;
    child.wait()?;
    Ok(())
}

/// Resolve the export/import target path: explicit arg or `cwd/TODO.md`.
fn resolve_todo_path(rest: &str, cwd: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(rest.trim());
    if rest.trim().is_empty() || p.is_absolute() {
        if rest.trim().is_empty() {
            std::path::PathBuf::from(cwd).join("TODO.md")
        } else {
            p
        }
    } else {
        std::path::PathBuf::from(cwd).join(p)
    }
}

pub struct TodoCommand;

impl SlashCommand for TodoCommand {
    fn name(&self) -> &'static str {
        "todo"
    }

    fn description(&self) -> &'static str {
        "Show or mutate the todo list"
    }

    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let trimmed = args.trim();
        let Some(provider) = ctx.session.todo_provider() else {
            ctx.reply(InlineMessageKind::Error, "Todo not configured");
            return SlashOutcome::Handled;
        };
        if trimmed.is_empty() {
            show_current(&provider, ctx);
            return SlashOutcome::Handled;
        }
        let (verb, rest) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        let rest = rest.trim();
        let handle = ctx.handle.clone();
        match verb.to_ascii_lowercase().as_str() {
            "help" | "?" => ctx.reply(InlineMessageKind::Info, TODO_USAGE),
            "expand" => {
                ctx.state.todo_expanded = true;
                ctx.reply(InlineMessageKind::Info, "Expanded the todo HUD.");
            }
            "collapse" => {
                ctx.state.todo_expanded = false;
                ctx.reply(InlineMessageKind::Info, "Collapsed the todo HUD.");
            }
            "append" => {
                let (op, msg) = append_op(rest);
                spawn_apply(provider, vec![op], handle, msg);
            }
            "start" => match start_op(rest, &provider.get_phases()) {
                Ok((op, msg)) => spawn_apply(provider, vec![op], handle, msg),
                Err(e) => ctx.reply(InlineMessageKind::Error, e),
            },
            "done" | "drop" | "rm" => match mutate_op(verb, rest, &provider.get_phases()) {
                Ok((op, msg)) => spawn_apply(provider, vec![op], handle, msg),
                Err(e) => ctx.reply(InlineMessageKind::Error, e),
            },
            "copy" => {
                let phases = provider.get_phases();
                if phases.is_empty() {
                    ctx.reply(InlineMessageKind::Warning, "No todos to copy.");
                } else {
                    match copy_to_clipboard(&phases_to_markdown(&phases)) {
                        Ok(()) => ctx.reply(
                            InlineMessageKind::Info,
                            "Copied todos as Markdown to clipboard.",
                        ),
                        Err(e) => ctx.reply(InlineMessageKind::Error, e.to_string()),
                    }
                }
            }
            "export" => {
                let phases = provider.get_phases();
                if phases.is_empty() {
                    ctx.reply(InlineMessageKind::Warning, "No todos to export.");
                } else {
                    let target = resolve_todo_path(rest, ctx.session.cwd());
                    match std::fs::write(&target, phases_to_markdown(&phases)) {
                        Ok(()) => ctx.reply(
                            InlineMessageKind::Info,
                            format!("Wrote todos to {}", target.display()),
                        ),
                        Err(e) => ctx.reply(
                            InlineMessageKind::Error,
                            format!("Failed to write todos: {e}"),
                        ),
                    }
                }
            }
            "import" => {
                let target = resolve_todo_path(rest, ctx.session.cwd());
                let content = match std::fs::read_to_string(&target) {
                    Ok(c) => c,
                    Err(e) => {
                        ctx.reply(
                            InlineMessageKind::Error,
                            format!("Failed to read todos: {e}"),
                        );
                        return SlashOutcome::Handled;
                    }
                };
                match markdown_to_phases(&content) {
                    Ok(phases) => {
                        let task_count: usize = phases.iter().map(|p| p.tasks.len()).sum();
                        provider.set_phases_sync(phases.clone());
                        ctx.reply(
                            InlineMessageKind::Info,
                            format!(
                                "Imported {} phase(s), {task_count} task(s) from {}.",
                                phases.len(),
                                target.display()
                            ),
                        );
                    }
                    Err(e) => ctx.reply(
                        InlineMessageKind::Error,
                        format!("Could not parse {}:\n  {e}", target.display()),
                    ),
                }
            }
            other => ctx.reply(
                InlineMessageKind::Error,
                format!("Unknown /todo verb \"{other}\".\n{TODO_USAGE}"),
            ),
        }
        SlashOutcome::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_agent::tools::todo::{TodoItem, TodoPhase, TodoStatus};

    fn make_task(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.into(),
            status,
            notes: None,
            block_reason: None,
        }
    }

    fn phases() -> Vec<TodoPhase> {
        vec![TodoPhase {
            name: "Auth".into(),
            tasks: vec![
                make_task("Wire OAuth providers", TodoStatus::Pending),
                make_task("Wire OAuth refresh", TodoStatus::InProgress),
            ],
        }]
    }

    #[test]
    fn append_op_defaults_phase_to_todos() {
        // Single token -> no phase prefix -> default "Todos" phase.
        let (op, msg) = append_op("OAuth");
        assert!(matches!(&op, TodoOp::Append { phase, .. } if phase == "Todos"));
        assert!(msg.contains("OAuth"));
    }

    #[test]
    fn append_op_respects_leading_phase_token() {
        let (op, msg) = append_op("Auth Wire OAuth");
        assert!(matches!(&op, TodoOp::Append { phase, .. } if phase == "Auth"));
        assert!(msg.contains("Auth"));
    }

    #[test]
    fn start_op_builds_start_for_fuzzy_task() {
        // "oauth providers" matches exactly one task -> unambiguous.
        let (op, msg) = start_op("oauth providers", &phases()).unwrap();
        assert!(
            matches!(&op, TodoOp::Start { task, .. } if task.as_deref() == Some("Wire OAuth providers"))
        );
        assert!(msg.contains("Started"));
    }

    #[test]
    fn start_op_errors_on_no_match() {
        assert!(start_op("nonexistent task", &phases()).is_err());
    }

    #[test]
    fn mutate_op_done_on_task_fuzzy_match() {
        let (op, msg) = mutate_op("done", "oauth refresh", &phases()).unwrap();
        assert!(
            matches!(&op, TodoOp::Done { task, .. } if task.as_deref() == Some("Wire OAuth refresh"))
        );
        assert!(msg.contains("Marked completed"));
    }

    #[test]
    fn mutate_op_done_with_no_arg_targets_all() {
        let (op, msg) = mutate_op("done", "", &phases()).unwrap();
        assert!(matches!(
            &op,
            TodoOp::Done {
                task: None,
                phase: None
            }
        ));
        assert!(msg.contains("all tasks"));
    }

    #[test]
    fn mutate_op_done_on_phase_fuzzy_match() {
        let (op, msg) = mutate_op("done", "auth", &phases()).unwrap();
        assert!(matches!(&op, TodoOp::Done { phase: Some(p), .. } if p == "Auth"));
        assert!(msg.contains("phase"));
    }

    #[test]
    fn mutate_op_errors_on_no_match() {
        assert!(mutate_op("rm", "nonexistent", &phases()).is_err());
    }

    #[test]
    fn resolve_todo_path_defaults_to_cwd_todo_md() {
        let p = resolve_todo_path("", "/tmp/work");
        assert_eq!(p, std::path::PathBuf::from("/tmp/work/TODO.md"));
    }

    #[test]
    fn resolve_todo_path_keeps_absolute_and_joins_relative() {
        assert_eq!(
            resolve_todo_path("/abs/out.md", "/tmp/work"),
            std::path::PathBuf::from("/abs/out.md")
        );
        assert_eq!(
            resolve_todo_path("out.md", "/tmp/work"),
            std::path::PathBuf::from("/tmp/work/out.md")
        );
    }
}
