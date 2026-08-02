//! Multi-agent parallel execution example using a self-contained mock.
//!
//! Run: `cargo run -p oxi-sdk --example multi_agent`
//!
//! No API key is required — the inline `MockProvider` echoes the last user
//! message, so both agents in the group respond deterministically and offline.

use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures::Stream;
use oxi_ai::{
    Api, AssistantMessage, ContentBlock, Context, Message, Model, Provider, ProviderEvent,
    StopReason, StreamOptions, StreamResult, TextContent, Usage,
};
use oxi_sdk::prelude::*;
use oxi_sdk::{AgentGroup, GroupStrategy};

// ─── Mock provider ─────────────────────────────────────────────────────────
//
// `Provider` impl that returns the most recent user message as the assistant
// reply. See `examples/builder_demo.rs` for a fully commented version of the
// same logic; here we keep the inline block minimal so the focus stays on the
// multi-agent orchestration API.

struct MockProvider;

impl Provider for MockProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let last_msg = context
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    Message::User(u) => u.content.as_str().map(str::to_string),
                    _ => None,
                })
                .unwrap_or_else(|| "mock response".to_string());

            let mut msg = AssistantMessage::new(Api::OpenAiCompletions, "mock", "mock/model");
            msg.content
                .push(ContentBlock::Text(TextContent::new(&last_msg)));
            msg.stop_reason = StopReason::Stop;
            msg.usage = Usage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 150,
                cost: Default::default(),
            };

            let partial = std::sync::Arc::new(msg.clone());
            let events: Vec<ProviderEvent> = vec![
                ProviderEvent::Start {
                    partial: partial.clone(),
                },
                ProviderEvent::TextDelta {
                    content_index: 0,
                    delta: last_msg,
                    partial,
                },
                ProviderEvent::Done {
                    reason: StopReason::Stop,
                    message: msg,
                },
            ];

            Ok(Box::pin(futures::stream::iter(events))
                as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }
}

fn mock_model() -> Model {
    Model::new(
        "model",
        "Mock",
        Api::OpenAiCompletions,
        "mock",
        "http://localhost",
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Build the engine with our mock provider + model.
    let oxi = OxiBuilder::new()
        .provider("mock", MockProvider)
        .model(mock_model())
        .build();

    // 2. Spin up two agents with different system prompts. Each gets its own
    //    configuration so a future PR can swap their model IDs independently.
    let config = AgentConfig {
        model_id: "mock/model".into(),
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

    // 3. Run both agents concurrently. The group collects each agent's
    //    result into a `GroupResult` we can iterate over.
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
