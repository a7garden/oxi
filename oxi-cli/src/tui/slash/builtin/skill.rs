//! `/skill` — list, activate, or deactivate skills. Migrated off the legacy
//! `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::app::agent_session::AgentSession;
use crate::tui::app::AppState;
use crate::tui::app::NotificationKind;
use crate::tui::completion::{CompletionItem, CompletionKind};
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// `/skill [off] [name]` — list, activate, or deactivate skills.
pub(crate) struct SkillCommand;

impl SlashCommand for SkillCommand {
    fn name(&self) -> &str {
        "skill"
    }
    fn description(&self) -> &str {
        "List, activate, or deactivate skills"
    }
    fn usage(&self) -> &str {
        "/skill [off <name>|<name>]"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let sub = args.trim();
        if !sub.is_empty() {
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
                if let Some(skill) = skills.get(sub) {
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
                        format!("Skill '{}' not found", sub),
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
                    "No skills found. Place skills in ~/.oxi/skills/<name>/SKILL.md".to_string(),
                    NotificationKind::Info,
                );
            } else {
                state.overlay = None;
                state.overlay_state = Some(overlay::tools_overlay(listing));
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
        // `/skill off <name>` → after "off", complete skill names.
        let (after_off, looking_for_skill) = match prefix.strip_prefix("off") {
            Some(rest) => (true, rest.trim_start().is_empty() || !rest.contains(' ')),
            None => (false, true),
        };
        let _ = after_off;
        let skills = state.skills.read();
        skills
            .all()
            .into_iter()
            .filter(|s| {
                if looking_for_skill {
                    let last = prefix.rsplit(' ').next().unwrap_or("");
                    s.name.to_lowercase().starts_with(&last.to_lowercase())
                } else {
                    false
                }
            })
            .map(|s| CompletionItem {
                text: s.name.clone(),
                label: s.name.clone(),
                description: Some(s.description.clone()),
                kind: CompletionKind::SlashArgument {
                    command: "skill".to_string(),
                },
            })
            .collect()
    }
}
