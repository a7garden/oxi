//! `/memory` — memory engine status and operations.
//!
//! Subcommands:
//!   `/memory`           — show working/episodic counts + last consolidation
//!   `/memory status`    — same as above
//!   `/memory sleep`     — trigger sleep consolidation (working → episodic)
//!   `/memory harmonize` — trigger SHMR harmonization

use super::super::registry::SlashCommand;
use crate::tui::app::NotificationKind;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// Memory engine status and operations.
pub(crate) struct MemoryCommand;

impl SlashCommand for MemoryCommand {
    fn name(&self) -> &str {
        "memory"
    }

    fn aliases(&self) -> &[&str] {
        &["mem"]
    }

    fn description(&self) -> &str {
        "Memory engine status, sleep consolidation, and harmonization"
    }

    fn usage(&self) -> &str {
        "/memory [status|sleep|harmonize|help]"
    }

    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let sub = args.split_whitespace().next().unwrap_or("status");

        let config = ctx.session.agent_ref().get_config();
        let Some(ref backend) = config.memory else {
            ctx.state.add_notification(
                "Memory is disabled. Enable it in /settings (memory_enabled = true).".to_string(),
                NotificationKind::Warning,
            );
            return SlashOutcome::Handled;
        };

        match sub {
            "status" | "" => {
                if let Some(info) = backend.memory_info() {
                    ctx.state.add_notification(info, NotificationKind::Info);
                } else {
                    ctx.state.add_notification(
                        "Memory is enabled (basic backend). Use memory_recall/retain tools to interact."
                            .to_string(),
                        NotificationKind::Info,
                    );
                }
                SlashOutcome::Handled
            }
            "sleep" | "consolidate" => {
                if let Some(msg) = backend.trigger_consolidation() {
                    ctx.state.add_notification(msg, NotificationKind::Info);
                } else {
                    ctx.state.add_notification(
                        "Sleep consolidation not supported by this backend.".to_string(),
                        NotificationKind::Warning,
                    );
                }
                SlashOutcome::Handled
            }
            "harmonize" | "harmonise" => {
                if let Some(msg) = backend.trigger_harmonize() {
                    ctx.state.add_notification(msg, NotificationKind::Info);
                } else {
                    ctx.state.add_notification(
                        "SHMR harmonization not supported by this backend.".to_string(),
                        NotificationKind::Warning,
                    );
                }
                SlashOutcome::Handled
            }
            "help" | "-h" => {
                ctx.state.add_notification(
                    "/memory status    — show memory stats\n\
                     /memory sleep     — consolidate working → episodic\n\
                     /memory harmonize — cluster + harmonize beliefs"
                        .to_string(),
                    NotificationKind::Info,
                );
                SlashOutcome::Handled
            }
            _ => {
                ctx.state.add_notification(
                    format!("Unknown subcommand: '{sub}'. Try /memory help."),
                    NotificationKind::Warning,
                );
                SlashOutcome::Handled
            }
        }
    }
}
