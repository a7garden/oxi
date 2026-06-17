//! `/reload` — reload settings, theme, and extensions. Migrated off the legacy
//! `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::tui::app::NotificationKind;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// `/reload` — reload settings, theme, and extensions.
pub(crate) struct ReloadCommand;

impl SlashCommand for ReloadCommand {
    fn name(&self) -> &str {
        "reload"
    }
    fn description(&self) -> &str {
        "Reload settings, theme, and extensions"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let reloaded = crate::store::settings::Settings::load().unwrap_or_default();
        let _theme_name = reloaded.theme.clone();
        session.set_thinking_level(reloaded.thinking_level);
        // Rebuild the system prompt to pick up the latest TUI language
        // policy (`output_languages`). The App-side `set_thinking_level`
        // above handles the thinking-level half; the language policy is
        // a separate concern (see `Settings::output_languages` docs and
        // `AgentSession::rebuild_system_prompt`).
        session.rebuild_system_prompt();
        // Apply model change to the active agent session
        if let Some(m) = reloaded.effective_model(None)
            && !m.is_empty()
        {
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

        // Reload WASM extensions
        if reloaded.extensions_enabled {
            let cwd_path =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let wasm_paths = crate::extensions::WasmExtensionManager::discover(&cwd_path);
            if !wasm_paths.is_empty() {
                let mut mgr = crate::extensions::WasmExtensionManager::new();
                let (loaded, errors) = mgr.load_all(&wasm_paths);
                let loaded_count = loaded.len();
                let error_count = errors.len();
                if !mgr.is_empty() {
                    // Unregister old WASM tools
                    let tools = session.agent_ref().tools();
                    let old_names: Vec<String> = if let Some(ref old_ext) = state.wasm_ext {
                        old_ext
                            .all_tool_defs()
                            .iter()
                            .map(|d| d.name.clone())
                            .collect()
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
                    state.add_notification(
                        format!("{} loaded, {} error(s)", loaded_count, error_count),
                        NotificationKind::Info,
                    );
                } else {
                    state.wasm_ext = None;
                }
            } else {
                state.wasm_ext = None;
            }
        } else {
            state.wasm_ext = None;
        }

        // Reload skills
        {
            let new_mgr = crate::skills::SkillManager::discover_all(
                &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                &[],
            )
            .unwrap_or_else(|_| crate::skills::SkillManager::new());
            let count = new_mgr.len();
            *state.skills.write() = new_mgr;
            if count > 0 {
                state
                    .add_notification(format!("{} skill(s) loaded", count), NotificationKind::Info);
            }
        }

        state.add_notification("Settings reloaded".to_string(), NotificationKind::Success);
        SlashOutcome::Handled
    }
}
