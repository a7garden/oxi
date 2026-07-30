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
        // Flag the render loop to rebuild the live theme (theme name + glyph
        // set) from the freshly-loaded settings. The overlay's Esc handler
        // sets the same flag; `/reload` mirrors it so hand-edited
        // `settings.toml` theme changes are picked up without a restart.
        state.appearance_needs_reload = true;
        session.set_thinking_level(reloaded.thinking_level);
        // Rebuild the system prompt from freshly loaded settings so the next
        // turn observes hot-applied prompt inputs.
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

        // Reload WASM extensions (shared with the /settings overlay).
        crate::tui::overlay::settings::sync_wasm_extensions(
            state,
            session,
            reloaded.extensions_enabled,
        );

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
