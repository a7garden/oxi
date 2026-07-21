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
/// Extract the lowercase host from a base URL
/// (`https://api.z.ai/api/paas/v4` → `api.z.ai`).
fn upstream_host(base_url: &str) -> Option<String> {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
}

/// Given each catalog provider's `(name, base_url)`, the set of provider names
/// the user explicitly registered (stored credential or custom provider), and
/// the base URLs of configured custom providers, return the catalog providers
/// to hide. Pure core of the issue-#24 dedup — no `AppState`, so it is unit
/// testable.
fn shadowed_provider_names(
    catalog: &[(String, Option<String>)],
    explicit_names: &std::collections::HashSet<String>,
    custom_base_urls: &[String],
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    // Upstream hosts covered by an explicitly registered provider.
    let mut explicit_hosts: HashSet<String> = HashSet::new();
    for (name, base_url) in catalog {
        if explicit_names.contains(name)
            && let Some(host) = base_url.as_deref().and_then(upstream_host)
        {
            explicit_hosts.insert(host);
        }
    }
    for url in custom_base_urls {
        if let Some(host) = upstream_host(url) {
            explicit_hosts.insert(host);
        }
    }
    // Shadow non-explicit catalog providers sitting on a covered host.
    catalog
        .iter()
        .filter(|(name, _)| !explicit_names.contains(name))
        .filter_map(|(name, base_url)| {
            base_url
                .as_deref()
                .and_then(upstream_host)
                .filter(|h| explicit_hosts.contains(h))
                .map(|_| name.clone())
        })
        .collect()
}

/// Catalog providers hidden from the model selector because a provider the user
/// explicitly registered already covers the same upstream host (issue #24:
/// `zai/*` duplicating `zai-coding-plan/*` — both hit `api.z.ai`).
///
/// A catalog provider is shadowed only when **both** hold:
///   1. some provider the user explicitly registered (a stored credential under
///      its exact name, or an entry in `settings.custom_providers`) sits on the
///      same upstream host, **and**
///   2. this provider itself is *not* explicitly registered — it only appears
///      because it shares an env-key with the registered one.
///
/// Hosts with no explicitly-registered provider are left untouched, so env-keyed
/// builtins keep working for users who never ran the setup wizard.
fn shadowed_catalog_providers(
    state: &AppState,
    auth: &crate::store::auth_storage::AuthStorage,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let Some(ref cat) = state.catalog else {
        return HashSet::new();
    };

    let catalog: Vec<(String, Option<String>)> = cat
        .list_providers_sync()
        .into_iter()
        .map(|pid| {
            let base_url = cat.get_provider_sync(&pid).and_then(|e| e.base_url);
            (pid, base_url)
        })
        .collect();

    let mut explicit_names: HashSet<String> = auth.configured_providers().into_iter().collect();
    let custom_cp: Vec<(String, String)> = crate::store::settings::Settings::load()
        .map(|s| {
            s.custom_providers
                .iter()
                .map(|cp| (cp.name.clone(), cp.base_url.clone()))
                .collect()
        })
        .unwrap_or_default();
    let custom_base_urls: Vec<String> = custom_cp.iter().map(|(_, u)| u.clone()).collect();
    for (name, _) in &custom_cp {
        explicit_names.insert(name.clone());
    }

    shadowed_provider_names(&catalog, &explicit_names, &custom_base_urls)
}

