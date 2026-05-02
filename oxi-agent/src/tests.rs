//! Integration tests for oxi-agent

use oxi_agent::{Agent, AgentConfig, AgentEvent, AgentState};
use oxi_ai::{Model, Provider, ProviderEvent, StreamOptions};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use futures::StreamExt;

/// Mock provider for testing
struct MockProvider {
    responses: Vec<MockResponse>,
    call_count: Arc<Mutex<usize>>,
}

#[derive(Clone)]
struct MockResponse {
    content: String,
    stop_reason: String,
    tool_calls: Vec<MockToolCall>,
}

#[derive(Clone)]
struct MockToolCall {
    id: String,
    name: String,
    input: HashMap<String, serde_json::Value>,
}

impl MockProvider {
    fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses,
            call_count: Arc::new(Mutex::new(0)),
        }
    }
}

impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }
}

#[::async_trait::async_trait]
impl oxi_ai::Provider for MockProvider {
    async fn stream(
        &self,
        _model: &Model,
        _context: &oxi_ai::Context,
        _options: Option<StreamOptions>,
    ) -> std::result::Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = std::result::Result<ProviderEvent, oxi_ai::ProviderError>>
                + Send,
        >,
        oxi_ai::ProviderError,
    > {
        let mut call_count = self.call_count.lock().unwrap();
        *call_count += 1;
        let idx = (*call_count - 1) % self.responses.len();
        let response = self.responses[idx].clone();

        let stream = async_stream::stream! {
            // Stream text chunk
            yield Ok(ProviderEvent::ContentBlock {
                content: oxi_ai::ContentBlock::Text(oxi_ai::TextContent::new(response.content.clone()))
            });
            
            // Stream tool calls if any
            for tool_call in &response.tool_calls {
                yield Ok(ProviderEvent::ToolUse {
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    input: tool_call.input.clone(),
                });
            }
            
            // Complete
            yield Ok(ProviderEvent::MessageStop {
                index: 0,
                content: response.content,
                stop_reason: oxi_ai::StopReason::Stop,
            });
        };

        Ok(Box::pin(stream))
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
    let config = AgentConfig::new("gpt-4")
        .with_name("my-agent")
        .with_system_prompt("You are helpful")
        .with_max_iterations(5);
    assert_eq!(config.model_id, "gpt-4");
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
    use oxi_agent::state::SharedState;
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
        stop_reason: "stop".to_string(),
        tool_calls: vec![],
    }]));
    
    let config = AgentConfig::new("mock-model");
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
        stop_reason: "stop".to_string(),
        tool_calls: vec![],
    }]));
    
    let config = AgentConfig::default();
    let agent = Agent::new(provider, config);
    
    let (_, events) = agent.run("Test prompt".to_string()).await.unwrap();
    
    assert!(events.first().map(|e| matches!(e, AgentEvent::Start { .. })).unwrap_or(false));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Thinking)));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::TextChunk { .. })));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Complete { .. })));
}

#[test]
fn test_tool_definition() {
    use oxi_agent::types::ToolDefinition;
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
    use oxi_agent::types::ToolCall;
    let tool_call = ToolCall::new("call_1", "get_weather", r#"{"city": "NYC"}"#);
    assert_eq!(tool_call.id, "call_1");
    assert_eq!(tool_call.name, "get_weather");
}

#[test]
fn test_tool_result() {
    use oxi_agent::types::ToolResult;
    let success = ToolResult::success("call_1", "Sunny, 72°F");
    assert!(!success.is_error);
    let error = ToolResult::error("call_2", "City not found");
    assert!(error.is_error);
}
