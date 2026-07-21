//! Auth commands: `/provider`, `/logout`. Migrated off the legacy
//! `handle_slash_command` match.
//!
//! (Currently: `/logout` and `/provider` ported.)

use super::super::registry::SlashCommand;
use crate::app::agent_session::AgentSession;
use crate::tui::app::{AppState, NotificationKind};
use crate::tui::completion::{CompletionItem, CompletionKind};
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// `/provider <name> <key>` — save the API key (used by `/provider`).
fn try_provider_with_key(provider: &str, key: &str, state: &mut AppState) -> bool {
    if key.is_empty() {
        return false;
    }
    let auth = crate::store::auth_storage::shared_auth_storage();
    auth.set_api_key(provider, key.to_string());
    state.add_notification(
        format!("API key for {} saved.", provider),
        NotificationKind::Success,
    );
    true
}

/// `/logout [provider]` — remove provider authentication.
pub(crate) struct LogoutCommand;

impl SlashCommand for LogoutCommand {
    fn name(&self) -> &str {
        "logout"
    }
    fn description(&self) -> &str {
        "Remove provider authentication"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let provider = args.trim();
        if !provider.is_empty() {
            crate::store::auth_storage::shared_auth_storage().remove(provider);
            state.add_notification(format!("Removed {}", provider), NotificationKind::Success);
        } else {
            let auth = crate::store::auth_storage::shared_auth_storage();
            let providers = auth.configured_providers();
            if providers.is_empty() {
                state.add_notification(
                    "No providers configured.".to_string(),
                    NotificationKind::Info,
                );
            } else {
                state.overlay = None;
                state.overlay_state = Some(overlay::logout_select(providers, state));
            }
        }
        SlashOutcome::Handled
    }

    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        _state: &AppState,
    ) -> Vec<CompletionItem> {
        // `/logout <provider>` → configured provider names.
        let last = prefix.rsplit(' ').next().unwrap_or("");
        let auth = crate::store::auth_storage::shared_auth_storage();
        auth.configured_providers()
            .into_iter()
            .filter(|p| p.starts_with(last))
            .map(|p| CompletionItem {
                text: p.clone(),
                label: p,
                description: None,
                kind: CompletionKind::SlashArgument {
                    command: "logout".to_string(),
                },
            })
            .collect()
    }
}

/// `/provider [name] [key]` — configure API key for a provider.
pub(crate) struct ProviderCommand;

impl SlashCommand for ProviderCommand {
    fn name(&self) -> &str {
        "provider"
    }
    fn description(&self) -> &str {
        "Configure API key for a provider"
    }
    fn usage(&self) -> &str {
        "/provider [name [key]]"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let arg = args.trim();
        if !arg.is_empty() {
            let parts: Vec<&str> = arg.splitn(2, ' ').collect();
            if parts.len() == 2 {
                try_provider_with_key(parts[0], parts[1], state);
            } else {
                // Open provider-select overlay with the named provider pre-selected
                let provider_name = parts[0].to_string();
                let entries = overlay::provider_select::build_provider_entries_with_catalog(
                    state.catalog.as_ref(),
                );
                let mut provider_overlay = overlay::provider_select::ProviderSelectOverlay::new(
                    entries, false, // not initial setup
                );
                // Try to pre-select the named provider
                if let Some(idx) = provider_overlay
                    .providers
                    .iter()
                    .position(|p| p.name == provider_name)
                {
                    provider_overlay.selected = idx;
                }
                state.overlay_state = Some(Box::new(provider_overlay));
            }
        } else {
            // Open full provider-select overlay
            let entries = overlay::provider_select::build_provider_entries_with_catalog(
                state.catalog.as_ref(),
            );
            state.overlay_state = Some(Box::new(
                overlay::provider_select::ProviderSelectOverlay::new(entries, false),
            ));
        }
        SlashOutcome::Handled
    }

    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        state: &AppState,
    ) -> Vec<CompletionItem> {
        // `/provider <name>` → selectable provider names.
        let last = prefix.rsplit(' ').next().unwrap_or("");
        overlay::provider_select::build_provider_entries_with_catalog(state.catalog.as_ref())
            .into_iter()
            .filter(|p| p.name.starts_with(last))
            .map(|p| {
                let desc = Some(p.description);
                CompletionItem {
                    text: p.name.clone(),
                    label: p.name,
                    description: desc,
                    kind: CompletionKind::SlashArgument {
                        command: "provider".to_string(),
                    },
                }
            })
            .collect()
    }
}
