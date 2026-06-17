//! Model commands: `/model`, `/scoped-models`. Migrated off the legacy
//! `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::app::agent_session::{AgentSession, ScopedModel};
use crate::tui::app::{AppState, NotificationKind};
use crate::tui::completion::{CompletionItem, CompletionKind};
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// Collect all catalog models as `(provider, model_id)` pairs. Uses the
/// catalog port when available, else the legacy global model DB.
pub(super) fn collect_catalog_models(state: &AppState) -> Vec<(String, String)> {
    if let Some(ref cat) = state.catalog {
        cat.search_sync("")
            .into_iter()
            .map(|e| (e.provider, e.model_id))
            .collect()
    } else {
        oxi_sdk::get_all_models()
            .map(|e| (e.provider.to_string(), e.id.to_string()))
            .collect()
    }
}

/// Build the set of provider names that have a configured API key, including
/// siblings sharing the same `env_key`. Uses the catalog port when available.
pub(super) fn build_providers_with_key(
    state: &AppState,
    auth: &crate::store::auth_storage::AuthStorage,
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    if let Some(ref cat) = state.catalog {
        // Gather (name, env_key) for all providers that have a key set.
        let keyed: Vec<(String, Option<String>)> = cat
            .list_providers_sync()
            .into_iter()
            .filter_map(|pid| {
                let entry = cat.get_provider_sync(&pid)?;
                if auth.get_api_key(&pid).is_some() {
                    Some((pid, entry.env_key))
                } else {
                    None
                }
            })
            .collect();
        // For each keyed provider, find all providers sharing its env_key.
        for (name, env_key) in &keyed {
            set.insert(name.clone());
            if let Some(ek) = env_key {
                for other in cat.list_providers_sync() {
                    if let Some(oe) = cat.get_provider_sync(&other)
                        && oe.env_key.as_deref() == Some(ek.as_str())
                    {
                        set.insert(other);
                    }
                }
            }
        }
    } else {
        for p in oxi_sdk::get_builtin_providers() {
            if auth.get_api_key(p.name).is_some() {
                for p2 in oxi_sdk::get_builtin_providers() {
                    if p2.env_key == p.env_key {
                        set.insert(p2.name.to_string());
                    }
                }
            }
        }
    }
    set
}

/// `/model [id]` — switch or show model (opens selector UI when bare).
pub(crate) struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn description(&self) -> &str {
        "Select or switch model (opens selector UI)"
    }
    fn usage(&self) -> &str {
        "/model [provider/model]"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let model_id = args.trim();
        if !model_id.is_empty() {
            match session.set_model(model_id) {
                Ok(()) => {
                    state.add_notification(
                        format!("Model: {}", model_id),
                        NotificationKind::Success,
                    );
                    state.footer_state.data.model_name = model_id.to_string();
                    // Also set provider_name if model_id is in "provider/model" format
                    if let Some((provider, _model)) = model_id.split_once('/') {
                        state.footer_state.data.provider_name = provider.to_string();
                    }
                    crate::store::settings::Settings::save_last_used(model_id);
                }
                Err(e) => {
                    state.add_notification(format!("Error: {}", e), NotificationKind::Error);
                }
            }
        } else {
            let auth = crate::store::auth_storage::shared_auth_storage();

            // Build a set of provider names that have a configured key,
            // including all providers sharing the same env_key.
            // This handles the case where a user sets a key for "zai-coding-global"
            // but model DB entries have provider "zai" (both share ZAI_API_KEY).
            //
            // Uses the catalog port (sync read API) when available;
            // falls back to legacy global state otherwise.
            let providers_with_key: std::collections::HashSet<String> =
                build_providers_with_key(state, &auth);

            // Catalog models filtered by API key (via port when available).
            let mut all_models: Vec<String> = collect_catalog_models(state)
                .into_iter()
                .filter(|(provider, _)| providers_with_key.contains(provider))
                .map(|(provider, model_id)| format!("{}/{}", provider, model_id))
                .collect();

            // Dynamic models (custom providers + router/auto)
            for dyn_model in oxi_sdk::dynamic_models() {
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
                state.overlay_state = Some(overlay::model_select(all_models, session, state));
            }
        }
        SlashOutcome::Handled
    }

    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        state: &AppState,
    ) -> Vec<CompletionItem> {
        // `/model <provider/model>` → all accessible models.
        let auth = crate::store::auth_storage::shared_auth_storage();
        let providers_with_key: std::collections::HashSet<String> =
            build_providers_with_key(state, &auth);
        let mut all: Vec<String> = collect_catalog_models(state)
            .into_iter()
            .filter(|(provider, _)| providers_with_key.contains(provider))
            .map(|(provider, model_id)| format!("{}/{}", provider, model_id))
            .collect();
        for dyn_model in oxi_sdk::dynamic_models() {
            let entry = format!("{}/{}", dyn_model.provider, dyn_model.id);
            if !all.contains(&entry) {
                all.push(entry);
            }
        }
        all.into_iter()
            .filter(|m| m.starts_with(prefix))
            .map(|m| CompletionItem {
                text: m.clone(),
                label: m,
                description: None,
                kind: CompletionKind::SlashArgument {
                    command: "model".to_string(),
                },
            })
            .collect()
    }
}

/// `/scoped-models [list]` / `/models` — set/get models for Ctrl+P cycling.
pub(crate) struct ScopedModelsCommand;

impl SlashCommand for ScopedModelsCommand {
    fn name(&self) -> &str {
        "scoped-models"
    }
    fn aliases(&self) -> &[&str] {
        &["models"]
    }
    fn description(&self) -> &str {
        "Set/get models for Ctrl+P cycling (alias: /models)"
    }
    fn usage(&self) -> &str {
        "/scoped-models provider/model1,provider/model2"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let models_str = args.trim();
        if !models_str.is_empty() {
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
        SlashOutcome::Handled
    }
}
