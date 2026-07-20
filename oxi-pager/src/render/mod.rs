//! Render glue — PR-4 ships a no-op; PR-5 fills in real rendering.

use crate::state::PagerState;

pub fn render(_state: &PagerState) -> anyhow::Result<()> {
    Ok(())
}
