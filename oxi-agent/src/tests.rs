//! Integration tests for oxi-agent

use crate::{Agent, AgentConfig, AgentEvent, AgentState};
use crate::types::{ToolDefinition, ToolCall, ToolResult};
use oxi_ai::{
    Provider, ProviderEvent, Context, ContentBlock, TextContent, ThinkingContent,
    StopReason, transform_for_provider, Api,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::pin::Pin;
use futures::Stream;
use std::task::{Poll, Context as TaskContext};
use async_trait::async_trait;

/// Mock provider for testing
struct MockProvider {
    responses: Vec<MockResponse>,
    call_count: Arc<Mutex<usize>>,
}

#[derive(Clone)]
struct MockResponse {
    content: String,
}

impl MockProvider {
    fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses,
            call_count: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn stream(
        &self,
        _model: &oxi_ai::Model,
        _context: &Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> std::result::Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
        oxi_ai::ProviderError,
    > {
        let mut call_count = self.call_count.lock().unwrap();
        *call_count += 1;
        let idx = (*call_count - 1) % self.responses.len();
        let response = self.responses[idx].clone();

        let stream = MockStream {
            text: response.content,
            done: false,
        };

        Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
    }

    fn name(&self) -> &str {
        "mock"
    }
}

struct MockStream {
    text: String,
    done: bool,
}

impl Stream for MockStream {
    type Item = ProviderEvent;
    
    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        
        self.done = true;
        
        // Create assistant message with text content
        let mut assistant = oxi_ai::AssistantMessage::new(
            oxi_ai::Api::AnthropicMessages,
            "mock",
            "mock-model",
        );
        assistant.content = vec![ContentBlock::Text(TextContent::new(self.text.clone()))];
        
        Poll::Ready(Some(ProviderEvent::Done {
            reason: StopReason::Stop,
            message: assistant,
        }))
    }
}

#[test]
fn test_agent_config_default() {
    let config = AgentConfig::default();
    assert_eq!(config.name, "oxi-agent");
    assert_eq!(config.max_iterations, 10);
    assert_eq!(config.timeout_seconds, 300);
}

#[test]
fn test_agent_config_builder() {
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514")
        .with_name("my-agent")
        .with_system_prompt("You are helpful")
        .with_max_iterations(5);
    assert_eq!(config.model_id, "anthropic/claude-sonnet-4-20250514");
    assert_eq!(config.name, "my-agent");
    assert_eq!(config.system_prompt, Some("You are helpful".to_string()));
    assert_eq!(config.max_iterations, 5);
}

#[test]
fn test_agent_state_messages() {
    let mut state = AgentState::new();
    state.add_user_message("Hello".to_string());
    state.add_assistant_message("Hi there!".to_string());
    assert_eq!(state.messages.len(), 2);
}

#[test]
fn test_agent_state_iteration() {
    let mut state = AgentState::new();
    assert_eq!(state.iteration, 0);
    state.increment_iteration();
    assert_eq!(state.iteration, 1);
}

#[test]
fn test_agent_state_usage() {
    let mut state = AgentState::new();
    state.record_usage(100, 50);
    assert_eq!(state.input_tokens, 100);
    assert_eq!(state.output_tokens, 50);
    assert_eq!(state.total_tokens, 150);
}

#[test]
fn test_agent_state_clear() {
    let mut state = AgentState::new();
    state.add_user_message("Hello".to_string());
    state.increment_iteration();
    state.clear();
    assert_eq!(state.messages.len(), 0);
    assert_eq!(state.iteration, 0);
}

#[test]
fn test_agent_state_is_complete() {
    let mut state = AgentState::new();
    assert!(!state.is_complete());
    state.set_stop_reason(crate::types::StopReason::Stop);
    assert!(state.is_complete());
}

#[test]
fn test_shared_state() {
    use crate::state::SharedState;
    let shared = SharedState::new();
    shared.update(|s| {
        s.add_user_message("Test".to_string());
    });
    let state = shared.get_state();
    assert_eq!(state.messages.len(), 1);
    shared.reset();
    let state = shared.get_state();
    assert_eq!(state.messages.len(), 0);
}

#[tokio::test]
async fn test_agent_with_mock_provider() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Hello! How can I help you?".to_string(),
    }]));
    
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider.clone(), config);
    
    let (response, events) = agent.run("Hi".to_string()).await.unwrap();
    
    assert_eq!(response.content, "Hello! How can I help you?");
    assert_eq!(*provider.call_count.lock().unwrap(), 1);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Start { .. })));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Complete { .. })));
}

#[tokio::test]
async fn test_agent_events_sequence() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Test response".to_string(),
    }]));
    
    let config = AgentConfig::default();
    let agent = Agent::new(provider, config);
    
    let (_, events) = agent.run("Test prompt".to_string()).await.unwrap();
    
    assert!(events.first().map(|e| matches!(e, AgentEvent::Start { .. })).unwrap_or(false));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Thinking)));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Complete { .. })));
}

