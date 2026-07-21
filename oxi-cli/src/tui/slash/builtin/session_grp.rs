//! Session-lifecycle commands: `/name`, `/new`, `/clone`, `/resume`, `/fork`,
//! `/tree`. Migrated off the legacy `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::tui::app::{AppState, NotificationKind};
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// Resolve a user-provided selector to a full entry ID (for `/fork`).
///
/// Accepts: a 1-based number, a short ID prefix, or a full UUID.
fn resolve_entry_id(sel: &str, entries: &[&crate::store::session::SessionEntry]) -> Option<String> {
    // Try numeric index first (1-based)
    if let Ok(idx) = sel.parse::<usize>()
        && idx >= 1
        && idx <= entries.len()
    {
        return Some(entries[idx - 1].id.clone());
    }
    // Try prefix match or full match on entry IDs
    for entry in entries {
        if entry.id == sel || entry.id.starts_with(sel) {
            return Some(entry.id.clone());
        }
    }
    None
}

/// Collect all entries from a session tree (for `/tree`).
fn collect_tree_entries(
    roots: &[crate::store::session::SessionTreeNode],
) -> Vec<crate::store::session::SessionEntry> {
    let mut entries = Vec::new();
    fn visit(
        node: &crate::store::session::SessionTreeNode,
        entries: &mut Vec<crate::store::session::SessionEntry>,
    ) {
        entries.push(node.entry.clone());
        for child in &node.children {
            visit(child, entries);
        }
    }
    for root in roots {
        visit(root, &mut entries);
    }
    entries
}

/// `/name <name>` — set the session display name.
pub(crate) struct NameCommand;

impl SlashCommand for NameCommand {
    fn name(&self) -> &str {
        "name"
    }
    fn description(&self) -> &str {
        "Set session display name"
    }
    fn usage(&self) -> &str {
        "/name <name>"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let name = args.trim();
        if name.is_empty() {
            ctx.state
                .add_notification("/name <name>".to_string(), NotificationKind::Info);
        } else {
            ctx.session.set_session_name(name.to_string());
            ctx.state
                .add_notification(format!("Session: {}", name), NotificationKind::Success);
        }
        SlashOutcome::Handled
    }
}

/// `/new` — start a new session.
pub(crate) struct NewCommand;

impl SlashCommand for NewCommand {
    fn name(&self) -> &str {
        "new"
    }
    fn description(&self) -> &str {
        "Start a new session"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        // Reload settings so the next session uses latest config
        let fresh = crate::store::settings::Settings::load().unwrap_or_default();
        ctx.session.set_thinking_level(fresh.thinking_level);
        if let Some(m) = fresh.effective_model(None)
            && !m.is_empty()
        {
            // effective_model may already include the provider ("provider/model")
            // or be just a model id. Only prepend provider when needed.
            let full_id = if m.contains('/') {
                m.clone()
            } else {
                let p = fresh.effective_provider(None).unwrap_or_default();
                format!("{}/{}", p, m)
            };
            if let Ok(()) = ctx.session.set_model(&full_id) {
                let parts: Vec<&str> = full_id.splitn(2, '/').collect();
                ctx.state.footer_state.data.model_name = full_id.clone();
                if parts.len() == 2 {
                    ctx.state.footer_state.data.provider_name = parts[0].to_string();
                }
            }
        }
        ctx.state.chat.clear();
        ctx.session.reset();
        ctx.state
            .add_notification("New session started".to_string(), NotificationKind::Success);
        ctx.state.next_action = Some(crate::tui::app::TuiNextAction::NewSession);
        SlashOutcome::Handled
    }
}

/// `/clone` — duplicate the current session at the current position.
pub(crate) struct CloneCommand;

impl SlashCommand for CloneCommand {
    fn name(&self) -> &str {
        "clone"
    }
    fn description(&self) -> &str {
        "Duplicate the current session at the current position"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        if let Some(ref path) = state.session_file_path {
            let cwd: String = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string());
            match crate::store::session::SessionManager::fork_from(path, &cwd, None) {
                Ok(new_sm) => {
                    if let Some(new_path) = new_sm.get_session_file() {
                        state.add_notification(
                            format!("Cloned: {}", new_path),
                            NotificationKind::Success,
                        );
                    } else {
                        state.add_notification(
                            "Session cloned".to_string(),
                            NotificationKind::Success,
                        );
                    }
                }
                Err(e) => {
                    state.add_notification(format!("Clone failed: {}", e), NotificationKind::Error);
                }
            }
        } else {
            state.add_notification("No session to clone.".to_string(), NotificationKind::Info);
        }
        SlashOutcome::Handled
    }
}

/// `/resume` — resume a different session.
pub(crate) struct ResumeCommand;

