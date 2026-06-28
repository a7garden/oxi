//! `/advisor` — toggle the read-only advisor reviewer on/off, or show status.

use super::super::registry::SlashCommand;
use crate::tui::app::NotificationKind;
use crate::tui::slash::{SlashCtx, SlashOutcome};

fn advisor_help() -> String {
    r#"Advisor Commands:

  /advisor          Toggle the read-only advisor on/off
  /advisor on       Enable the advisor
  /advisor off      Disable the advisor
  /advisor status   Show advisor status (enabled, backlog, turns)

The advisor shadows the primary agent as a peer reviewer and surfaces advice
(nit / concern / blocker) via an `advise` tool. Configure its model with the
`advisor` role in /roles."#
        .to_string()
}

/// `/advisor <on|off|toggle|status>` — control the read-only advisor.
pub(crate) struct AdvisorCommand;

impl SlashCommand for AdvisorCommand {
    fn name(&self) -> &str {
        "advisor"
    }
    fn description(&self) -> &str {
        "Toggle the read-only advisor reviewer (on/off/status)"
    }
    fn usage(&self) -> &str {
        "/advisor <on|off|toggle|status>"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let state = &mut *ctx.state;
        let session = ctx.session;
        let sub = args.trim();
        match sub {
            "" | "toggle" => match session.toggle_advisor() {
                Ok(true) => state
                    .add_notification("Advisor enabled.".to_string(), NotificationKind::Success),
                Ok(false) => {
                    state.add_notification("Advisor disabled.".to_string(), NotificationKind::Info)
                }
                Err(e) => state.add_notification(format!("Advisor: {e}"), NotificationKind::Error),
            },
            "on" | "enable" => match session.set_advisor_enabled(true) {
                Ok(_) => state
                    .add_notification("Advisor enabled.".to_string(), NotificationKind::Success),
                Err(e) => state.add_notification(
                    format!("Advisor could not start: {e}"),
                    NotificationKind::Error,
                ),
            },
            "off" | "disable" => {
                let _ = session.set_advisor_enabled(false);
                state.add_notification("Advisor disabled.".to_string(), NotificationKind::Info);
            }
            "status" => {
                state.add_notification(session.advisor_status(), NotificationKind::Info);
            }
            _ => {
                state.add_notification(advisor_help(), NotificationKind::Warning);
            }
        }
        SlashOutcome::Handled
    }
}
