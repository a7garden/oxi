//! Slash command handling.

use super::app::{AppOverlay, AppState, NotificationKind, SetupStep, UiEvent};
use super::overlay::router_integration;
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
            state.overlay = None;
            state.overlay_state = Some(super::overlay::help_overlay());
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
                        state.add_notification(
                            format!("Model: {}", model_id),
                            NotificationKind::Success,
                        );
                        state.footer_state.data.model_name = model_id.to_string();
                        oxi_store::settings::Settings::save_last_used(model_id);
                    }
                    Err(e) => {
                        state.add_notification(format!("Error: {}", e), NotificationKind::Error);
                    }
                }
            } else {
                let auth = oxi_store::auth_storage::shared_auth_storage();
                // Static models filtered by API key
                let mut all_models: Vec<String> = oxi_ai::model_db::get_all_models()
                    .filter(|entry| auth.get_api_key(entry.provider).is_some())
                    .map(|entry| format!("{}/{}", entry.provider, entry.id))
                    .collect();

                // Dynamic models (custom providers + router/auto)
                for dyn_model in oxi_ai::dynamic_models() {
                    let entry = format!("{}/{}", dyn_model.provider, dyn_model.id);
                    if !all_models.contains(&entry) {
                        all_models.push(entry);
                    }
                }
                if all_models.is_empty() {
                    state.add_notification(
                        format!("Model: {}", session.model_id()),
                        NotificationKind::Info,
                    );
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
            let content = format!(
                "Session: {}\n\nMessages: {} ({} user, {} assistant)\nTools: {} calls, {} results\n\nModel: {}\nThinking: {:?}\n\nAuto-compact: {}\nAuto-retry: {}",
                stats.session_id, stats.total_messages, stats.user_messages, stats.assistant_messages,
                stats.tool_calls, stats.tool_results, session.model_id(),
                session.thinking_level(), session.auto_compaction_enabled(), session.auto_retry_enabled(),
            );
            state.overlay = None;
            state.overlay_state = Some(Box::new(
                super::overlay::text_viewer::TextViewerOverlay::new(" Session Info ", content),
            ));
            true
        }
        "/settings" => {
            state.overlay_state = Some(super::overlay::settings_overlay(
                &session.clone_handle(),
                state,
            ));
            true
        }
        "/tools" => {
            let registry = session.agent_ref().tools();
            let names = registry.names();
            if let Some(action) = arg {
                handle_tool_command(action, &registry, state);
            } else {
                let mut out = "Available Tools:\n\n".to_string();
                for name in &names {
                    if let Some(tool) = registry.get(name) {
                        out.push_str(&format!("  {} — {}\n", name, tool.label()));
                    }
                }
                out.push_str("\n/tools <name>  Toggle tool on/off");
                state.overlay = None;
                state.overlay_state = Some(super::overlay::tools_overlay(out));
            }
            true
        }
        "/extensions" | "/ext" => {
            state.overlay = None;
            state.overlay_state = Some(super::overlay::extensions_overlay(session, state));
            true
        }
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                state.add_notification(format!("Session: {}", name), NotificationKind::Success);
            } else {
                state.add_notification("/name <name>".to_string(), NotificationKind::Info);
            }
            true
        }
        "/copy" => {
            if let Some(ref code) = state.chat.last_code_block {
                match clipboard_write::copy_to_clipboard(code) {
                    Ok(()) => state.add_notification(
                        "Code block copied to clipboard".to_string(),
                        NotificationKind::Success,
                    ),
                    Err(e) => state
                        .add_notification(format!("Copy failed: {}", e), NotificationKind::Error),
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
                        state.add_notification(
                            "No text content to copy".to_string(),
                            NotificationKind::Warning,
                        );
                    } else {
                        match clipboard_write::copy_to_clipboard(&content) {
                            Ok(()) => state.add_notification(
                                "Copied to clipboard".to_string(),
                                NotificationKind::Success,
                            ),
                            Err(e) => state.add_notification(
                                format!("Copy failed: {}", e),
                                NotificationKind::Error,
                            ),
                        }
                    }
                } else {
                    state.add_notification(
                        "No assistant message".to_string(),
                        NotificationKind::Info,
                    );
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
                state.add_notification("No changelog found".to_string(), NotificationKind::Info);
            } else {
                let changelog_entries: Vec<(String, String)> = entries
                    .iter()
                    .take(10)
                    .map(|e| (e.version_string(), e.content.clone()))
                    .collect();
                state.overlay = None;
                state.overlay_state = Some(super::overlay::changelog_overlay(changelog_entries));
            }
            true
        }
        "/hotkeys" | "/keys" => {
            state.overlay = None;
            state.overlay_state = Some(super::overlay::hotkeys_overlay());
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
                            Ok(()) => state.add_notification(
                                format!("Exported: {}", path.display()),
                                NotificationKind::Success,
                            ),
                            Err(e) => state.add_notification(
                                format!("Write failed: {}", e),
                                NotificationKind::Error,
                            ),
                        }
                    } else {
                        // Auto-save to CWD with session-based filename
                        let sid = session.session_id();
                        let short_sid = &sid[..8.min(sid.len())];
                        let default_name = format!("oxi-export-{}.html", short_sid);
                        match std::fs::write(&default_name, &html) {
                            Ok(()) => state.add_notification(
                                format!("Exported: {} ({} bytes)", default_name, html.len()),
                                NotificationKind::Success,
                            ),
                            Err(e) => state.add_notification(
                                format!("Write failed: {}", e),
                                NotificationKind::Error,
                            ),
                        }
                    }
                }
                Err(e) => {
                    state.add_notification(format!("Export failed: {}", e), NotificationKind::Error)
                }
            }
            true
        }
        "/share" => {
            // Check if gh CLI is available
            match std::process::Command::new("gh")
                .arg("auth")
                .arg("status")
                .output()
            {
                Ok(output) if output.status.success() => {
                    // Export session to HTML first
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
                            // Write to temp file and create gist
                            let temp_path = std::env::temp_dir().join("oxi-share.html");
                            match std::fs::write(&temp_path, &html) {
                                Ok(()) => {
                                    state.add_notification(
                                        "Creating Gist... (Esc to cancel)".to_string(),
                                        NotificationKind::Info,
                                    );
                                    // Show loader overlay during gist creation
                                    let _sh = session.clone_handle();
                                    let tx = ui_tx.clone();
                                    tokio::spawn(async move {
                                        let result = tokio::process::Command::new("gh")
                                            .args(["gist", "create", &temp_path.to_string_lossy()])
                                            .output()
                                            .await;
                                        let _ = std::fs::remove_file(&temp_path);
                                        match result {
                                            Ok(output) if output.status.success() => {
                                                let stdout =
                                                    String::from_utf8_lossy(&output.stdout);
                                                let gist_url = stdout.trim().to_string();
                                                let _ = tx.send(UiEvent::SystemMessage(format!(
                                                    "Gist created: {}",
                                                    gist_url
                                                )));
                                            }
                                            Ok(output) => {
                                                let stderr =
                                                    String::from_utf8_lossy(&output.stderr);
                                                let _ = tx.send(UiEvent::SystemMessage(format!(
                                                    "Gist failed: {}",
                                                    stderr.trim()
                                                )));
                                            }
                                            Err(e) => {
                                                let _ = tx.send(UiEvent::SystemMessage(format!(
                                                    "Gist failed: {}",
                                                    e
                                                )));
                                            }
                                        }
                                    });
                                }
                                Err(e) => {
                                    state.add_notification(
                                        format!("Error: {}", e),
                                        NotificationKind::Error,
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            state.add_notification(
                                format!("Export failed: {}", e),
                                NotificationKind::Error,
                            );
                        }
                    }
                }
                Ok(_output) => {
                    state.add_notification(
                        "GitHub CLI not authenticated. Run: gh auth login".to_string(),
                        NotificationKind::Warning,
                    );
                }
                Err(_e) => {
                    state.add_notification(
                        "GitHub CLI (gh) not found".to_string(),
                        NotificationKind::Error,
                    );
                }
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
                            state.add_notification(
                                format!("File not found: {}", resolved),
                                NotificationKind::Error,
                            );
                        } else {
                            state.next_action =
                                Some(super::app::TuiNextAction::SwitchSession(resolved.clone()));
                            state.add_notification(
                                format!("Importing session from {}...", resolved),
                                NotificationKind::Info,
                            );
                        }
                    }
                    Err(e) => {
                        state.add_notification(
                            format!("Error resolving path: {}", e),
                            NotificationKind::Error,
                        );
                    }
                }
            } else {
                state.add_notification(
                    "/import <path> [path-to-jsonl]".to_string(),
                    NotificationKind::Info,
                );
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
                        let shared = std::sync::Arc::new(std::sync::Mutex::new(
                            state as *mut super::app::AppState,
                        ));
                        state.overlay_state =
                            Some(Box::new(super::overlay::ForkSelectOverlay::new(
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
                        state.add_notification(
                            format!("Clone failed: {}", e),
                            NotificationKind::Error,
                        );
                    }
                }
            } else {
                state.add_notification("No session to clone.".to_string(), NotificationKind::Info);
            }
            true
        }
        "/tree" => {
            if let Some(ref path) = state.session_file_path {
                let sm = oxi_store::session::SessionManager::open(path, None, None);
                match sm.get_tree(uuid::Uuid::nil()) {
                    Ok(roots) => {
                        if roots.is_empty() {
                            state.add_notification(
                                "Empty session.".to_string(),
                                NotificationKind::Info,
                            );
                        } else {
                            // Collect all entries from the tree for the overlay
                            let entries = collect_tree_entries(&roots);
                            state.overlay_state = Some(super::overlay::tree_navigator(
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
        "/router" => {
            if let Some(sub) = arg {
                let mut parts = sub.split_whitespace();
                let cmd = parts.next().unwrap_or("");
                match cmd {
                    "status" => {
                        if let Some(snap) = oxi_ai::router::RouterProvider::get_snapshot() {
                            let content = format!(
                                "Router Status:\n\nProfile: {}\nTier: {:?}\nScore: {:.2}\nModel: {}\nProvider: {}\nCost: ${:.4}\nTurns: {}",
                                snap.profile.as_deref().unwrap_or("-"),
                                snap.last_tier.unwrap_or(oxi_ai::router::RouterTier::Medium),
                                snap.last_score,
                                snap.last_model.as_deref().unwrap_or("-"),
                                snap.last_provider.as_deref().unwrap_or("-"),
                                snap.accumulated_cost,
                                snap.turn_count,
                            );
                            state.overlay = None;
                            state.overlay_state = Some(Box::new(
                                super::overlay::text_viewer::TextViewerOverlay::new(
                                    " Router Status ",
                                    content,
                                ),
                            ));
                        } else {
                            state.add_notification(
                                "Router not active. Use /router to configure.".to_string(),
                                NotificationKind::Warning,
                            );
                        }
                    }
                    "pin" => {
                        // /router pin <low|medium|high|off>
                        if let Some(tier_arg) = parts.next() {
                            match tier_arg.to_lowercase().as_str() {
                                "low" => {
                                    oxi_ai::router::set_router_pin(Some(oxi_ai::router::RouterTier::Low));
                                    state.add_notification(
                                        "Router pinned to LOW tier".to_string(),
                                        NotificationKind::Success,
                                    );
                                }
                                "medium" => {
                                    oxi_ai::router::set_router_pin(Some(oxi_ai::router::RouterTier::Medium));
                                    state.add_notification(
                                        "Router pinned to MEDIUM tier".to_string(),
                                        NotificationKind::Success,
                                    );
                                }
                                "high" => {
                                    oxi_ai::router::set_router_pin(Some(oxi_ai::router::RouterTier::High));
                                    state.add_notification(
                                        "Router pinned to HIGH tier".to_string(),
                                        NotificationKind::Success,
                                    );
                                }
                                "off" | "none" | "clear" => {
                                    oxi_ai::router::set_router_pin(None);
                                    state.add_notification(
                                        "Router pin cleared (auto-routing resumed)".to_string(),
                                        NotificationKind::Success,
                                    );
                                }
                                _ => {
                                    state.add_notification(
                                        "Usage: /router pin <low|medium|high|off>".to_string(),
                                        NotificationKind::Info,
                                    );
                                }
                            }
                        } else {
                            let current = oxi_ai::router::get_router_pin();
                            let msg = match current {
                                Some(t) => format!("Router pin: {:?}", t),
                                None => "Router pin: none (auto)".to_string(),
                            };
                            state.add_notification(msg, NotificationKind::Info);
                        }
                    }
                    "disable" => {
                        // Switch away from router to the default model
                        let settings = oxi_store::settings::Settings::load().unwrap_or_default();
                        if let Some(default_model) = settings.effective_model(None) {
                            let full_id = if default_model.contains('/') {
                                default_model.clone()
                            } else {
                                let p = settings.effective_provider(None).unwrap_or_default();
                                format!("{}/{}", p, default_model)
                            };
                            match session.set_model(&full_id) {
                                Ok(()) => {
                                    state.footer_state.data.model_name = full_id.clone();
                                    state.add_notification(
                                        format!("Router disabled, using {}", full_id),
                                        NotificationKind::Success,
                                    );
                                }
                                Err(e) => {
                                    state.add_notification(
                                        format!("Error switching model: {}", e),
                                        NotificationKind::Error,
                                    );
                                }
                            }
                        } else {
                            state.add_notification(
                                "No default model configured".to_string(),
                                NotificationKind::Warning,
                            );
                        }
                    }
                    "enable" => {
                        // Switch to router/auto
                        match session.set_model("router/auto") {
                            Ok(()) => {
                                state.footer_state.data.model_name = "router/auto".to_string();
                                state.add_notification(
                                    "Router enabled (router/auto)".to_string(),
                                    NotificationKind::Success,
                                );
                            }
                            Err(e) => {
                                state.add_notification(
                                    format!("Error enabling router: {}", e),
                                    NotificationKind::Error,
                                );
                            }
                        }
                    }
                    _ => {
                        state.overlay = None;
                        state.overlay_state = Some(Box::new(
                            super::overlay::text_viewer::TextViewerOverlay::new(
                                " Router Help ",
                                router_help(),
                            ),
                        ));
                    }
                }
            } else {
                let global_dir = dirs::config_dir().unwrap_or_default().join("oxi");
                let project_dir = std::env::current_dir().unwrap_or_default();
                let has_config =
                    oxi_store::router_config::load_router_config(&global_dir, &project_dir)
                        .is_some();

                if has_config {
                    if let Some(snap) = oxi_ai::router::RouterProvider::get_snapshot() {
                        let content = format!(
                            "Router Status:\n\nProfile: {}\nTier: {:?}\nScore: {:.2}\nModel: {}\nCost: ${:.4}\nTurns: {}",
                            snap.profile.as_deref().unwrap_or("-"),
                            snap.last_tier.unwrap_or(oxi_ai::router::RouterTier::Medium),
                            snap.last_score,
                            snap.last_model.as_deref().unwrap_or("-"),
                            snap.accumulated_cost,
                            snap.turn_count,
                        );
                        state.overlay = None;
                        state.overlay_state = Some(Box::new(
                            super::overlay::text_viewer::TextViewerOverlay::new(
                                " Router Status ",
                                content,
                            ),
                        ));
                    } else {
                        state.add_notification(
                            "Router configured but not yet active".to_string(),
                            NotificationKind::Info,
                        );
                    }
                } else {
                    state.add_notification(
                        "Opening router setup...".to_string(),
                        NotificationKind::Info,
                    );
                    let auth = oxi_store::auth_storage::shared_auth_storage();
                    let setup_models: Vec<String> = oxi_ai::model_db::get_all_models()
                        .filter(|entry| auth.get_api_key(entry.provider).is_some())
                        .map(|entry| format!("{}/{}", entry.provider, entry.id))
                        .collect();
                    let initial = super::overlay::RouterSetupData {
                        profile_name: "auto".to_string(),
                        ..Default::default()
                    };
                    state.overlay = None;
                    state.overlay_state = Some(super::overlay::router_setup(
                        initial,
                        setup_models,
                        move |data: &super::overlay::RouterSetupData| {
                            let store_cfg = router_integration::save_router_config(data)?;
                            let ai_cfg = router_integration::store_config_to_ai_config(&store_cfg);
                            oxi_ai::router::register_router(&ai_cfg);
                            Ok(())
                        },
                        || {},
                    ));
                }
            }
            true
        }
        "/logout" => {
            if let Some(provider) = arg {
                oxi_store::auth_storage::shared_auth_storage().remove(provider);
                state.add_notification(format!("Removed {}", provider), NotificationKind::Success);
            } else {
                let auth = oxi_store::auth_storage::shared_auth_storage();
                let providers = auth.configured_providers();
                if providers.is_empty() {
                    state.add_notification(
                        "No providers configured.".to_string(),
                        NotificationKind::Info,
                    );
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
                    state.add_notification(
                        "No previous sessions found.".to_string(),
                        NotificationKind::Info,
                    );
                }
                Ok(sessions) => {
                    let recent: Vec<_> = sessions.into_iter().take(15).collect();
                    state.overlay = None;
                    state.overlay_state = Some(super::overlay::resume_select(recent));
                }
                Err(e) => {
                    state.add_notification(
                        format!("Error listing sessions: {}", e),
                        NotificationKind::Error,
                    );
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
                            state.add_notification(
                                format!("Warning: Could not apply model: {}", e),
                                NotificationKind::Warning,
                            );
                        }
                    }
                }
            }

            // Reload WASM extensions
            let ext_status = if reloaded.extensions_enabled {
                let cwd_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let wasm_paths = crate::extensions::WasmExtensionManager::discover(&cwd_path);
                if wasm_paths.is_empty() {
                    state.wasm_ext = None;
                    "No extensions found".to_string()
                } else {
                    let mut mgr = crate::extensions::WasmExtensionManager::new();
                    let (loaded, errors) = mgr.load_all(&wasm_paths);
                    let loaded_count = loaded.len();
                    let error_count = errors.len();
                    if mgr.is_empty() {
                        state.wasm_ext = None;
                        format!("0 loaded, {} error(s)", error_count)
                    } else {
                        // Unregister old WASM tools
                        let tools = session.agent_ref().tools();
                        let old_names: Vec<String> = if let Some(ref old_ext) = state.wasm_ext {
                            old_ext.all_tool_defs().iter().map(|d| d.name.clone()).collect()
                        } else {
                            vec![]
                        };
                        for name in &old_names {
                            tools.unregister(name);
                        }

                        // Register new WASM tools
                        let arc_mgr = std::sync::Arc::new(mgr);
                        for tool_def in arc_mgr.all_tool_defs() {
                            let wasm_tool = crate::extensions::WasmTool::new(
                                arc_mgr.clone(),
                                tool_def.name.clone(),
                                tool_def.description.clone(),
                                tool_def.schema.clone(),
                            );
                            tools.register(wasm_tool);
                        }
                        state.wasm_ext = Some(arc_mgr);
                        format!("{} loaded, {} error(s)", loaded_count, error_count)
                    }
                }
            } else {
                state.wasm_ext = None;
                "Disabled".to_string()
            };

            // Reload skills
            let skill_count = {
                let new_mgr = crate::skills::SkillManager::discover_all(
                    &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                    &[],
                )
                .unwrap_or_else(|_| crate::skills::SkillManager::new());
                let count = new_mgr.len();
                *state.skills.write() = new_mgr;
                count
            };

            let content = format!(
                "Reloaded Configuration\n\nModel: {}\nProvider: {}\nTheme: {}\nThinking: {:?}\nExtensions: {}\nSkills: {}\nStream: {}\nAuto-compact: {}",
                state.footer_state.data.model_name,
                state.footer_state.data.provider_name,
                theme_name, reloaded.thinking_level,
                ext_status, skill_count,
                reloaded.stream_responses, reloaded.auto_compaction,
            );
            state.overlay = None;
            state.overlay_state = Some(Box::new(
                super::overlay::text_viewer::TextViewerOverlay::new(" Reload ", content),
            ));
            true
        }
        "/skill" => {
            if let Some(sub) = arg {
                let parts: Vec<&str> = sub.splitn(2, ' ').collect();
                if parts[0].eq_ignore_ascii_case("off") {
                    // Deactivate a skill
                    let name = parts.get(1).unwrap_or(&"").trim();
                    if name.is_empty() {
                        state.add_notification("/skill off <name>".to_string(), NotificationKind::Info);
                    } else {
                        let mut active = state.active_skills.write();
                        let name_lower = name.to_lowercase();
                        if active.iter().any(|n| n.eq_ignore_ascii_case(&name_lower)) {
                            active.retain(|n| !n.eq_ignore_ascii_case(&name_lower));
                            drop(active);
                            state.add_notification(
                                format!("Skill deactivated: {}", name),
                                NotificationKind::Success,
                            );
                        } else {
                            drop(active);
                            state.add_notification(
                                format!("Skill '{}' is not active", name),
                                NotificationKind::Warning,
                            );
                        }
                    }
                } else {
                    // Activate a skill
                    let skills = state.skills.read();
                    if let Some(skill) = skills.get(sub.trim()) {
                        let name = skill.name.clone();
                        drop(skills);
                        let mut active = state.active_skills.write();
                        let name_lower = name.to_lowercase();
                        if active.iter().any(|n| n.eq_ignore_ascii_case(&name_lower)) {
                            drop(active);
                            state.add_notification(
                                format!("Skill '{}' is already active", name),
                                NotificationKind::Info,
                            );
                        } else {
                            active.push(name_lower);
                            drop(active);
                            state.add_notification(
                                format!("Skill activated: {}", name),
                                NotificationKind::Success,
                            );
                        }
                    } else {
                        drop(skills);
                        state.add_notification(
                            format!("Skill '{}' not found", sub.trim()),
                            NotificationKind::Warning,
                        );
                    }
                }
            } else {
                // List all skills with status
                let (is_empty, listing) = {
                    let skills = state.skills.read();
                    let active = state.active_skills.read();
                    if skills.is_empty() {
                        (true, String::new())
                    } else {
                        let mut out = String::from("Skills:\n\n");
                        for skill in skills.all() {
                            let is_active = active.iter().any(|n| n.eq_ignore_ascii_case(&skill.name));
                            let status = if is_active { "\u{2713}" } else { " " };
                            out.push_str(&format!(
                                "  [{}] {} — {}\n",
                                status, skill.name, skill.description
                            ));
                        }
                        out.push_str("\n/skill <name>  Activate a skill");
                        out.push_str("\n/skill off <name>  Deactivate a skill");
                        (false, out)
                    }
                };
                if is_empty {
                    state.add_notification(
                        "No skills found. Place skills in ~/.oxi/skills/<name>/SKILL.md"
                            .to_string(),
                        NotificationKind::Info,
                    );
                } else {
                    state.overlay = None;
                    state.overlay_state =
                        Some(super::overlay::tools_overlay(listing));
                }
            }
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
                    state.add_notification(
                        format!("Scoped: {}", names.join(", ")),
                        NotificationKind::Info,
                    );
                } else {
                    state.add_notification(
                        "Usage: /scoped-models provider/model1,provider/model2".to_string(),
                        NotificationKind::Info,
                    );
                }
            } else {
                let scoped = session.scoped_models();
                if scoped.is_empty() {
                    state.add_notification("No scoped models".to_string(), NotificationKind::Info);
                } else {
                    let names: Vec<String> = scoped
                        .iter()
                        .map(|m| format!("{}/{}", m.provider, m.model_id))
                        .collect();
                    state.add_notification(
                        format!("Scoped: {}", names.join(", ")),
                        NotificationKind::Info,
                    );
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
                    state.add_notification(
                        format!("[{}] {}", ext_name, output),
                        NotificationKind::Info,
                    );
                    return true;
                }
            }
            state.add_notification(
                format!("Unknown command: {}", cmd),
                NotificationKind::Warning,
            );
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

fn router_help() -> String {
    r#"Router Commands:

  /router          Configure router (opens setup) or show status
  /router status   Show routing status (tier, score, model, cost)
  /router pin      Pin current tier (coming soon)
  /router disable  Disable router, return to fixed model

  Or select \"router/auto\" in /model"#
        .to_string()
}

#[allow(dead_code)]
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
    /router           Configure model router

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

#[allow(dead_code)]
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
    state.add_notification(
        format!("API key for {} saved.", provider),
        NotificationKind::Success,
    );
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
    "github_search",
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
        state.add_notification(
            format!("Unknown tool: {}", tool_name),
            NotificationKind::Warning,
        );
        return;
    }

    if registry.get(&tool_name).is_some() {
        if let Some(tool) = registry.get(&tool_name) {
            if tool.essential() {
                state.add_notification(
                    format!("Cannot disable essential tool: {}", tool_name),
                    NotificationKind::Warning,
                );
                return;
            }
        }
        registry.unregister(&tool_name);
        if tool_name == "web_search" {
            registry.unregister("get_search_results");
        }
        state.add_notification(
            format!("Tool disabled: {}", tool_name),
            NotificationKind::Info,
        );
    } else {
        let re_registered = try_re_register_tool(&tool_name, registry);
        if re_registered {
            state.add_notification(
                format!("Tool enabled: {}", tool_name),
                NotificationKind::Success,
            );
        } else {
            state.add_notification(
                format!("Cannot re-enable {}", tool_name),
                NotificationKind::Warning,
            );
        }
    }
}

/// Collect all entries from a tree of SessionTreeNodes into a flat Vec<SessionEntry>.
/// Preserves tree order (depth-first traversal).
fn collect_tree_entries(
    roots: &[oxi_store::session::SessionTreeNode],
) -> Vec<oxi_store::session::SessionEntry> {
    let mut entries = Vec::new();
    fn visit(
        node: &oxi_store::session::SessionTreeNode,
        entries: &mut Vec<oxi_store::session::SessionEntry>,
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
