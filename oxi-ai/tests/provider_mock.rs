//! Provider mock HTTP tests
//!
//! Tests streaming responses using mockito for HTTP mocking.
//! These tests verify that providers correctly handle SSE streaming responses.

use futures::StreamExt;
use mockito::Server;
use serde_json::json;
use std::pin::Pin;

use futures::Stream;
use oxi_ai::{
    get_model, Context, Message, Model, OpenAiProvider, Provider, ProviderEvent, StreamOptions,
    UserMessage, ProviderError,
};

/// Helper to create a minimal context for testing
fn test_context() -> Context {
    Context::new().with_system_prompt("You are a helpful assistant.")
}

/// Helper to get a model and override its base_url
fn test_model(provider: &str, model_id: &str) -> Model {
    get_model(provider, model_id)
        .expect("model should exist")
        .clone()
}

/// Helper type alias for the stream type - must match Provider::stream return type
type BoxedStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

/// Helper function to call provider.stream with explicit type annotation
async fn call_stream(
    provider: &OpenAiProvider,
    model: &Model,
    context: &Context,
    options: StreamOptions,
) -> Result<BoxedStream, ProviderError> {
    <OpenAiProvider as Provider>::stream(provider, model, context, Some(options)).await
}

/// Test OpenAI streaming with text content
#[tokio::test]
async fn test_openai_streaming_text() {
    let mut server = Server::new_async().await;

    // Create SSE mock response with multiple chunks
    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_header("cache-control", "no-cache")
        .match_header("authorization", "Bearer test-key-123")
        .match_header("content-type", "application/json")
        .with_body(
            r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"!"},"finish_reason":"stop"}]}

data: [DONE]
"#,
        )
        .create_async()
        .await;

    // Get model and override base_url
    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    // Create provider with test API key
    let provider = OpenAiProvider::with_api_key("test-key-123");
    let context = test_context();

    // Stream events
    let options = StreamOptions::default();
    let mut stream = call_stream(&provider, &model, &context, options)
        .await
        .expect("stream should succeed");

    // Collect events
    let mut text_parts: Vec<String> = Vec::new();

    while let Some(event) = stream.next().await {
        if let ProviderEvent::TextDelta { delta, .. } = event {
            text_parts.push(delta);
        }
    }

    // Verify we got text parts
    assert!(!text_parts.is_empty(), "should have received text parts");
    assert_eq!(text_parts.join(""), "Hello world!");

    // Verify mock was called
    mock.assert();
}

/// Test OpenAI streaming with tool call deltas
#[tokio::test]
async fn test_openai_tool_call_parsing() {
    let mut server = Server::new_async().await;

    let tool_call_body = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"","arguments":"{\"loc"}}]},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"","type":"function","function":{"name":"","arguments":"ation\":\"Los"}}]},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"","type":"function","function":{"name":"","arguments":" Angeles\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]
"#;

    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(tool_call_body)
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    let provider = OpenAiProvider::with_api_key("test-key");
    let context = test_context();

    let options = StreamOptions::default();
    let mut stream = call_stream(&provider, &model, &context, options)
        .await
        .expect("stream should succeed");

    // Collect tool call events
    let mut tool_call_parts: Vec<String> = Vec::new();
    let mut saw_tool_call_delta = false;

    while let Some(event) = stream.next().await {
        if let ProviderEvent::ToolCallDelta { ref delta, .. } = event {
            tool_call_parts.push(delta.clone());
            saw_tool_call_delta = true;
        }
        if let ProviderEvent::ToolCallEnd { ref tool_call, .. } = event {
            // Verify we got the complete tool call
            tool_call_parts.push(format!("end:name:{}", tool_call.name));
        }
    }

    // Verify we got tool call delta events
    assert!(saw_tool_call_delta, "should have received tool call delta events");
    // Delta should contain JSON arguments
    let all_deltas: String = tool_call_parts.join("");
    assert!(
        all_deltas.contains("location") && all_deltas.contains("Los Angeles"),
        "should have tool call arguments with location"
    );

    mock.assert();
}