impl SlashCommand for ResumeCommand {
    fn name(&self) -> &str {
        "resume"
    }
    fn description(&self) -> &str {
        "Resume a different session"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        // SessionManager::list is async but only does std::fs I/O.
        // Use spawn_blocking to avoid "Cannot start a runtime from within
        // a runtime" panic when called from inside the tokio runtime.
        let list_result = std::thread::scope(|s| {
            s.spawn(|| {
                // Build a temp runtime on this OS thread — safe because it's
                // a fresh thread, not the TUI's tokio worker thread.
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build temp runtime");
                rt.block_on(crate::store::session::SessionManager::list(&cwd, None))
            })
            .join()
            .unwrap_or_else(|e| {
                let msg = e
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| e.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                Err(anyhow::anyhow!("thread panicked: {}", msg))
            })
        });
        match list_result {
            Ok(sessions) if sessions.is_empty() => {
                state.add_notification(
                    "No previous sessions found.".to_string(),
                    NotificationKind::Info,
                );
            }
            Ok(sessions) => {
                let recent: Vec<_> = sessions.into_iter().take(15).collect();
                state.overlay = None;
                state.overlay_state = Some(overlay::resume_select(recent));
            }
            Err(e) => {
                state.add_notification(
                    format!("Error listing sessions: {}", e),
                    NotificationKind::Error,
                );
            }
        }
        SlashOutcome::Handled
    }
}

/// `/fork [selector]` — create a new fork from a previous user message.
pub(crate) struct ForkCommand;

impl SlashCommand for ForkCommand {
    fn name(&self) -> &str {
        "fork"
    }
    fn description(&self) -> &str {
        "Create a new fork from a previous user message"
    }
    fn usage(&self) -> &str {
        "/fork [<number>|<id>]"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        if let Some(ref path) = state.session_file_path {
            let sm = crate::store::session::SessionManager::open(path, None, None);
            let branch = sm.get_branch(None);
            let user_entries: Vec<_> = branch.iter().filter(|e| e.message.is_user()).collect();

            let sel = args.trim();
            if !sel.is_empty() {
                // Resolve the user's selection to a full entry ID.
                // Accept: number (1-based index), short ID (prefix), or full ID.
                let resolved_id = resolve_entry_id(sel, &user_entries);
                match resolved_id {
                    Some(full_id) => match sm.branch_from_entry(&full_id) {
                        Ok(new_path) => {
                            state.next_action =
                                Some(crate::tui::app::TuiNextAction::SwitchSession(new_path));
                            state.add_notification(
                                format!("Forked from [{}]", &full_id[..8.min(full_id.len())]),
                                NotificationKind::Success,
                            );
                        }
                        Err(e) => {
                            state.add_notification(
                                format!("Error forking: {}", e),
                                NotificationKind::Error,
                            );
                        }
                    },
                    None => {
                        state.add_notification(
                            format!("Entry not found: {}", sel),
                            NotificationKind::Warning,
                        );
                    }
                }
            } else {
                // No arg: open interactive fork selector overlay
                if user_entries.is_empty() {
                    state.add_notification(
                        "No user messages to fork from.".to_string(),
                        NotificationKind::Info,
                    );
                } else {
                    let entries: Vec<(String, String)> = user_entries
                        .iter()
                        .map(|e| {
                            let preview: String = e.content().chars().take(60).collect();
                            (e.id.clone(), preview)
                        })
                        .collect();
                    #[allow(clippy::arc_with_non_send_sync)]
                    let shared = std::sync::Arc::new(std::sync::Mutex::new(state as *mut AppState));
                    state.overlay_state = Some(Box::new(overlay::ForkSelectOverlay::new(
                        entries,
                        session.clone_handle(),
                        shared,
                    )));
                }
            }
        } else {
            state.add_notification(
                "No session file available.".to_string(),
                NotificationKind::Info,
            );
        }
        SlashOutcome::Handled
    }
}
pub(crate) struct TreeCommand;

impl SlashCommand for TreeCommand {
    fn name(&self) -> &str {
        "tree"
    }
    fn description(&self) -> &str {
        "Show session tree structure"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        if let Some(ref path) = state.session_file_path {
            let sm = crate::store::session::SessionManager::open(path, None, None);
            match sm.get_tree(uuid::Uuid::nil()) {
                Ok(roots) => {
                    if roots.is_empty() {
                        state
                            .add_notification("Empty session.".to_string(), NotificationKind::Info);
                    } else {
                        // Collect all entries from the tree for the overlay
                        let entries = collect_tree_entries(&roots);
                        state.overlay_state = Some(overlay::tree_navigator(
                            entries, None, // current leaf detection
                            session, state,
                        ));
                    }
                }
                Err(e) => {
                    state.add_notification(
                        format!("Error reading tree: {}", e),
                        NotificationKind::Error,
                    );
                }
            }
        } else {
            state.add_notification(
                "No session file available.".to_string(),
                NotificationKind::Info,
            );
        }
        SlashOutcome::Handled
    }
}
