//! Demonstrates the oxi-sdk multi-agent builder pattern.
//!
//! Run with: cargo run -p oxi-sdk --example builder_demo

fn main() {
    println!("oxi-sdk Multi-Agent Builder Demo");
    println!("=================================");
    println!();

    // The SDK provides these core types for building multi-agent systems:
    //
    // 1. Oxi (engine) - Central orchestrator created via OxiBuilder:
    //    let engine = OxiBuilder::new()
    //        .include_builtins(true)
    //        .provider("my-provider", my_provider)
    //        .build();
    //
    // 2. AgentBuilder - Configure individual agents:
    //    let agent = AgentBuilder::new("researcher")
    //        .model_id("claude-sonnet-4-20250514")
    //        .system_prompt("You are a research assistant.")
    //        .build();
    //
    // 3. AgentGroup - Orchestrate multiple agents:
    //    let group = AgentGroup::new("research-and-write")
    //        .strategy(Strategy::Pipeline)
    //        .add_agent(researcher)
    //        .add_agent(writer)
    //        .build();
    //
    // 4. MessageBus - Pub/sub inter-agent communication
    //
    // 5. AgentMetrics - Track runs, tokens, durations

    println!("SDK Components:");
    println!("  - OxiBuilder: configure the engine with providers");
    println!("  - AgentBuilder: configure individual agents");
    println!("  - AgentGroup: orchestrate multi-agent pipelines and parallel execution");
    println!("  - MessageBus: pub/sub inter-agent communication");
    println!("  - AgentMetrics: track runs, tokens, and durations");
    println!();
    println!("See oxi-sdk documentation for full API details.");
}
