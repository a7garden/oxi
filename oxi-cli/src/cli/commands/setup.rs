//! Setup subcommand handler.

use anyhow::Result;

/// Handle `oxi setup [--reset]`
pub async fn handle_setup(reset: bool) -> Result<()> {
    if reset {
        super::config::handle_config_reset(true)?;
    }
    crate::setup_wizard::run().await
}
