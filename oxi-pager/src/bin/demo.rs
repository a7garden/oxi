//! Standalone pager binary — tests the grok-quality TUI render.
//!
//! Run with: cargo run -p oxi-pager

use oxi_pager::{BackgroundEvent, run};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Simulate agent events for demo
    let demo_tx = tx.clone();
    tokio::spawn(async move {
        // Simulate some streaming text
        let _ = demo_tx.send(BackgroundEvent::AssistantDelta("Hello! ".into()));
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        let _ = demo_tx.send(BackgroundEvent::AssistantDelta(
            "I'm the grok-quality TUI running on oxi. ".into(),
        ));
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        let _ = demo_tx.send(BackgroundEvent::AssistantDelta(
            "You can type messages and see them rendered with TokyoNight theme.".into(),
        ));
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        let _ = demo_tx.send(BackgroundEvent::StreamDone);
    });

    // Need to provide a session type. Use () for demo.
    run((), rx).await
}
