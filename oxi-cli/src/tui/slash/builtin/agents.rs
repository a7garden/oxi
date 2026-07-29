//! `/agents` — open the Agent Hub overlay (advisor + subagent monitor).

use super::super::registry::SlashCommand;
use crate::tui::overlay::agent_hub::AgentHubOverlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

pub(crate) struct AgentsCommand;

impl SlashCommand for AgentsCommand {
    fn name(&self) -> &str {
        "agents"
    }
    fn aliases(&self) -> &[&str] {
        &["hub"]
    }
    fn description(&self) -> &str {
        "Open the agent hub overlay (advisor + subagents)"
    }
    fn usage(&self) -> &str {
        "/agents"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay = None;
        ctx.state.overlay_state = Some(Box::new(AgentHubOverlay::new(ctx.session.clone_handle())));
        SlashOutcome::Handled
    }
}
