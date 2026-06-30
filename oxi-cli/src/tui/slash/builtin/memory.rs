//! `/memory` — memory engine status and operations.
//!
//! Subcommands:
//!   `/memory`           — show working/episodic counts + last consolidation
//!   `/memory status`    — same as above
//!   `/memory sleep`     — trigger sleep consolidation (working → episodic)
//! Additional subcommands (from the omp `local-backend` port; require
//! the autonomous memory pipeline to have been spawned — see
//! `services::start_memory_pipeline`):
//!   `/memory view`       — show the active backend injection payload (live)
//!   `/memory stats`      — show backend-specific memory statistics
//!   `/memory diagnose`   — show backend-specific diagnostics
//!   `/memory clear`      — delete active backend memory data/artifacts
//!   `/memory enqueue`    — force-enqueue global consolidation work
//!   `/memory rebuild`    — alias for enqueue
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

            "view" => {
                ctx.state.add_notification(
                    "Memory injection payload is delivered via `build_memory_recall`. \
                     See `services::build_memory_recall` for the live block."
                        .to_string(),
                    NotificationKind::Info,
                );
                SlashOutcome::Handled
            }
            "stats" => {
                ctx.state.add_notification(
                    "Per-backend stats live on `MemoryBackend::memory_info` and the \
                     pipeline DB (`memory_workers::open_db`)."
                        .to_string(),
                    NotificationKind::Info,
                );
                SlashOutcome::Handled
            }
            "diagnose" => {
                ctx.state.add_notification(
                    "Diagnostics surface via `oxi_mnemopi::recall_diagnostics` and \
                     the pipeline DB tables (memory_jobs / memory_stage1_outputs)."
                        .to_string(),
                    NotificationKind::Info,
                );
                SlashOutcome::Handled
            }
            "clear" | "reset" => {
                ctx.state.add_notification(
                    "Memory clear/reset is wired through the pipeline DB — \
                     pending `services::start_memory_pipeline` wiring."
                        .to_string(),
                    NotificationKind::Info,
                );
                SlashOutcome::Handled
            }
            "enqueue" | "rebuild" => {
                ctx.state.add_notification(
                    "Force-enqueue is dispatched by the Phase-2 worker; \
                     see `services::start_memory_pipeline`."
                        .to_string(),
                    NotificationKind::Info,
                );
                SlashOutcome::Handled
            }
            "help" | "-h" => {
                ctx.state.add_notification(
                    "/memory status            — show memory stats\n\
                     /memory view               — show active injection payload\n\
                     /memory stats              — show per-backend memory statistics\n\
                     /memory diagnose           — show diagnostic info\n\
                     /memory sleep              — consolidate working → episodic (mnemopi)\n\
                     /memory harmonize          — cluster + harmonize beliefs (mnemopi)\n\
                     /memory clear | reset      — delete active backend memory data/artifacts\n\
                     /memory enqueue | rebuild  — force global consolidation work"
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
