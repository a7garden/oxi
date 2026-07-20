//! Standalone pager demo — tests grok-quality TUI render.

use oxi_pager::run;
use std::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (user_tx, _user_rx) = mpsc::channel::<String>();
    let (bg_tx, bg_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(bg_tx);
    run(user_tx, bg_rx).await
}