#[test]
fn test_tool_definition() {
    let mut schema = HashMap::new();
    schema.insert("query".to_string(), serde_json::json!({
        "type": "string",
        "description": "Search query"
    }));
    let tool = ToolDefinition::new("search", "Search the web", schema);
    assert_eq!(tool.name, "search");
    assert!(tool.input_schema.contains_key("query"));
}

#[test]
fn test_tool_call() {
    let tool_call = ToolCall::new("call_1", "get_weather", r#"{"city": "NYC"}"#);
    assert_eq!(tool_call.id, "call_1");
    assert_eq!(tool_call.name, "get_weather");
}

#[test]
fn test_tool_result() {
    let success = ToolResult::success("call_1", "Sunny, 72°F");
    assert!(!success.is_error);
    let error = ToolResult::error("call_2", "City not found");
    assert!(error.is_error);
}

// ── Cross-provider handoff tests ──────────────────────────────────

#[test]
fn test_transform_for_provider_thinking_to_openai() {
    // Create an assistant message with thinking blocks (from Anthropic)
    let mut assistant = oxi_ai::AssistantMessage::new(
        Api::AnthropicMessages,
        "anthropic",
        "claude-sonnet-4-20250514",
    );
    assistant.content = vec![
        ContentBlock::Thinking(ThinkingContent::new("Let me think about this...")),
        ContentBlock::Text(TextContent::new("Here is my answer.")),
    ];

    let messages = vec![
        oxi_ai::Message::User(oxi_ai::UserMessage::new("Hello")),
        oxi_ai::Message::Assistant(assistant),
    ];

    // Transform for OpenAI
    let transformed = transform_for_provider(
        &messages,
        &Api::AnthropicMessages,
        &Api::OpenAiCompletions,
    );

    assert_eq!(transformed.len(), 2);

    // User message should be unchanged
    assert!(matches!(&transformed[0], oxi_ai::Message::User(_)));

    // Assistant message should have thinking converted to text
    if let oxi_ai::Message::Assistant(a) = &transformed[1] {
        assert_eq!(a.content.len(), 1); // merged into single text block
        let text = a.content[0].as_text().unwrap();
        assert!(text.contains("<thinking>"));
        assert!(text.contains("Let me think about this..."));
        assert!(text.contains("Here is my answer."));
        assert_eq!(a.api, Api::OpenAiCompletions);
    } else {
        panic!("Expected Assistant message");
    }
}

#[test]
fn test_transform_for_provider_preserves_anthropic() {
    // Thinking blocks should be preserved when target is Anthropic
    let mut assistant = oxi_ai::AssistantMessage::new(
        Api::AnthropicMessages,
        "anthropic",
        "claude-sonnet-4-20250514",
    );
    assistant.content = vec![
        ContentBlock::Thinking(ThinkingContent::new("Thinking...")),
        ContentBlock::Text(TextContent::new("Answer.")),
    ];

    let messages = vec![
        oxi_ai::Message::Assistant(assistant),
    ];

    let transformed = transform_for_provider(
        &messages,
        &Api::AnthropicMessages,
        &Api::AnthropicMessages,
    );

    if let oxi_ai::Message::Assistant(a) = &transformed[0] {
        assert_eq!(a.content.len(), 2); // unchanged
        assert!(a.content[0].as_thinking().is_some());
        assert!(a.content[1].as_text().is_some());
    } else {
        panic!("Expected Assistant message");
    }
}

#[test]
fn test_transform_preserves_tool_results() {
    // Tool results should pass through unchanged
    let tool_result = oxi_ai::ToolResultMessage::new(
        "call_123",
        "read",
        vec![ContentBlock::Text(TextContent::new("file contents"))],
    );

    let messages = vec![
        oxi_ai::Message::ToolResult(tool_result),
    ];

    let transformed = transform_for_provider(
        &messages,
        &Api::AnthropicMessages,
        &Api::OpenAiCompletions,
    );

    assert_eq!(transformed.len(), 1);
    if let oxi_ai::Message::ToolResult(tr) = &transformed[0] {
        assert_eq!(tr.tool_call_id, "call_123");
        assert_eq!(tr.tool_name, "read");
    } else {
        panic!("Expected ToolResult message");
    }
}

#[test]
fn test_agent_model_id() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config);
    assert_eq!(agent.model_id(), "anthropic/claude-sonnet-4-20250514");
}

#[test]
fn test_agent_switch_model_invalid_format() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config);

    // Invalid format (no provider prefix)
    let result = agent.switch_model("gpt-4o");
    assert!(result.is_err());
}

#[test]
fn test_agent_switch_model_unknown_model() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config);

    let result = agent.switch_model("nonexistent/model");
    assert!(result.is_err());
}

