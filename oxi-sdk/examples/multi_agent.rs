//! Multi-agent parallel execution example.
//!
//! Run: `cargo run -p oxi-sdk --example multi_agent`

use std::sync::Arc;

use oxi_sdk::prelude::*;
use oxi_sdk::AgentGroup;
use oxi_sdk::GroupStrategy;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Set ANTHROPIC_API_KEY to run this example.");
        return Ok(());
    }

    let oxi = OxiBuilder::new().with_builtins().build();

    let config = AgentConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".into(),
        max_iterations: 5,
        ..Default::default()
    };

    let reviewer = oxi
        .agent(config.clone())
        .system_prompt("You are a code reviewer. Review briefly.")
        .build()?;

    let tester = oxi
        .agent(config)
        .system_prompt("You are a test engineer. Suggest tests briefly.")
        .build()?;

    let group = AgentGroup::new(GroupStrategy::Parallel { max_concurrency: 2 })
        .agent(Arc::new(reviewer))
        .agent(Arc::new(tester));

    let result = group
        .run("Analyze a hypothetical REST API for issues.".into())
        .await?;

    println!(
        "Results ({} agents, {} ms):",
        result.results.len(),
        result.total_duration_ms
    );
    for output in &result.results {
        let preview = &output.content[..output.content.len().min(120)];
        println!(
            "  {} [{}]: {}",
            output.name,
            if output.success { "OK" } else { "FAIL" },
            preview
        );
    }
    Ok(())
}
