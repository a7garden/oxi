//! `/issue` — open issues panel, or `new <title>` / `show|start|release|close
//! <id>`. Migrated off the legacy `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::app::agent_session::AgentSession;
use crate::store::issues::Priority;
use crate::tui::app::{AppState, NotificationKind, UiEvent};
use crate::tui::overlay;
use crate::tui::slash::SlashCtx;
use crate::tui::slash::SlashOutcome;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// `/issue [subcommand]` — manage issues.
pub(crate) struct IssueCommand;

impl SlashCommand for IssueCommand {
    fn name(&self) -> &str {
        "issue"
    }
    fn aliases(&self) -> &[&str] {
        &["issues"]
    }
    fn description(&self) -> &str {
        "Open issues panel, or `new <title>` / `show|start|release|close <id>`"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let arg = args.trim();
        let arg_opt = if arg.is_empty() { None } else { Some(arg) };
        handle_issue_command(arg_opt, ctx.session, ctx.state, ctx.ui_tx);
        SlashOutcome::Handled
    }

    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        _state: &AppState,
    ) -> Vec<crate::tui::completion::CompletionItem> {
        // `/issue <new|show|start|release|close>` → first-token subcommands.
        let tokens = ["new", "show", "start", "release", "close", "reopen", "help"];
        let last = prefix.rsplit(' ').next().unwrap_or("");
        tokens
            .iter()
            .filter(|t| t.starts_with(last))
            .map(|t| crate::tui::completion::CompletionItem {
                text: t.to_string(),
                label: t.to_string(),
                description: None,
                kind: crate::tui::completion::CompletionKind::SlashArgument {
                    command: "issue".to_string(),
                },
            })
            .collect()
    }
}

/// Handle `/issue` — open the panel, pre-select by id, or run a subcommand.
/// Returns `true` (always handled).
#[allow(clippy::too_many_lines)]
fn handle_issue_command(
    arg: Option<&str>,
    session: &AgentSession,
    state: &mut AppState,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
) -> bool {
    // No issue store → hard error.
    let Some(store) = state.issue_store.clone() else {
        state.add_notification(
            "Issue store is not available in this directory".to_string(),
            NotificationKind::Error,
        );
        return true;
    };

    // No arg → open the panel (default).
    let Some(raw) = arg else {
        state.overlay = None;
        let mut overlay = overlay::IssuesPanelOverlay::new(store);
        overlay.set_ui_tx(ui_tx.clone());
        state.overlay_state = Some(Box::new(overlay));
        return true;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        state.overlay = None;
        let mut overlay = overlay::IssuesPanelOverlay::new(store);
        overlay.set_ui_tx(ui_tx.clone());
        state.overlay_state = Some(Box::new(overlay));
        return true;
    }

    // First token is the subcommand-or-id.
    let (head, rest) = match trimmed.find(char::is_whitespace) {
        Some(i) => (&trimmed[..i], trimmed[i..].trim_start()),
        None => (trimmed, ""),
    };

    // `/issue <numeric>` → open panel with that id pre-selected.
    if let Ok(id) = head.parse::<u32>() {
        state.overlay = None;
        let mut overlay = overlay::IssuesPanelOverlay::new(store);
        overlay.set_ui_tx(ui_tx.clone());
        if !overlay.pre_select(id) {
            state.add_notification(format!("Issue #{id} not found"), NotificationKind::Warning);
        }
        state.overlay_state = Some(Box::new(overlay));
        return true;
    }

    let sub = head.to_lowercase();
    let session_id_owned: String = {
        let sid = session.session_id();
        if sid.is_empty() {
            overlay::IssuesPanelOverlay::session_id().to_string()
        } else {
            sid
        }
    };

    match sub.as_str() {
        "new" => {
            if rest.is_empty() {
                state.add_notification(
                    "Usage: /issue new <title>".to_string(),
                    NotificationKind::Info,
                );
                return true;
            }
            let title = rest.to_string();
            // Parse optional `[priority=high]` and `[labels=a,b]` after the
            // title — supports both `/issue new Fix bug [high] [bug,auth]`
            // and the bare title form.
            let (title, opts) = parse_new_opts(&title);
            let priority = opts
                .as_ref()
                .and_then(|o| o.get("priority").map(|s| s.as_str()))
                .and_then(parse_priority_loose)
                .unwrap_or(Priority::Medium);
            let labels: Vec<String> = opts
                .as_ref()
                .and_then(|o| o.get("labels").map(|s| s.as_str()))
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default();
            match store.create(
                title.clone(),
                String::new(),
                priority,
                labels,
                Some(&session_id_owned),
            ) {
                Ok(issue) => {
                    state.add_notification(
                        format!("Created issue #{}: {}", issue.meta.id, issue.meta.title),
                        NotificationKind::Success,
                    );
                }
                Err(e) => {
                    state.add_notification(
                        format!("Failed to create issue: {e}"),
                        NotificationKind::Error,
                    );
                }
            }
        }
        "start" | "release" | "close" | "reopen" => {
            let Some(id) = parse_id(rest) else {
                let cmd = sub.as_str();
                state.add_notification(format!("Usage: /issue {cmd} <id>"), NotificationKind::Info);
                return true;
            };
            // The TUI loop already holds a `flock` for `"tui"` for the entire
            // run — no per-command liveness::acquire needed. The closure
            // captures only what it uses (id, sid, store, tx); the session
            // handle is intentionally NOT pulled into the spawned task since
            // the task never touches the session.
            let sid = session_id_owned.clone();
            let store_for_task = store.clone();
            let tx = ui_tx.clone();
            let op = sub.clone();
            tokio::spawn(async move {
                let result = match op.as_str() {
                    "start" => store_for_task.start(id, &sid, None).await.map(|issue| {
                        format!(
                            "Assigned issue #{} to {}",
                            issue.meta.id,
                            overlay::IssuesPanelOverlay::session_id()
                        )
                    }),
                    "release" => store_for_task
                        .release(id, &sid, None)
                        .await
                        .map(|_| format!("Released issue #{id}")),
                    "close" => store_for_task.close(id, &sid, None).await.map(|issue| {
                        format!("Closed issue #{}: {}", issue.meta.id, issue.meta.title)
                    }),
                    "reopen" => store_for_task.reopen(id, None).await.map(|issue| {
                        format!("Reopened issue #{}: {}", issue.meta.id, issue.meta.title)
                    }),
                    _ => unreachable!(),
                };
                let line = match result {
                    Ok(msg) => msg,
                    Err(e) => format!("{op} failed: {e}"),
                };
                let _ = tx.send(UiEvent::SystemMessage(line));
            });
        }
        "help" | "?" => {
            let help = "\
`/issue`                  Open the issues panel
`/issue <id>`             Open panel with #id pre-selected
`/issue new <title>`      Create a new issue (default priority: medium)
                          e.g. `/issue new Fix login bug [high] [bug,auth]`
`/issue start <id>`       Claim an issue for the current TUI session
`/issue release <id>`     Release your claim
`/issue close <id>`       Close the issue (must be owner)
`/issue reopen <id>`      Reopen a closed issue
";
            state.overlay = None;
            state.overlay_state = Some(Box::new(overlay::text_viewer::TextViewerOverlay::new(
                " Issue Help ",
                help.to_string(),
            )));
        }
        other => {
            state.add_notification(
                format!("Unknown /issue subcommand: `{other}`. Try `/issue help`."),
                NotificationKind::Warning,
            );
        }
    }
    true
}

