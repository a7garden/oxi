//! Slash command handling.

use super::app::{AppOverlay, AppState, SetupStep, UiEvent};
use crate::app::agent_session::{AgentSession, ScopedModel};
use crate::media::clipboard_write;
use crate::storage::export::{self, ExportMeta, HtmlExportOptions};
use oxi_tui::widgets::chat::{ContentBlock, MessageRole};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// A slash command completion entry.
pub(crate) struct SlashCompletion {
    pub name: String,
    pub description: String,
}

/// Handle a slash command. Returns `true` if handled.
pub(crate) fn handle_slash_command(
    input: &str,
    session: &AgentSession,
    state: &mut AppState,
    running: &mut bool,
    ui_tx: &mpsc::UnboundedSender<UiEvent>,
) -> bool {
    let trimmed = input.trim();
    let (cmd, arg) = if let Some(space) = trimmed.find(' ') {
        (&trimmed[..space], Some(trimmed[space + 1..].trim()))
    } else {
        (trimmed, None)
    };
    let cmd_lower = cmd.to_lowercase();

    match cmd_lower.as_str() {
        "/help" | "/?" => {
            state.add_system_message(format_help());
            true
        }
        "/quit" | "/exit" | "/q" => {
            *running = false;
            true
        }
        "/model" => {
            if let Some(model_id) = arg {
                match session.set_model(model_id) {
                    Ok(()) => {
                        state.add_system_message(format!("Model: {}", model_id));
                        state.footer_state.data.model_name = model_id.to_string();
                        oxi_store::settings::Settings::save_last_used(model_id);
                    }
                    Err(e) => {
                        state.add_system_message(format!("Error: {}", e));
                    }
                }
            } else {
                let auth = oxi_store::auth_storage::shared_auth_storage();
                let all_models: Vec<String> = oxi_ai::model_db::get_all_models()
                    .filter(|entry| auth.get_api_key(entry.provider).is_some())
                    .map(|entry| format!("{}/{}", entry.provider, entry.id))
                    .collect();
                if all_models.is_empty() {
                    state.add_system_message(format!(
                        "Model: {}\n/model <provider/model> to switch",
                        session.model_id()
                    ));
                } else {
                    state.overlay = None;
                    state.overlay_state =
                        Some(super::overlay::model_select(all_models, session, state));
                }
            }
            true
        }
        "/compact" => {
            let instructions = arg.map(|s| s.to_string());
            let sh = session.clone_handle();
            let tx = ui_tx.clone();
            // The compaction events (CompactionStart/CompactionEnd) will be
            // emitted by AgentSession::compact() and handled in handle_ui_event.
            tokio::spawn(async move {
                let result = sh.compact(instructions).await;
                // For manual compaction, CompactionEnd is emitted by AgentSession.
                // But we also send the result via SystemMessage as a fallback.
                let _ = match &result {
                    Ok(r) => tx.send(UiEvent::SystemMessage(format!(
                        "Compacted from {} tokens",
                        r.tokens_before
                    ))),
                    Err(e) => tx.send(UiEvent::SystemMessage(format!("Compaction failed: {}", e))),
                };
            });
            true
        }
        "/session" => {
            let stats = session.session_stats();
            state.add_system_message(format!(
                "Session: {}\nMessages: {} ({} user, {} assistant)\nTools: {} calls, {} results\nModel: {}\nThinking: {:?}\nAuto-compact: {}\nAuto-retry: {}",
                stats.session_id, stats.total_messages, stats.user_messages, stats.assistant_messages,
                stats.tool_calls, stats.tool_results, session.model_id(),
                session.thinking_level(), session.auto_compaction_enabled(), session.auto_retry_enabled(),
            ));
            true
        }
        "/settings" => {
            state.add_system_message(format!(
                "Model: {}\nThinking: {:?}\nAuto-compact: {}\nAuto-retry: {}",
                session.model_id(),
                session.thinking_level(),
                session.auto_compaction_enabled(),
                session.auto_retry_enabled(),
            ));
            true
        }
        "/tools" => {
            let registry = session.agent_ref().tools();
            let names = registry.names();
            if let Some(action) = arg {
                handle_tool_command(action, &registry, state);
            } else {
                let mut out = "Tools:\n\n".to_string();
                for name in &names {
                    if let Some(tool) = registry.get(name) {
                        out.push_str(&format!("  {} — {}\n", name, tool.label()));
                    }
                }
                out.push_str("\n/tools <name>        Toggle tool on/off");
                state.add_system_message(out);
            }
            true
        }
        "/extensions" | "/ext" => {
            let registry = session.agent_ref().tools();
            let names = registry.names();
            let mut essential_lines = Vec::new();
            let mut optional_lines = Vec::new();
            let mut wasm_lines = Vec::new();
            let builtin_names: std::collections::HashSet<&str> = [
                "read",
                "write",
                "edit",
                "bash",
                "grep",
                "find",
                "ls",
                "web_search",
                "get_search_results",
                "github",
                "subagent",
            ]
            .into_iter()
            .collect();
            for name in &names {
                if let Some(tool) = registry.get(name) {
                    let line = format!("  {} — {}", name, tool.label());
                    if tool.essential() {
                        essential_lines.push(format!("{} [essential]", line));
                    } else if builtin_names.contains(name.as_str()) {
                        optional_lines.push(format!("{} [toggle]", line));
                    } else {
                        wasm_lines.push(format!("{} [wasm]", line));
                    }
                }
            }
            let mut out = "Essential (always on):\n\n".to_string();
            for line in &essential_lines {
                out.push_str(line);
                out.push('\n');
            }
            out.push_str("\nOptional (toggle with /tools <name>):\n\n");
            for line in &optional_lines {
                out.push_str(line);
                out.push('\n');
            }
            if !wasm_lines.is_empty() {
                out.push_str("\nWASM Extensions:\n\n");
                for line in &wasm_lines {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out.push_str("\n\nPlace .wasm files in ~/.oxi/extensions/ to add extensions");
            state.add_system_message(out);
            true
        }
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                state.add_system_message(format!("Session: {}", name));
            } else {
                state.add_system_message("/name <name>".to_string());
            }
            true
        }
        "/copy" => {
            if let Some(ref code) = state.chat.last_code_block {
                match clipboard_write::copy_to_clipboard(code) {
                    Ok(()) => {
                        state.add_system_message("OK: Code block copied to clipboard".to_string())
                    }
                    Err(e) => state.add_system_message(format!("Error: Copy failed: {}", e)),
                }
            } else {
                let last = state
                    .messages()
                    .iter()
                    .rev()
                    .find(|m| m.role == MessageRole::Assistant);
                if let Some(msg) = last {
                    let content: String = msg
                        .content_blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { content } => Some(content.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if content.trim().is_empty() {
                        state.add_system_message(
                            "No text content to copy (last message has no text)".to_string(),
                        );
                    } else {
                        match clipboard_write::copy_to_clipboard(&content) {
                            Ok(()) => {
                                state.add_system_message("OK: Copied to clipboard".to_string())
                            }
                            Err(e) => {
                                state.add_system_message(format!("Error: Copy failed: {}", e))
                            }
                        }
                    }
                } else {
                    state.add_system_message("No assistant message".to_string());
                }
            }
            true
        }
        "/changelog" => {
            let paths = vec![
                PathBuf::from("CHANGELOG.md"),
                PathBuf::from("../CHANGELOG.md"),
            ];
            let mut entries: Vec<crate::ui::changelog::ChangelogEntry> = Vec::new();
            for path in &paths {
                let parsed = crate::ui::changelog::parse_changelog(path);
                if !parsed.is_empty() {
                    entries = parsed;
                    break;
                }
            }
            if entries.is_empty() {
                state.add_system_message("No changelog found".to_string());
            } else {
                let mut out = "Changelog:\n\n".to_string();
                for entry in entries.iter().take(5) {
                    out.push_str(&format!("## {}\n\n", entry.version_string()));
                    let preview = if entry.content.len() > 200 {
                        let end = entry
                            .content
                            .char_indices()
                            .take_while(|(i, _)| *i < 200)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(0);
                        format!("{}...", &entry.content[..end])
                    } else {
                        entry.content.clone()
                    };
                    out.push_str(&preview);
                    out.push_str("\n\n");
                }
                state.add_system_message(out);
            }
            true
        }
        "/hotkeys" | "/keys" => {
            state.add_system_message(format_hotkeys());
            true
        }
        "/export" => {
            let export_path = arg.map(PathBuf::from);
            let meta = ExportMeta {
                model: Some(session.model_id()),
                provider: None,
                exported_at: chrono::Utc::now().timestamp_millis(),
                total_user_tokens: None,
                total_assistant_tokens: None,
            };
            let entries: Vec<oxi_store::session::SessionEntry> = state
                .messages()
                .iter()
                .map(|msg| {
                    let role = match msg.role {
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                        MessageRole::System => "system",
                    };
                    let content: String = msg
                        .content_blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { content } => Some(content.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    oxi_store::session::SessionEntry::simple_message(role, &content)
                })
                .collect();
            match export::export_to_html(&entries, &meta, &HtmlExportOptions::default()) {
                Ok(html) => {
                    if let Some(path) = export_path {
                        match std::fs::write(&path, &html) {
                            Ok(()) => state
                                .add_system_message(format!("OK: Exported: {}", path.display())),
                            Err(e) => {
                                state.add_system_message(format!("Error: Write failed: {}", e))
                            }
                        }
                    } else {
                        // Auto-save to CWD with session-based filename
                        let sid = session.session_id();
                        let short_sid = &sid[..8.min(sid.len())];
                        let default_name = format!("oxi-export-{}.html", short_sid);
                        match std::fs::write(&default_name, &html) {
                            Ok(()) => state.add_system_message(format!(
                                "OK: Exported: {} ({} bytes)",
                                default_name,
                                html.len()
                            )),
                            Err(e) => state.add_system_message(format!(
                                "Error: Write failed: {}. /export <path> to save manually.",
                                e
                            )),
                        }
                    }
                }
                Err(e) => state.add_system_message(format!("Error: Export failed: {}", e)),
            }
            true
        }
        "/import" => {
            if let Some(path) = arg {
                let cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".to_string());
                match oxi_store::session::resolve_session_path(path, &cwd) {
                    Ok(resolved) => {
                        if !std::path::Path::new(&resolved).exists() {
                            state.add_system_message(format!("File not found: {}", resolved));
                        } else {
                            state.next_action =
                                Some(super::app::TuiNextAction::SwitchSession(resolved.clone()));
                            state.add_system_message(format!(
                                "Importing session from {}...",
                                resolved
                            ));
                        }
                    }
                    Err(e) => {
                        state.add_system_message(format!("Error resolving path: {}", e));
                    }
                }
            } else {
                state.add_system_message("/import <path-to-jsonl>".to_string());
            }
            true
        }
        "/fork" => {
            if let Some(ref path) = state.session_file_path {
                let sm = oxi_store::session::SessionManager::open(path, None, None);
                let branch = sm.get_branch(None);
                let user_entries: Vec<_> = branch.iter().filter(|e| e.message.is_user()).collect();

                if let Some(sel) = arg {
                    // Resolve the user's selection to a full entry ID.
                    // Accept: number (1-based index), short ID (prefix), or full ID.
                    let resolved_id = resolve_entry_id(sel, &user_entries);
                    match resolved_id {
                        Some(full_id) => match sm.branch_from_entry(&full_id) {
                            Ok(new_path) => {
                                state.next_action =
                                    Some(super::app::TuiNextAction::SwitchSession(new_path));
                                state.add_system_message(format!(
                                    "Forked from [{}]\nStarting new session...",
                                    &full_id[..8.min(full_id.len())]
                                ));
                            }
                            Err(e) => {
                                state.add_system_message(format!("Error forking: {}", e));
                            }
                        },
                        None => {
                            state.add_system_message(format!(
                                "Entry not found: {}\nUse /fork to list available messages.",
                                sel
                            ));
                        }
                    }
                } else {
                    if user_entries.is_empty() {
                        state.add_system_message("No user messages to fork from.".to_string());
                    } else {
                        let mut out = "Fork from which message?\n\n".to_string();
                        for (i, entry) in user_entries.iter().enumerate() {
                            let preview: String = entry.content().chars().take(60).collect();
                            let short_id = &entry.id[..8.min(entry.id.len())];
                            out.push_str(&format!("  {}. [{}] {}\n", i + 1, short_id, preview));
                        }
                        out.push_str("\n/fork <number or id> to fork from a message");
                        state.add_system_message(out);
                    }
                }
            } else {
                state.add_system_message("No session file available.".to_string());
            }
            true
        }
        "/clone" => {
            if let Some(ref path) = state.session_file_path {
                let cwd: String = std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".to_string());
                match oxi_store::session::SessionManager::fork_from(path, &cwd, None) {
                    Ok(new_sm) => {
                        if let Some(new_path) = new_sm.get_session_file() {
                            state.add_system_message(format!(
                                "Cloned session: {}\nUse /resume to open it.",
                                new_path
                            ));
                        } else {
                            state.add_system_message(
                                "Session cloned. Use /resume to open it.".to_string(),
                            );
                        }
                    }
                    Err(e) => {
                        state.add_system_message(format!("Error cloning session: {}", e));
                    }
                }
            } else {
                state.add_system_message("No active session to clone.".to_string());
            }
            true
        }
        "/tree" => {
            if let Some(ref path) = state.session_file_path {
                let sm = oxi_store::session::SessionManager::open(path, None, None);
                match sm.get_tree(uuid::Uuid::nil()) {
                    Ok(roots) => {
                        if roots.is_empty() {
                            state.add_system_message("Empty session.".to_string());
                        } else {
                            let mut out = "Session tree:\n\n".to_string();
                            fn render_node(
                                node: &oxi_store::session::SessionTreeNode,
                                depth: usize,
                                out: &mut String,
                            ) {
                                let indent = "  ".repeat(depth);
                                let role = match &node.entry.message {
                                    oxi_store::session::AgentMessage::User { .. } => "U",
                                    oxi_store::session::AgentMessage::Assistant { .. } => "A",
                                    _ => "-",
                                };
                                let preview: String =
                                    node.entry.content().chars().take(50).collect();
                                let label = node
                                    .label
                                    .as_ref()
                                    .map(|l| format!(" [{}]", l))
                                    .unwrap_or_default();
                                let short_id = &node.entry.id[..8.min(node.entry.id.len())];
                                out.push_str(&format!(
                                    "{}{} [{}] {}{}\n",
                                    indent, role, short_id, preview, label
                                ));
                                for child in &node.children {
                                    render_node(child, depth + 1, out);
                                }
                            }
                            for root in &roots {
                                render_node(root, 0, &mut out);
                            }
                            state.add_system_message(out);
                        }
                    }
                    Err(e) => {
                        state.add_system_message(format!("Error reading tree: {}", e));
                    }
                }
            } else {
                state.add_system_message("No session file available.".to_string());
            }
            true
        }
        "/provider" => {
            if let Some(provider) = arg {
                let parts: Vec<&str> = provider.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    try_provider_with_key(parts[0], parts[1], state);
                } else {
                    state.overlay = Some(AppOverlay::ProviderConfig(SetupStep::EnterApiKey {
                        provider: parts[0].to_string(),
                        key: String::new(),
                        masked_cursor: 0,
                    }));
                }
            } else {
                state.overlay = Some(AppOverlay::ProviderConfig(SetupStep::SelectAuthType {
                    auth_type: None,
                    selected: 0,
                }));
            }
            true
        }
        "/logout" => {
            if let Some(provider) = arg {
                oxi_store::auth_storage::shared_auth_storage().remove(provider);
                state.add_system_message(format!("OK: Removed {}", provider));
            } else {
                let auth = oxi_store::auth_storage::shared_auth_storage();
                let providers = auth.configured_providers();
                if providers.is_empty() {
                    state.add_system_message("No providers configured.".to_string());
                } else {
                    state.overlay = None;
                    state.overlay_state = Some(super::overlay::logout_select(providers, state));
                }
            }
            true
        }
        "/new" => {
            // Reload settings so the next session uses latest config
            let fresh = oxi_store::settings::Settings::load().unwrap_or_default();
            session.set_thinking_level(fresh.thinking_level);
            if let Some(m) = fresh.effective_model(None) {
                if !m.is_empty() {
                    // effective_model may already include the provider ("provider/model")
                    // or be just a model id. Only prepend provider when needed.
                    let full_id = if m.contains('/') {
                        m.clone()
                    } else {
                        let p = fresh.effective_provider(None).unwrap_or_default();
                        format!("{}/{}", p, m)
                    };
                    if let Ok(()) = session.set_model(&full_id) {
                        let parts: Vec<&str> = full_id.splitn(2, '/').collect();
                        state.footer_state.data.model_name = full_id.clone();
                        if parts.len() == 2 {
                            state.footer_state.data.provider_name = parts[0].to_string();
                        }
                    }
                }
            }
            state.chat.clear();
            session.reset();
            state.next_action = Some(super::app::TuiNextAction::NewSession);
            true
        }
        "/resume" => {
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
                    rt.block_on(oxi_store::session::SessionManager::list(&cwd, None))
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
                    state.add_system_message("No previous sessions found.".to_string());
                }
                Ok(sessions) => {
                    let recent: Vec<_> = sessions.into_iter().take(15).collect();
                    state.overlay = None;
                    state.overlay_state = Some(super::overlay::resume_select(recent));
                }
                Err(e) => {
                    state.add_system_message(format!("Error listing sessions: {}", e));
                }
            }
            true
        }
        "/reload" => {
            let reloaded = oxi_store::settings::Settings::load().unwrap_or_default();
            let theme_name = reloaded.theme.clone();
            session.set_thinking_level(reloaded.thinking_level);
            // Apply model change to the active agent session
            if let Some(m) = reloaded.effective_model(None) {
                if !m.is_empty() {
                    // effective_model may already include provider ("provider/model")
                    let full_id = if m.contains('/') {
                        m
                    } else {
                        let p = reloaded.effective_provider(None).unwrap_or_default();
                        format!("{}/{}", p, m)
                    };
                    match session.set_model(&full_id) {
                        Ok(()) => {
                            let parts: Vec<&str> = full_id.splitn(2, '/').collect();
                            state.footer_state.data.model_name = full_id.clone();
                            if parts.len() == 2 {
                                state.footer_state.data.provider_name = parts[0].to_string();
                            }
                        }
                        Err(e) => {
                            state.add_system_message(format!(
                                "Warning: Could not apply model: {}",
                                e
                            ));
                        }
                    }
                }
            }
            state.add_system_message(format!(
                "OK: Reloaded configuration\n  Model: {}\n  Provider: {}\n  Theme: {}\n  Thinking: {:?}\n  Extensions: {}\n  Stream: {}\n  Auto-compact: {}",
                state.footer_state.data.model_name,
                state.footer_state.data.provider_name,
                theme_name, reloaded.thinking_level,
                reloaded.extensions_enabled, reloaded.stream_responses, reloaded.auto_compaction,
            ));
            true
        }
        "/scoped-models" | "/models" => {
            if let Some(models_str) = arg {
                let models: Vec<ScopedModel> = models_str
                    .split(',')
                    .filter_map(|s| {
                        let parts: Vec<&str> = s.trim().split('/').collect();
                        if parts.len() >= 2 {
                            Some(ScopedModel {
                                provider: parts[0].to_string(),
                                model_id: parts[1..].join("/"),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                if !models.is_empty() {
                    session.set_scoped_models(models.clone());
                    let names: Vec<String> = models
                        .iter()
                        .map(|m| format!("{}/{}", m.provider, m.model_id))
                        .collect();
                    state.add_system_message(format!(
                        "Scoped: {} (Ctrl+P to cycle)",
                        names.join(", ")
                    ));
                } else {
                    state.add_system_message(
                        "/scoped-models provider/model1,provider/model2".to_string(),
                    );
                }
            } else {
                let scoped = session.scoped_models();
                if scoped.is_empty() {
                    state.add_system_message(
                        "No scoped models. /scoped-models <m1>,<m2>".to_string(),
                    );
                } else {
                    let names: Vec<String> = scoped
                        .iter()
                        .map(|m| format!("{}/{}", m.provider, m.model_id))
                        .collect();
                    state.add_system_message(format!("Scoped: {}", names.join(", ")));
                }
            }
            true
        }
        _ => {
            // Check WASM extension commands
            if let Some(ref wasm_ext) = state.wasm_ext {
                let commands = wasm_ext.all_command_defs();
                let name = cmd.strip_prefix('/').unwrap_or(cmd);
                if let Some((ext_name, _cmd)) = commands.iter().find(|(_, c)| c.name == name) {
                    let output = wasm_ext
                        .execute_command(name, arg.unwrap_or(""))
                        .unwrap_or_else(|e| format!("Error: {}", e));
                    state.add_system_message(format!("[{}] {}", ext_name, output));
                    return true;
                }
            }
            state.add_system_message(format!(
                "Unknown command: {}\nType /help for available commands.",
                cmd
            ));
            false
        }
    }
}

// ── Entry ID resolution for /fork ─────────────────────────────────────────

/// Resolve a user-provided selector to a full entry ID.
///
/// Accepts:
/// - A 1-based number ("1", "2", ...) matching the displayed list
/// - A short ID prefix ("abc12345") that matches the start of a full ID
/// - A full UUID
///
/// Returns `None` if nothing matches.
fn resolve_entry_id(sel: &str, entries: &[&oxi_store::session::SessionEntry]) -> Option<String> {
    // Try numeric index first (1-based)
    if let Ok(idx) = sel.parse::<usize>() {
        if idx >= 1 && idx <= entries.len() {
            return Some(entries[idx - 1].id.clone());
        }
    }

    // Try prefix match or full match on entry IDs
    for entry in entries {
        if entry.id == sel || entry.id.starts_with(sel) {
            return Some(entry.id.clone());
        }
    }

    None
}

// ── Help text ────────────────────────────────────────────────────────────────

fn format_help() -> String {
    r#"
  Session
    /new              Start a new session
    /clone            Duplicate current session
    /resume           Resume a previous session
    /import <path>    Import session from JSONL
    /tree             Show session tree
    /fork             List messages to fork from
    /fork <number>    Fork from a message by list number
    /fork <id>        Fork from a specific message ID
    /session          Show session info
    /name <name>      Set session name

  Model
    /model [id]       Switch or show model
    /scoped-models    Models for Ctrl+P cycling

  Context
    /compact [instr]  Compact context

  Tools
    /tools            List active tools
    /tools <name>     Toggle tool on/off
    /extensions       List extensions & WASM tools
    /ext              Alias for /extensions

  Export
    /export [path]    Export to HTML
    /copy             Copy code block / last reply

  Auth
    /provider [name]  Configure API key
    /logout [name]    Remove key

  Info
    /help             This help
    /hotkeys          Key shortcuts
    /changelog        Changelog
    /settings         Current settings
    /reload           Reload config
    /quit             Quit

  Keys
    Enter             Send
    Ctrl+C            Interrupt / Quit
    PageUp/Down       Scroll
    /                 Slash commands
"#
    .to_string()
}

fn format_hotkeys() -> String {
    r#"
  Navigation
    Enter              Submit input
    Escape             Cancel
    PageUp/PageDown    Scroll chat

  Editor
    ←/→                Move cursor
    Home/End           Start/End of line
    Backspace          Delete char
    Ctrl+←/→           Move by word

  Session
    Ctrl+C             Interrupt / Quit
    Ctrl+Y             Copy last code block
    Ctrl+P             Cycle models
    Shift+Ctrl+P       Cycle models (reverse)
"#
    .to_string()
}

// ── Interactive login ────────────────────────────────────────────────────

/// /provider <provider> <key> — save the key directly
fn try_provider_with_key(provider: &str, key: &str, state: &mut AppState) -> bool {
    if key.is_empty() {
        return false;
    }
    let auth = oxi_store::auth_storage::shared_auth_storage();
    auth.set_api_key(provider, key.to_string());
    state.add_system_message(format!("API key for {} saved.", provider));
    true
}

/// API key masking (first 6 chars + ... + last 4 chars)
#[allow(dead_code)]
fn mask_key(key: &str) -> String {
    if key.len() <= 12 {
        return "***".to_string();
    }
    format!("{}...{}", &key[..6], &key[key.len() - 4..])
}

// ── Tool toggle ─────────────────────────────────────────────────────────

/// Built-in tool definitions (name) for toggle validation.
const BUILTIN_TOOL_NAMES: &[&str] = &[
    "read",
    "write",
    "edit",
    "bash",
    "grep",
    "find",
    "ls",
    "web_search",
    "get_search_results",
    "github",
    "subagent",
];

/// Handle `/tools <name>` — toggle a tool on/off.
fn handle_tool_command(
    action: &str,
    registry: &std::sync::Arc<oxi_agent::ToolRegistry>,
    state: &mut AppState,
) {
    let tool_name = action.trim().to_lowercase();

    let is_known =
        BUILTIN_TOOL_NAMES.contains(&tool_name.as_str()) || registry.get(&tool_name).is_some();

    if !is_known {
        state.add_system_message(format!(
            "Unknown tool: {}\nUse /tools to see available tools.",
            tool_name
        ));
        return;
    }

    if registry.get(&tool_name).is_some() {
        if let Some(tool) = registry.get(&tool_name) {
            if tool.essential() {
                state.add_system_message(format!("Cannot disable essential tool: {}", tool_name));
                return;
            }
        }
        registry.unregister(&tool_name);
        if tool_name == "web_search" {
            registry.unregister("get_search_results");
        }
        state.add_system_message(format!("OK: Tool disabled: {}", tool_name));
    } else {
        let re_registered = try_re_register_tool(&tool_name, registry);
        if re_registered {
            state.add_system_message(format!("OK: Tool enabled: {}", tool_name));
        } else {
            state.add_system_message(format!(
                "Cannot re-enable {}. Restart oxi to restore all tools.",
                tool_name
            ));
        }
    }
}

/// Try to re-register a previously disabled built-in tool.
fn try_re_register_tool(name: &str, registry: &std::sync::Arc<oxi_agent::ToolRegistry>) -> bool {
    use std::sync::Arc;

    match name {
        "read" | "write" | "edit" | "bash" | "grep" | "find" | "ls" => return false,
        "web_search" => {
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::WebSearchTool::new(cache.clone()));
            registry.register(oxi_agent::GetSearchResultsTool::new(cache));
        }
        "get_search_results" => {
            if registry.get("web_search").is_some() {
                return false;
            }
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::GetSearchResultsTool::new(cache));
        }
        "github" | "github_search" => {
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::GitHubTool::new(cache));
        }
        "subagent" => registry.register(oxi_agent::SubagentTool::with_cwd(
            std::path::PathBuf::from("."),
        )),
        _ => return false,
    }
    true
}
