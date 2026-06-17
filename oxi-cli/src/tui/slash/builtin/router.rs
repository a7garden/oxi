//! `/router` — configure or inspect model router. Migrated off the legacy
//! `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::app::agent_session::AgentSession;
use crate::tui::app::{AppState, NotificationKind};
use crate::tui::completion::{CompletionItem, CompletionKind};
use crate::tui::overlay::{self, router_integration};
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// `/router` help text (shown for unknown `/router <sub>`).
fn router_help() -> String {
    r#"Router Commands:

  /router          Configure router (opens setup) or show status
  /router status   Show routing status (tier, score, model, cost)
  /router pin      Pin current tier (low|medium|high|off)
  /router enable   Enable router (switch to router/auto)
  /router disable  Disable router, return to fixed model

  Or select \"router/auto\" in /model"#
        .to_string()
}

/// `/router [status|pin|enable|disable]` — configure or inspect model router.
pub(crate) struct RouterCommand;

impl SlashCommand for RouterCommand {
    fn name(&self) -> &str {
        "router"
    }
    fn description(&self) -> &str {
        "Configure or inspect model router"
    }
    fn usage(&self) -> &str {
        "/router <status|pin <low|medium|high|off>|enable|disable>"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let sub = args.trim();
        if !sub.is_empty() {
            let mut parts = sub.split_whitespace();
            let cmd = parts.next().unwrap_or("");
            match cmd {
                "status" => {
                    if let Some(snap) = oxi_sdk::router::RouterProvider::get_snapshot() {
                        let content = format!(
                            "Router Status:\n\nProfile: {}\nTier: {:?}\nScore: {:.2}\nModel: {}\nProvider: {}\nCost: ${:.4}\nTurns: {}",
                            snap.profile.as_deref().unwrap_or("-"),
                            snap.last_tier
                                .unwrap_or(oxi_sdk::router::RouterTier::Medium),
                            snap.last_score,
                            snap.last_model.as_deref().unwrap_or("-"),
                            snap.last_provider.as_deref().unwrap_or("-"),
                            snap.accumulated_cost,
                            snap.turn_count,
                        );
                        state.overlay = None;
                        state.overlay_state =
                            Some(Box::new(overlay::text_viewer::TextViewerOverlay::new(
                                " Router Status ",
                                content,
                            )));
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
                                oxi_sdk::router::set_router_pin(Some(
                                    oxi_sdk::router::RouterTier::Low,
                                ));
                                state.add_notification(
                                    "Router pinned to LOW tier".to_string(),
                                    NotificationKind::Success,
                                );
                            }
                            "medium" => {
                                oxi_sdk::router::set_router_pin(Some(
                                    oxi_sdk::router::RouterTier::Medium,
                                ));
                                state.add_notification(
                                    "Router pinned to MEDIUM tier".to_string(),
                                    NotificationKind::Success,
                                );
                            }
                            "high" => {
                                oxi_sdk::router::set_router_pin(Some(
                                    oxi_sdk::router::RouterTier::High,
                                ));
                                state.add_notification(
                                    "Router pinned to HIGH tier".to_string(),
                                    NotificationKind::Success,
                                );
                            }
                            "off" | "none" | "clear" => {
                                oxi_sdk::router::set_router_pin(None);
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
                        let current = oxi_sdk::router::get_router_pin();
                        let msg = match current {
                            Some(t) => format!("Router pin: {:?}", t),
                            None => "Router pin: none (auto)".to_string(),
                        };
                        state.add_notification(msg, NotificationKind::Info);
                    }
                }
                "disable" => {
                    // Switch away from router to the default model
                    let settings = crate::store::settings::Settings::load().unwrap_or_default();
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
                    state.overlay_state =
                        Some(Box::new(overlay::text_viewer::TextViewerOverlay::new(
                            " Router Help ",
                            router_help(),
                        )));
                }
            }
        } else {
            let global_dir = dirs::config_dir().unwrap_or_default().join("oxi");
            let project_dir = std::env::current_dir().unwrap_or_default();
            let has_config =
                crate::store::router_config::load_router_config(&global_dir, &project_dir)
                    .is_some();

            if has_config {
                if let Some(snap) = oxi_sdk::router::RouterProvider::get_snapshot() {
                    let content = format!(
                        "Router Status:\n\nProfile: {}\nTier: {:?}\nScore: {:.2}\nModel: {}\nCost: ${:.4}\nTurns: {}",
                        snap.profile.as_deref().unwrap_or("-"),
                        snap.last_tier
                            .unwrap_or(oxi_sdk::router::RouterTier::Medium),
                        snap.last_score,
                        snap.last_model.as_deref().unwrap_or("-"),
                        snap.accumulated_cost,
                        snap.turn_count,
                    );
                    state.overlay = None;
                    state.overlay_state = Some(Box::new(
                        overlay::text_viewer::TextViewerOverlay::new(" Router Status ", content),
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
                let auth = crate::store::auth_storage::shared_auth_storage();
                let setup_models: Vec<String> = super::model::collect_catalog_models(state)
                    .into_iter()
                    .filter(|(provider, _)| auth.get_api_key(provider).is_some())
                    .map(|(provider, model_id)| format!("{}/{}", provider, model_id))
                    .collect();
                let initial = overlay::RouterSetupData {
                    profile_name: "auto".to_string(),
                    ..Default::default()
                };
                state.overlay = None;
                state.overlay_state = Some(overlay::router_setup(
                    initial,
                    setup_models,
                    move |data: &overlay::RouterSetupData| {
                        let store_cfg = router_integration::save_router_config(data)?;
                        let ai_cfg = router_integration::store_config_to_ai_config(&store_cfg);
                        oxi_sdk::router::register_router(&ai_cfg);
                        Ok(())
                    },
                    || {},
                ));
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
        // First token: status | pin | enable | disable.
        // After `pin`: low | medium | high | off.
        let mut tokens = vec!["status", "pin", "enable", "disable"];
        let first = prefix.split_whitespace().next().unwrap_or("");
        if prefix.starts_with("pin") && prefix.trim() != "pin" {
            tokens = vec!["low", "medium", "high", "off"];
            let last = prefix.rsplit(' ').next().unwrap_or("");
            return tokens
                .iter()
                .filter(|t| t.starts_with(last))
                .map(|t| CompletionItem {
                    text: t.to_string(),
                    label: t.to_string(),
                    description: None,
                    kind: CompletionKind::SlashArgument {
                        command: "router".to_string(),
                    },
                })
                .collect();
        }
        let _ = first;
        tokens
            .iter()
            .filter(|t| t.starts_with(prefix))
            .map(|t| CompletionItem {
                text: t.to_string(),
                label: t.to_string(),
                description: None,
                kind: CompletionKind::SlashArgument {
                    command: "router".to_string(),
                },
            })
            .collect()
    }
}