/// Assemble the full list of selectable `provider/model` entries shared by the
/// `/model` selector and its tab-completion.
///
/// Combines catalog models (filtered to providers with a usable key) with
/// dynamically-discovered models (custom providers + router/auto), then applies
/// upstream deduplication so providers sharing one API host don't produce
/// redundant entries (issue #24).
pub(super) fn accessible_models(
    state: &AppState,
    auth: &crate::store::auth_storage::AuthStorage,
) -> Vec<String> {
    let providers_with_key = build_providers_with_key(state, auth);
    let shadowed = shadowed_catalog_providers(state, auth);

    let mut models: Vec<String> = collect_catalog_models(state)
        .into_iter()
        .filter(|(provider, _)| {
            providers_with_key.contains(provider) && !shadowed.contains(provider)
        })
        .map(|(provider, model_id)| format!("{}/{}", provider, model_id))
        .collect();

    // Dynamic models (custom providers + router/auto) are always explicitly
    // registered, so they are never shadowed; exact-string dedup keeps the
    // list free of catalog/custom collisions.
    for dyn_model in oxi_sdk::dynamic_models() {
        let entry = format!("{}/{}", dyn_model.provider, dyn_model.id);
        if !models.contains(&entry) {
            models.push(entry);
        }
    }
    models
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

            // Catalog models (keyed providers) + dynamic models, with upstream
            // deduplication so providers sharing one API host don't show
            // redundant entries (issue #24).
            let all_models = accessible_models(state, &auth);
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
        let all: Vec<String> = accessible_models(state, &auth);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn upstream_host_extracts_lowercase_host() {
        assert_eq!(
            upstream_host("https://api.z.ai/api/coding/paas/v4").as_deref(),
            Some("api.z.ai")
        );
        assert_eq!(
            upstream_host("https://open.bigmodel.cn/api/paas/v4").as_deref(),
            Some("open.bigmodel.cn")
        );
        // Invalid URLs yield None — never panic.
        assert_eq!(upstream_host("not a url"), None);
        // Bare scheme without a host.
        assert_eq!(upstream_host("file:///etc/hosts"), None);
    }

    fn cat(name: &str, url: Option<&str>) -> (String, Option<String>) {
        (name.to_string(), url.map(|s| s.to_string()))
    }

    /// Reproduces issue #24: registering `zai-coding-plan` must hide the
    /// generic `zai` catalog provider because both point at `api.z.ai`, while
    /// the unrelated `openai` provider (different host) is unaffected.
    #[test]
    fn zai_coding_plan_shadows_generic_zai_same_host() {
        let catalog = vec![
            cat("zai", Some("https://api.z.ai/api/paas/v4")),
            cat(
                "zai-coding-plan",
                Some("https://api.z.ai/api/coding/paas/v4"),
            ),
            cat("openai", Some("https://api.openai.com/v1")),
        ];
        // User explicitly registered only `zai-coding-plan`.
        let explicit: HashSet<String> = ["zai-coding-plan".to_string()].into_iter().collect();

        let shadowed = shadowed_provider_names(&catalog, &explicit, &[]);

        assert!(shadowed.contains("zai"), "generic zai must be shadowed");
        assert!(
            !shadowed.contains("zai-coding-plan"),
            "the registered provider is never shadowed"
        );
        assert!(
            !shadowed.contains("openai"),
            "providers on other hosts are untouched"
        );
    }

    /// Two real upstreams share one host (`zai` + `zai-coding-plan` both on
    /// `api.z.ai`) and two share another (`zhipuai` + `zhipuai-coding-plan` on
    /// `open.bigmodel.cn`). Registering one on each host shadows only the
    /// non-explicit sibling on that host.
    #[test]
    fn shadows_only_non_explicit_sibling_per_host() {
        let catalog = vec![
            cat("zai", Some("https://api.z.ai/api/paas/v4")),
            cat(
                "zai-coding-plan",
                Some("https://api.z.ai/api/coding/paas/v4"),
            ),
            cat("zhipuai", Some("https://open.bigmodel.cn/api/paas/v4")),
            cat(
                "zhipuai-coding-plan",
                Some("https://open.bigmodel.cn/api/coding/paas/v4"),
            ),
        ];
        let explicit: HashSet<String> = [
            "zai-coding-plan".to_string(),
            "zhipuai-coding-plan".to_string(),
        ]
        .into_iter()
        .collect();

        let shadowed = shadowed_provider_names(&catalog, &explicit, &[]);

        assert_eq!(
            shadowed,
            ["zai".to_string(), "zhipuai".to_string()]
                .into_iter()
                .collect::<HashSet<_>>()
        );
    }

    /// With no explicit registration at all, nothing is shadowed — env-keyed
    /// builtins keep working for users who never ran the setup wizard.
    #[test]
    fn no_explicit_registration_shadows_nothing() {
        let catalog = vec![
            cat("zai", Some("https://api.z.ai/api/paas/v4")),
            cat(
                "zai-coding-plan",
                Some("https://api.z.ai/api/coding/paas/v4"),
            ),
        ];
        let explicit: HashSet<String> = HashSet::new();

        let shadowed = shadowed_provider_names(&catalog, &explicit, &[]);
        assert!(shadowed.is_empty());
    }

    /// A configured custom provider (settings.custom_providers) covers its host,
    /// shadowing a catalog builtin that points at the same upstream even though
    /// the custom name has no stored credential.
    #[test]
    fn custom_provider_shadows_builtin_on_same_host() {
        let catalog = vec![
            cat("zai", Some("https://api.z.ai/api/paas/v4")),
            cat("anthropic", Some("https://api.anthropic.com")),
        ];
        // Custom provider "my-zai" (no catalog entry) points at api.z.ai.
        let explicit: HashSet<String> = HashSet::new();
        let custom_base_urls = vec!["https://api.z.ai/api/paas/v4".to_string()];

        let shadowed = shadowed_provider_names(&catalog, &explicit, &custom_base_urls);

        assert!(
            shadowed.contains("zai"),
            "builtin zai shadowed by custom host"
        );
        assert!(!shadowed.contains("anthropic"), "different host untouched");
    }

    /// If the user explicitly registered the *generic* provider, it is kept and
    /// the sibling is shadowed instead.
    #[test]
    fn registered_generic_provider_keeps_it_shadows_sibling() {
        let catalog = vec![
            cat("zai", Some("https://api.z.ai/api/paas/v4")),
            cat(
                "zai-coding-plan",
                Some("https://api.z.ai/api/coding/paas/v4"),
            ),
        ];
        let explicit: HashSet<String> = ["zai".to_string()].into_iter().collect();

        let shadowed = shadowed_provider_names(&catalog, &explicit, &[]);
        assert!(shadowed.contains("zai-coding-plan"));
        assert!(!shadowed.contains("zai"));
    }
}
