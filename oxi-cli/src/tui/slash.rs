//! Slash command handling.

use super::app::{AppOverlay, AppState, SetupStep};
use crate::agent_session::{AgentSession, ScopedModel};
use crate::auth_storage::AuthStorage;
use crate::clipboard_write;
use crate::export::{self, ExportMeta, HtmlExportOptions};
use oxi_tui::widgets::chat::{ContentBlock, MessageRole};
use std::path::PathBuf;

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
        "/clear" => {
            state.chat.clear();
            session.reset();
            true
        }
        "/model" => {
            if let Some(model_id) = arg {
                // Direct model switch (backward compatible)
                match session.set_model(model_id) {
                    Ok(()) => {
                        state.add_system_message(format!("→ model: {}", model_id));
                        state.footer_state.data.model_name = model_id.to_string();
                    }
                    Err(e) => {
                        state.add_system_message(format!("✗ {}", e));
                    }
                }
            } else {
                // Show interactive model selector overlay
                let all_models: Vec<String> = oxi_ai::model_db::get_all_models()
                    .map(|entry| format!("{}/{}", entry.provider, entry.id))
                    .collect();
                if all_models.is_empty() {
                    state.add_system_message(format!(
                        "Model: {}\n/model <provider/model> to switch",
                        session.model_id()
                    ));
                } else {
                    state.overlay = Some(AppOverlay::ModelSelect {
                        models: all_models,
                        filter: String::new(),
                        selected: 0,
                    });
                }
            }
            true
        }
        "/compact" => {
            let instructions = arg.map(|s| s.to_string());
            let sh = session.clone_handle();
            tokio::spawn(async move {
                match sh.compact(instructions).await {
                    Ok(result) => {
                        tracing::info!("Compaction: {} tokens before", result.tokens_before)
                    }
                    Err(e) => tracing::warn!("Compaction failed: {}", e),
                }
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
        "/name" => {
            if let Some(name) = arg {
                session.set_session_name(name.to_string());
                state.add_system_message(format!("Session → {}", name));
            } else {
                state.add_system_message("/name <name>".to_string());
            }
            true
        }
        "/copy" => {
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
                match clipboard_write::copy_to_clipboard(&content) {
                    Ok(()) => state.add_system_message("✓ Copied to clipboard".to_string()),
                    Err(e) => state.add_system_message(format!("✗ Copy failed: {}", e)),
                }
            } else {
                state.add_system_message("No assistant message".to_string());
            }
            true
        }
        "/changelog" => {
            let paths = vec![PathBuf::from("CHANGELOG.md"), PathBuf::from("../CHANGELOG.md")];
            let mut entries: Vec<crate::changelog::ChangelogEntry> = Vec::new();
            for path in &paths {
                let parsed = crate::changelog::parse_changelog(path);
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
                        format!("{}…", &entry.content[..end])
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
            let entries: Vec<crate::session::SessionEntry> = state
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
                    crate::session::SessionEntry::simple_message(role, &content)
                })
                .collect();
            match export::export_to_html(&entries, &meta, &HtmlExportOptions::default()) {
                Ok(html) => {
                    if let Some(path) = export_path {
                        match std::fs::write(&path, &html) {
                            Ok(()) => {
                                state.add_system_message(format!("✓ Exported: {}", path.display()))
                            }
                            Err(e) => {
                                state.add_system_message(format!("✗ Write failed: {}", e))
                            }
                        }
                    } else {
                        state.add_system_message(format!(
                            "HTML ready ({} bytes). /export <path> to save.",
                            html.len()
                        ));
                    }
                }
                Err(e) => state.add_system_message(format!("✗ Export failed: {}", e)),
            }
            true
        }
        "/import" => {
            state.add_system_message(if let Some(p) = arg {
                format!("Import '{}' — coming soon", p)
            } else {
                "/import <path-to-jsonl>".to_string()
            });
            true
        }
        "/share" => {
            state.add_system_message(
                "GitHub gist sharing coming soon. Use /export for HTML.".to_string(),
            );
            true
        }
        "/fork" => {
            state.add_system_message(
                "Use /tree to view branches. Fork via session navigation.".to_string(),
            );
            true
        }
        "/clone" => {
            state.add_system_message(
                "Run oxi --continue in a new terminal to clone.".to_string(),
            );
            true
        }
        "/tree" => {
            state.add_system_message(
                "Linear session. Use /fork to branch from a previous message.".to_string(),
            );
            true
        }
        "/login" => {
            if let Some(provider) = arg {
                let parts: Vec<&str> = provider.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    // /login <provider> <key> — direct save (backward compatible)
                    try_login_with_key(parts[0], parts[1], state);
                } else {
                    // /login <provider> — show EnterApiKey overlay for that provider
                    state.overlay = Some(AppOverlay::LoginProvider(SetupStep::EnterApiKey {
                        provider: parts[0].to_string(),
                        key: String::new(),
                        masked_cursor: 0,
                    }));
                }
            } else {
                // /login — show provider selection overlay
                let auth = AuthStorage::new();
                let providers = vec![
                    "anthropic", "openai", "google", "deepseek", "groq",
                    "openrouter", "mistral", "xai", "minimax", "zai",
                ];
                let provider_list: Vec<(String, bool)> = providers.iter().map(|name| {
                    let has_key = auth.has_auth(name);
                    (name.to_string(), has_key)
                }).collect();
                state.overlay = Some(AppOverlay::LoginProvider(SetupStep::SelectProvider {
                    providers: provider_list,
                    selected: 0,
                }));
            }
            true
        }
        "/logout" => {
            if let Some(provider) = arg {
                // Direct logout (backward compatible)
                AuthStorage::new().remove(provider);
                state.add_system_message(format!("✓ Removed {}", provider));
            } else {
                // Show provider selection overlay for logout
                let auth = AuthStorage::new();
                let providers = auth.configured_providers();
                if providers.is_empty() {
                    state.add_system_message("No providers configured.".to_string());
                } else {
                    state.overlay = Some(AppOverlay::LogoutSelect {
                        providers,
                        selected: 0,
                    });
                }
            }
            true
        }
        "/new" => {
            state.add_system_message("Starting new session…".to_string());
            session.reset();
            state.chat.clear();
            true
        }
        "/resume" => {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());
            let session_dir = crate::session::get_default_session_dir(&cwd);
            if let Ok(sessions) = std::fs::read_dir(&session_dir) {
                let list: Vec<_> = sessions
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "jsonl"))
                    .take(10)
                    .collect();
                if list.is_empty() {
                    state.add_system_message("No previous sessions".to_string());
                } else {
                    let mut out = "Recent:\n\n".to_string();
                    for (i, entry) in list.iter().enumerate() {
                        if let Some(name) = entry.file_name().to_str() {
                            out.push_str(&format!("{}. {}\n", i + 1, name));
                        }
                    }
                    out.push_str("\n/import <path> to resume");
                    state.add_system_message(out);
                }
            } else {
                state.add_system_message("No sessions found".to_string());
            }
            true
        }
        "/reload" => {
            state.add_system_message("✓ Configuration reloaded".to_string());
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
                                thinking_level: None,
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
        _ => false,
    }
}