/// Parse the trailing `[key=value]` or `[bare]` tokens after a
/// `/issue new <title>` line. Strips up to two bracketed tokens from the end
/// of the title and returns the cleaned title plus a map of parsed options.
fn parse_new_opts(input: &str) -> (String, Option<HashMap<String, String>>) {
    let mut tokens: Vec<&str> = input.split_whitespace().collect();
    let mut opts = HashMap::new();
    // Bare bracket tokens (no `=`) get accumulated as labels in document
    // order; key=value tokens get inserted into the map. We pop from the end
    // until we hit a non-bracket token or run out.
    let mut ordered_labels: Vec<String> = Vec::new();
    while tokens.len() > 1 {
        let Some(inner) = tokens
            .last()
            .and_then(|s| s.strip_prefix('['))
            .and_then(|s| s.strip_suffix(']'))
        else {
            break;
        };
        if let Some((k, v)) = inner.split_once('=') {
            opts.insert(k.trim().to_string(), v.trim().to_string());
        } else {
            ordered_labels.push(inner.trim().to_string());
        }
        tokens.pop();
    }
    // Re-scan original input for any leading bare-bracket labels (in document
    // order — the LIFO pop above reverses them).
    if !ordered_labels.is_empty() {
        ordered_labels.reverse();
    }
    let bulk_labels = opts.remove("labels");
    let mut labels_vec: Vec<String> = Vec::new();
    if let Some(bulk) = bulk_labels {
        labels_vec.extend(
            bulk.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    labels_vec.extend(ordered_labels);
    if opts.is_empty() && labels_vec.is_empty() {
        return (input.to_string(), None);
    }
    if !labels_vec.is_empty() {
        opts.insert("labels".to_string(), labels_vec.join(","));
    }
    let title = tokens.join(" ");
    (title, Some(opts))
}

/// Parse priority strings case-insensitively. Returns None on unknown input.
fn parse_priority_loose(s: &str) -> Option<Priority> {
    match s.to_lowercase().as_str() {
        "low" | "l" => Some(Priority::Low),
        "medium" | "med" | "m" | "default" => Some(Priority::Medium),
        "high" | "h" => Some(Priority::High),
        "critical" | "crit" | "c" | "urgent" => Some(Priority::Critical),
        _ => None,
    }
}

/// Parse a numeric id token, returning None on invalid input.
fn parse_id(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok()
}
