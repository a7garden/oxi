//! `/roles` — open the model-roles configuration overlay.

use super::super::registry::SlashCommand;
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// `/roles` — view and edit per-role model assignments.
pub(crate) struct RolesCommand;

impl SlashCommand for RolesCommand {
    fn name(&self) -> &str {
        "roles"
    }
    fn description(&self) -> &str {
        "Edit model roles (assign a model per role: default, smol, slow, ...)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay_state = Some(overlay::roles_config_overlay());
        SlashOutcome::Handled
    }
}