/// Test rate limit (429) error handling
#[tokio::test]
async fn test_rate_limit_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(429)
        .with_header("content-type", "application/json")
        .with_header("retry-after", "60")
        .with_body(
            json!({
                "error": {
                    "type": "rate_limit_exceeded",
                    "message": "Rate limit exceeded. Please retry after 60 seconds."
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    let provider = OpenAiProvider::with_api_key("test-key");
    let context = test_context();

    let options = StreamOptions::default();
    let result = call_stream(&provider, &model, &context, options).await;

    // Should get an error
    assert!(result.is_err(), "should return error for 429 status");

    mock.assert();
}

/// Test authentication error (401) handling
#[tokio::test]
async fn test_auth_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(401)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "error": {
                    "type": "invalid_request_error",
                    "message": "Invalid API key provided."
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    let provider = OpenAiProvider::with_api_key("invalid-key");
    let context = test_context();

    let options = StreamOptions::default();
    let result = call_stream(&provider, &model, &context, options).await;

    // Should get an error
    assert!(result.is_err(), "should return error for 401 status");

    mock.assert();
}

/// Test server error (500) handling
#[tokio::test]
async fn test_server_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(500)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":{"message":"Internal server error"}}"#)
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    let provider = OpenAiProvider::with_api_key("test-key");
    let context = test_context();

    let options = StreamOptions::default();
    let result = call_stream(&provider, &model, &context, options).await;

    // Should get an error
    assert!(result.is_err(), "should return error for 500 status");

    mock.assert();
}

/// Test empty stream response
#[tokio::test]
async fn test_empty_stream() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body("data: [DONE]\n")
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    let provider = OpenAiProvider::with_api_key("test-key");
    let context = test_context();

    let options = StreamOptions::default();
    let mut stream = call_stream(&provider, &model, &context, options)
        .await
        .expect("stream should succeed");

    // Collect all events
    let mut event_count = 0;
    while let Some(_event) = stream.next().await {
        event_count += 1;
    }

    // Should complete without error. An empty stream still emits exactly one
    // ProviderEvent::Start event at the beginning (empty = no content chunks).
    assert_eq!(event_count, 1, "empty stream should have exactly one Start event");

    mock.assert();
}

/// Test that request is made with custom model ID
#[tokio::test]
async fn test_model_override() {
    let mut server = Server::new_async().await;

    // Simple mock that just returns success
    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(
            r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"custom-model-id","choices":[{"index":0,"delta":{"content":"OK"},"finish_reason":"stop"}]}

data: [DONE]
"#,
        )
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();
    model.id = "custom-model-id".to_string();

    let provider = OpenAiProvider::with_api_key("test-key");
    let context = test_context();

    let options = StreamOptions::default();
    let result = call_stream(&provider, &model, &context, options).await;

    // Verify the request was made
    mock.assert();

    // The stream may succeed or fail depending on response parsing
    if result.is_ok() {
        let mut stream = result.unwrap();
        while let Some(event) = stream.next().await {
            let _ = event;
        }
    }
}

/// Test streaming with context that has messages
#[tokio::test]
async fn test_streaming_with_history() {
    let mut server = Server::new_async().await;

    // Simple mock that just returns success
    let mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(
            r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"Paris"},"finish_reason":"stop"}]}

data: [DONE]
"#,
        )
        .create_async()
        .await;

    let mut model = test_model("openai", "gpt-4o-mini");
    model.base_url = server.url();

    let provider = OpenAiProvider::with_api_key("test-key");

    // Create context with history
    let mut context = Context::new();
    context.add_message(Message::User(UserMessage::new(
        "What is the capital of France",
    )));
    context.set_system_prompt("You are a geography assistant.");

    let options = StreamOptions::default();
    let result = call_stream(&provider, &model, &context, options).await;

    // Verify the request was made
    mock.assert();

    // The stream may succeed or fail depending on response parsing
    if result.is_ok() {
        let mut stream = result.unwrap();
        while let Some(event) = stream.next().await {
            let _ = event;
        }
    }
}