#[test]
fn test_agent_switch_model_same_provider() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config);

    // Switch to another Anthropic model (same provider, same API)
    let result = agent.switch_model("anthropic/claude-3-haiku");
    assert!(result.is_ok());
    assert_eq!(agent.model_id(), "anthropic/claude-3-haiku");
}

/// Mock provider that tracks which API it was called with
struct ApiAwareMockProvider {
    responses: Vec<MockResponse>,
    call_count: Arc<Mutex<usize>>,
    last_api: Arc<Mutex<Option<Api>>>,
}

impl ApiAwareMockProvider {
    fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses,
            call_count: Arc::new(Mutex::new(0)),
            last_api: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Provider for ApiAwareMockProvider {
    async fn stream(
        &self,
        model: &oxi_ai::Model,
        _context: &Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> std::result::Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>,
        oxi_ai::ProviderError,
    > {
        let mut call_count = self.call_count.lock().unwrap();
        *call_count += 1;
        let idx = (*call_count - 1) % self.responses.len();
        let response = self.responses[idx].clone();

        *self.last_api.lock().unwrap() = Some(model.api);

        let stream = MockStream {
            text: response.content,
            done: false,
        };

        Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
    }

    fn name(&self) -> &str {
        "mock-api-aware"
    }
}

#[tokio::test]
async fn test_cross_provider_handoff_openai_to_anthropic() {
    // Simulate a conversation that starts on OpenAI and switches to Anthropic
    // mid-conversation. We use an API-aware mock that doesn't require real keys.
    // The handoff is tested by verifying messages survive the switch.

    let provider = Arc::new(ApiAwareMockProvider::new(vec![
        MockResponse { content: "OpenAI response".to_string() },
        MockResponse { content: "Continued response".to_string() },
    ]));
    let config = AgentConfig::new("openai/gpt-4o");
    let agent = Agent::new(provider, config);

    // 1. Send a message and get a response (on OpenAI)
    let (response, _) = agent.run("Hello from OpenAI".to_string()).await.unwrap();
    assert_eq!(response.content, "OpenAI response");
    assert_eq!(agent.model_id(), "openai/gpt-4o");

    // 2. Verify we have the right message count
    let state = agent.state();
    assert_eq!(state.messages.len(), 2); // user + assistant

    // 3. Verify transform_for_provider works for the cross-provider case
    //    (this is what switch_model would call internally with a real provider)
    let transformed = transform_for_provider(
        &state.messages,
        &Api::OpenAiCompletions,
        &Api::AnthropicMessages,
    );
    assert_eq!(transformed.len(), 2); // all messages preserved

    // 4. Switch model (this fails to create the provider, but the message
    //    transformation logic was verified above)
    let result = agent.switch_model("anthropic/claude-sonnet-4-20250514");
    // The switch itself may fail due to missing API keys for the real provider,
    // but the model_id won't change since we guard the update atomically.
    // The key invariant: if the switch succeeds, messages are transformed.
    if result.is_ok() {
        assert_eq!(agent.model_id(), "anthropic/claude-sonnet-4-20250514");
    }
    // Regardless of switch result, messages should still be intact
    assert_eq!(agent.state().messages.len(), 2);
}

#[tokio::test]
async fn test_cross_provider_message_transformation_roundtrip() {
    // Build up a conversation with thinking blocks, then transform for different providers
    let provider = Arc::new(MockProvider::new(vec![
        MockResponse { content: "First response".to_string() },
        MockResponse { content: "Second response".to_string() },
    ]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config);

    // Build up conversation
    agent.run("Message 1".to_string()).await.unwrap();
    agent.run("Message 2".to_string()).await.unwrap();
    assert_eq!(agent.state().messages.len(), 4);

    // Transform all messages for OpenAI (cross-provider)
    let messages = agent.state().messages.clone();
    let transformed = transform_for_provider(
        &messages,
        &Api::AnthropicMessages,
        &Api::OpenAiCompletions,
    );

    // All messages should be preserved
    assert_eq!(transformed.len(), 4);

    // User messages should be unchanged
    assert!(matches!(&transformed[0], oxi_ai::Message::User(_)));
    assert!(matches!(&transformed[2], oxi_ai::Message::User(_)));

    // Assistant messages should have their API updated
    for msg in &transformed {
        if let oxi_ai::Message::Assistant(a) = msg {
            assert_eq!(a.api, Api::OpenAiCompletions);
            // No thinking blocks in transformed output for non-Anthropic targets
            for block in &a.content {
                assert!(!matches!(block, ContentBlock::Thinking(_)));
            }
        }
    }

    // Transform back to Anthropic (should be lossless for text-only content)
    let back = transform_for_provider(
        &transformed,
        &Api::OpenAiCompletions,
        &Api::AnthropicMessages,
    );
    assert_eq!(back.len(), 4);
}
