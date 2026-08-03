//! Integration test utilities — MockProvider and helpers.

use futures::Stream;
use oxicode_ai::{
    Api, AssistantMessage, ContentBlock, Context, Model, Provider, ProviderEvent, StopReason,
    StreamOptions, StreamResult, TextContent, Usage,
};
use oxicode_sdk::OxicodeBuilder;
use std::future::Future;
use std::pin::Pin;

/// Mock provider that echoes the last user message.
pub struct MockProvider;

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
                    oxicode_ai::Message::User(u) => u.content.as_str().map(|s| s.to_string()),
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

pub fn mock_model() -> Model {
    Model::new(
        "model",
        "Mock",
        Api::OpenAiCompletions,
        "mock",
        "http://localhost",
    )
}

pub fn mock_oxicode() -> oxicode_sdk::Oxicode {
    OxicodeBuilder::new()
        .provider("mock", MockProvider)
        .model(mock_model())
        .build()
}
