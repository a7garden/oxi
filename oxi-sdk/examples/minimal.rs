//! Minimal agent example.
//!
//! Run: `cargo run -p oxi-sdk --example minimal`

use oxi_sdk::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Set ANTHROPIC_API_KEY to run this example.");
        return Ok(());
    }

    let oxi = OxiBuilder::new().with_builtins().build();

    let agent = oxi
        .agent(AgentConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".into(),
            max_iterations: 5,
            ..Default::default()
        })
        .system_prompt("You are a helpful assistant. Respond briefly.")
        .build()?;

    let (response, events) = agent.run("What is 2+2?".into()).await?;

    println!("Response: {}", response.content);
    println!("Events: {}", events.len());
    Ok(())
}
