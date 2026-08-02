//! Demonstrates the oxi-sdk builder pattern with a single agent.
//!
//! This example is fully self-contained: it ships an inline `MockProvider` so
//! it runs without any API keys. Run it with:
//!
//! ```text
//! cargo run -p oxi-sdk --example builder_demo
//! ```

use std::pin::Pin;

use anyhow::Result;
use futures::Stream;
use oxi_ai::{
    Api, AssistantMessage, ContentBlock, Context, Message, Model, Provider, ProviderEvent,
    StopReason, StreamOptions, StreamResult, TextContent, Usage,
};
use oxi_sdk::prelude::*;

// ─── Mock provider ─────────────────────────────────────────────────────────
//
// A self-contained `Provider` that echoes the last user message back as the
// assistant reply. Lets us exercise the SDK end-to-end without any external
// service or API key.

struct MockProvider;

impl Provider for MockProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            // Pull the most recent user turn out of the context; fall back to
            // a placeholder if none is present.
            let last_msg = context
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    Message::User(u) => u.content.as_str().map(str::to_string),
                    _ => None,
                })
                .unwrap_or_else(|| "mock response".to_string());

            // Build the assistant message that the stream will emit.
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

/// Build the catalog model entry the engine will route to the mock provider.
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
    // 1. Build the engine and register our mock provider + model.
    let oxi = OxiBuilder::new()
        .provider("mock", MockProvider)
        .model(mock_model())
        .build();

    // 2. Configure a single agent that targets the mock model.
    let agent = oxi
        .agent(AgentConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        })
        .system_prompt("You are a helpful assistant. Respond briefly.")
        .build()?;

    // 3. Run the agent and collect the final response plus the event trace.
    let (response, events) = agent.run("Hello, oxi!".into()).await?;

    println!("Response: {}", response.content);
    println!("Events:   {}", events.len());
    Ok(())
}