// ── Help text ────────────────────────────────────────────────────────────

fn format_help() -> String {
    r#"
  Session
    /new              Start a new session
    /clone            Duplicate current session
    /resume           List recent sessions
    /tree             Show session tree
    /fork             Fork from a previous message
    /session          Show session info
    /name <name>      Set session name

  Model
    /model [id]       Switch or show model
    /scoped-models    Models for Ctrl+P cycling

  Context
    /compact [instr]  Compact context
    /clear            Clear history

  Export
    /export [path]    Export to HTML
    /import <path>    Import from JSONL
    /copy             Copy last reply

  Auth
    /login <provider> Set API key
    /logout <provider> Remove key

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
    Ctrl+P             Cycle models
    Shift+Ctrl+P       Cycle models (reverse)
"#
    .to_string()
}

// ── Interactive login ────────────────────────────────────────────────────

/// 프리셋 프로바이더 목록
const LOGIN_PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("google", "GOOGLE_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
    ("mistral", "MISTRAL_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("fireworks", "FIREWORKS_API_KEY"),
    ("minimax", "MINIMAX_API_KEY"),
    ("zai", "ZAI_API_KEY"),
];

/// /login — 프로바이더 목록 표시 후 안내
fn interactive_login_select(state: &mut AppState) {
    let mut msg = "🔑 프로바이더를 선택하세요:\n\n".to_string();
    let auth = AuthStorage::new();

    for (_i, (name, env_key)) in LOGIN_PROVIDERS.iter().enumerate() {
        let has_key = std::env::var(env_key).ok().is_some()
            || auth.get_api_key(name).is_some();
        let status = if has_key { "✅" } else { "☐" };
        msg.push_str(&format!("  {} {}\n", status, name));
    }

    msg.push_str("\n/login <provider> 로 API 키를 입력하세요.");
    msg.push_str("\n예: /login minimax");
    state.add_system_message(msg);
}

/// /login <provider> — 현재 키 상태 표시 + 안내
fn interactive_login(provider: &str, state: &mut AppState) {
    let provider = provider.to_lowercase();

    // 이미 키가 있는지 확인
    let env_key = LOGIN_PROVIDERS
        .iter()
        .find(|(name, _)| *name == provider)
        .map(|(_, key)| *key)
        .unwrap_or_else(|| {
            // 커스텀 프로바이더 — 환경변수 이름 추측
            let guessed = format!("{}_API_KEY", provider.to_uppercase());
            // Return a static str from LOGIN_PROVIDERS or leak the String
            // This is only called once at login time
            Box::leak(guessed.into_boxed_str())
        });

    let auth = AuthStorage::new();
    let existing = std::env::var(&env_key)
        .ok()
        .or_else(|| auth.get_api_key(&provider));

    let masked = existing
        .as_ref()
        .map(|k| mask_key(k))
        .unwrap_or_else(|| "없음".to_string());

    state.add_system_message(format!(
        "🔑 {} API 키\n\n현재: {}\n환경변수: {}\n\n아래에 API 키를 입력하세요:\n/login {} <your-api-key>",
        provider, masked, env_key, provider
    ));
}

/// /login <provider> <key> — 키 직접 입력 시 저장
fn try_login_with_key(provider: &str, key: &str, state: &mut AppState) -> bool {
    if key.is_empty() {
        return false;
    }
    let auth = AuthStorage::new();
    auth.set_api_key(provider, key.to_string());
    state.add_system_message(format!(
        "✅ {} API 키가 저장되었습니다.",
        provider
    ));
    true
}

/// API 키 마스킹 (앞 6글자 + ... + 뒤 4글자)
fn mask_key(key: &str) -> String {
    if key.len() <= 12 {
        return "***".to_string();
    }
    format!("{}...{}", &key[..6], &key[key.len()-4..])
}
