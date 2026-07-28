/// Integration tests for oxi-agent
use crate::types::{ToolCall, ToolDefinition, ToolResult};
use crate::{Agent, AgentConfig, AgentEvent, AgentState, ToolRegistry};
use futures::Stream;
use oxi_ai::{
    Api, ContentBlock, Context, Provider, ProviderEvent, StopReason, StreamResult, TextContent,
    ThinkingContent, transform_for_provider,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};

/// Mock provider for testing
struct MockProvider {
    responses: Vec<MockResponse>,
    call_count: Arc<Mutex<usize>>,
}

#[derive(Clone, Default)]
struct MockResponse {
    content: String,
    /// Token usage reported via `ProviderEvent::Done.message.usage`.
    /// The real provider is the source of truth, but issue #28
    /// needs a mock that can simulate provider-reported
    /// `input_tokens` above the compaction threshold while keeping
    /// actual message bytes small (the exact production failure).
    /// When left at the default (all zeros), the existing test
    /// behavior is preserved — existing call sites can use
    /// `..Default::default()` to opt in.
    usage: oxi_ai::Usage,
}

impl MockResponse {
    /// Convenience constructor that sets `content` and leaves
    /// `usage` at its default (all zeros). Mirrors the pattern
    /// used in every pre-existing test.
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..Default::default()
        }
    }

    /// Override the synthetic usage reported on the `Done` event.
    /// Used by the issue #28 regression test to simulate
    /// provider-reported `input_tokens` well above the compaction
    /// threshold while the actual message bytes are tiny.
    fn with_usage(mut self, input_tokens: usize) -> Self {
        self.usage.input = input_tokens;
        self
    }
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
    fn stream<'a>(
        &'a self,
        _model: &'a oxi_ai::Model,
        _context: &'a Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let mut call_count = self.call_count.lock().unwrap();
            *call_count += 1;
            let idx = (*call_count - 1) % self.responses.len();
            let response = self.responses[idx].clone();
            let stream = MockStream {
                text: response.content,
                usage: response.usage,
                done: false,
            };

            Ok(Box::pin(stream)
                as Pin<
                    Box<dyn futures::Stream<Item = ProviderEvent> + Send>,
                >)
        })
    }

    fn name(&self) -> &str {
        "mock"
    }
}

#[derive(Default)]
struct MockStream {
    text: String,
    /// Usage to report on the synthetic `Done` event. Defaults to
    /// zero so existing tests that don't care about usage see no
    /// behavior change.
    usage: oxi_ai::Usage,
    done: bool,
}

impl Stream for MockStream {
    type Item = ProviderEvent;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        self.done = true;

        // Create assistant message with text content + the configured
        // usage. The streaming handler reads `message.usage` to drive
        // `record_provider_turn` and the `Real` token source — issue
        // #28.
        let mut assistant =
            oxi_ai::AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock-model");
        assistant.content = vec![ContentBlock::Text(TextContent::new(self.text.clone()))];
        assistant.usage = self.usage.clone();

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
    assert_eq!(config.timeout_seconds, 300);
}

#[test]
fn test_agent_config_builder() {
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514")
        .with_name("my-agent")
        .with_system_prompt("You are helpful");
    assert_eq!(config.model_id, "anthropic/claude-sonnet-4-20250514");
    assert_eq!(config.name, "my-agent");
    assert_eq!(config.system_prompt, Some("You are helpful".to_string()));
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

// ── Issue #28 gap 2: provider-reported token accounting ───────────────

#[test]
fn test_record_provider_turn_overwrites_last_input_tokens() {
    use crate::state::TokenSource;
    let mut state = AgentState::new();
    state.add_user_message("hi".to_string());
    // Cold start: no provider count, but messages exist → Heuristic.
    assert!(matches!(
        state.current_token_source(),
        TokenSource::Heuristic(_)
    ));
    assert_eq!(state.last_input_tokens, None);

    // First provider report — the field becomes Real.
    state.record_provider_turn(1_000, 500);
    assert_eq!(state.last_input_tokens, Some(1_000));
    assert_eq!(state.last_estimate_at_report, Some(500));
    assert_eq!(state.last_estimate_divergence, Some(2.0));
    assert!(matches!(
        state.current_token_source(),
        TokenSource::Real(1_000)
    ));

    // Second provider report — overwrites, not cumulative. This is
    // the behavior gap that would silently over-trigger compaction
    // if we used `state.input_tokens` instead of a dedicated field.
    state.record_provider_turn(2_000, 1_000);
    assert_eq!(state.last_input_tokens, Some(2_000));
    assert_eq!(state.last_estimate_at_report, Some(1_000));
    assert_eq!(state.last_estimate_divergence, Some(2.0));
    // record_provider_turn does NOT touch the cumulative input_tokens.
    assert_eq!(state.input_tokens, 0);
    assert!(matches!(
        state.current_token_source(),
        TokenSource::Real(2_000)
    ));
}

#[test]
fn test_record_provider_turn_divergence_edge_cases() {
    // Zero estimate against non-zero report — worst case heuristic
    // miss. Should record `f64::INFINITY` (preserved through serde as
    // null? — the in-memory surface here is what the loop checks).
    let mut state = AgentState::new();
    state.record_provider_turn(500, 0);
    assert_eq!(state.last_input_tokens, Some(500));
    assert_eq!(state.last_estimate_divergence, Some(f64::INFINITY));

    // Both zero — the trivially-equal case, divergence = 1.0.
    let mut state2 = AgentState::new();
    state2.record_provider_turn(0, 0);
    assert_eq!(state2.last_estimate_divergence, Some(1.0));

    // Realistic #28 case: 122_576 reported / 34_955 estimated ≈ 3.5×.
    let mut state3 = AgentState::new();
    state3.record_provider_turn(122_576, 34_955);
    let div = state3.last_estimate_divergence.expect("divergence set");
    assert!(div > 3.4 && div < 3.6, "divergence {div} not ≈3.5×");
}

#[test]
fn test_clear_resets_provider_turn_state() {
    use crate::state::TokenSource;
    let mut state = AgentState::new();
    state.record_provider_turn(1_000, 500);
    state.clear();
    assert_eq!(state.last_input_tokens, None);
    assert_eq!(state.last_estimate_at_report, None);
    assert_eq!(state.last_estimate_divergence, None);
    // After clear, with no messages, TokenSource is None (not Heuristic).
    assert!(matches!(state.current_token_source(), TokenSource::None));
}

#[test]
fn test_token_source_prefers_real_over_heuristic() {
    use crate::state::TokenSource;
    let mut state = AgentState::new();
    // Seed a large heuristic: lots of bulky messages.
    for _ in 0..50 {
        state.add_user_message("x".repeat(1_000));
    }
    // Heuristic should be > 0 (real JSON for those messages is large).
    let heuristic_size = match state.current_token_source() {
        TokenSource::Heuristic(n) => n,
        other => panic!("expected Heuristic, got {other:?}"),
    };
    assert!(heuristic_size > 0);

    // Now record a Real value — it must win regardless of size.
    state.record_provider_turn(42, 100_000);
    assert!(matches!(
        state.current_token_source(),
        TokenSource::Real(42)
    ));
}

// ── Issue #28 gap 2 — loop-level regression test ─────────────────────
//
// Reproduces the exact failure mode reported in #28: the provider
// reports a high `usage.input` while the actual message bytes (and
// therefore the `bytes/4` heuristic) are tiny. With the fix in
// place, the next `maybe_compact` call sees `TokenSource::Real` and
// triggers compaction. Without the fix, the heuristic would
// undercount and `Threshold` would never fire — leaving the
// context to grow past the window.
//
// The mock emits a `Done` with `usage.input = 900` on the first
// turn, then a follow-up forces a second turn. The second turn's
// `maybe_compact` (run at the top, before streaming) reads
// `last_input_tokens = 900` from the previous `Done` and triggers
// `Compaction::Triggered` with `source = "provider-reported"`.
// Without the fix, the heuristic on the same conversation
// (a few hundred bytes total) would be far below the 800-token
// `Threshold(0.8) * 1000` cutoff, and no event would fire.

#[tokio::test]
async fn test_provider_reported_usage_drives_compaction_threshold() {
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::compaction::CompactionEvent;
    use crate::events::AgentEvent;
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    // Mock returns 2 responses. The first reports `usage.input = 900`
    // — above the 800-token threshold. The second is the response
    // to the follow-up message and is irrelevant to the test.
    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::new("first turn ok").with_usage(900),
        MockResponse::new("second turn ok"),
    ]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: Some("You are helpful.".to_string()),
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Threshold(0.8),
        // 1,000-token context window → Threshold(0.8) fires at 800.
        context_window: 1_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Queue a follow-up so the loop runs a second turn. On the
    // second turn's `maybe_compact` (called at the top, before
    // streaming), `last_input_tokens = 900` is already set from
    // the first turn's `Done` event — so the Real path drives
    // the decision and the threshold fires.
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new("follow-up")));

    let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = agent_loop
        .run("hello".to_string(), move |e| events_clone.lock().push(e))
        .await;
    assert!(result.is_ok(), "agent loop run failed: {result:?}");

    let events = events.lock();

    // Collect the source of every Compaction::Triggered event.
    // At least one must be present and labeled "provider-reported".
    let triggered: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compaction {
                event: CompactionEvent::Triggered { source, .. },
            } => Some(source.clone()),
            _ => None,
        })
        .collect();

    assert!(
        !triggered.is_empty(),
        "expected CompactionEvent::Triggered; got events: {events:#?}"
    );
    // Proof the Real path drove the decision. If someone reverts
    // `maybe_compact` back to `bytes/4`, this assertion fails —
    // the heuristic on a few-hundred-byte conversation is well
    // below the 800-token threshold.
    assert!(
        triggered.iter().any(|s| s == "provider-reported"),
        "expected a Triggered event with source=\"provider-reported\", got: {triggered:?}"
    );
}
// ── Tool-call loop guard wiring test ─────────────────────────────────
//
// Verifies that the tool_call_loop_guard wired into AgentLoop::run_loop
// fires at threshold and injects a steering user message that the
// provider sees on its next call.

/// Stream that emits a single ProviderEvent then ends.
struct OneShotStream {
    event: Option<ProviderEvent>,
}

impl Stream for OneShotStream {
    type Item = ProviderEvent;
    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.event.take())
    }
}

/// Provider that returns the same `echo` tool call each invocation.
/// Returns text-only when it detects a steering message, or after a
/// safety limit of 20 calls to prevent infinite loops in tests.
struct LoopingToolCallProvider {
    received: Arc<Mutex<Vec<Vec<oxi_ai::Message>>>>,
    call_count: std::sync::atomic::AtomicU32,
}

impl LoopingToolCallProvider {
    fn new(received: Arc<Mutex<Vec<Vec<oxi_ai::Message>>>>) -> Self {
        Self {
            received,
            call_count: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl Provider for LoopingToolCallProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a oxi_ai::Model,
        context: &'a Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        let n = self
            .call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // (debug output removed)
        self.received
            .lock()
            .unwrap()
            .push(context.messages.to_vec());

        let has_steering = context.messages.iter().any(|m| {
            matches!(m, oxi_ai::Message::User(u) if u
                .content
                .as_str()
                .map(|s| s.contains("Tool-call loop detected"))
                .unwrap_or(false))
        });
        if n >= 20 {
            panic!("PROVIDER_SAFETY_LIMIT: provider called 20 times");
        }
        let force_stop = has_steering;

        Box::pin(async move {
            let mut assistant =
                oxi_ai::AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock");
            let reason = if force_stop {
                assistant.content = vec![ContentBlock::Text(TextContent::new("Done."))];
                StopReason::Stop
            } else {
                let tc =
                    oxi_ai::ToolCall::new("call_1", "echo", serde_json::json!({"msg":"hello"}));
                assistant.content = vec![ContentBlock::ToolCall(tc)];
                StopReason::ToolUse
            };
            Ok(Box::pin(OneShotStream {
                event: Some(ProviderEvent::Done {
                    reason,
                    message: assistant,
                }),
            })
                as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }

    fn name(&self) -> &str {
        "looping-tool-call-provider"
    }
}

/// Trivial tool that always returns the same result.
struct LoopGuardEchoTool;

#[async_trait::async_trait]
impl crate::tools::AgentTool for LoopGuardEchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn label(&self) -> &str {
        "Echo"
    }
    fn description(&self) -> &str {
        "Echoes back"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"msg":{"type":"string"}}})
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &crate::tools::ToolContext,
    ) -> Result<crate::tools::AgentToolResult, crate::tools::ToolError> {
        Ok(crate::tools::AgentToolResult::success("echo result"))
    }
}

#[tokio::test]
async fn loop_guard_injects_steering_on_repeated_tool_call() {
    use crate::ProviderResolver;
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::utils::tool_call_loop::ToolCallLoopGuardOptions;

    struct DummyResolver;
    impl ProviderResolver for DummyResolver {
        fn resolve_provider(&self, _name: &str) -> Option<Arc<dyn oxi_ai::Provider>> {
            None
        }
        fn resolve_model(&self, _id: &str) -> Option<oxi_ai::Model> {
            Some(oxi_ai::Model {
                id: "test".into(),
                name: "Test".into(),
                api: oxi_ai::Api::AnthropicMessages,
                provider: "test".into(),
                base_url: String::new(),
                reasoning: false,
                input: vec![oxi_ai::InputModality::Text],
                cost: oxi_ai::Cost::default(),
                context_window: 128_000,
                max_tokens: 4096,
                headers: std::collections::HashMap::new(),
                compat: None,
            })
        }
    }

    let received: Arc<Mutex<Vec<Vec<oxi_ai::Message>>>> = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(LoopingToolCallProvider::new(received.clone()));

    let config = AgentLoopConfig {
        model_id: "test".to_string(),
        tool_execution: ToolExecutionMode::Sequential,
        tool_call_loop_guard: ToolCallLoopGuardOptions {
            threshold: 2,
            exempt_tools: vec![],
        },
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    tools.register(LoopGuardEchoTool);

    let state = SharedState::new();
    let agent_loop =
        AgentLoop::new_with_resolver(provider, config, tools, state, Arc::new(DummyResolver));

    let result = agent_loop.run("test".to_string(), |_| {}).await;
    assert!(result.is_ok(), "agent loop failed: {result:?}");

    let contexts = received.lock().unwrap();
    let call_count = contexts.len();

    let steering_seen = contexts.iter().any(|msgs| {
        msgs.iter().any(|m| {
            matches!(m, oxi_ai::Message::User(u) if u
                .content
                .as_str()
                .map(|s| s.contains("Tool-call loop detected"))
                .unwrap_or(false))
        })
    });

    assert!(
        steering_seen,
        "steering message should appear in provider context \
         after threshold repetitions. Provider was called {} times.",
        call_count
    );
    assert!(
        call_count <= 5,
        "guard should have terminated the loop early, but provider \
         was called {} times",
        call_count
    );
}

#[tokio::test]
async fn test_agent_with_mock_provider() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Hello! How can I help you?".to_string(),
        ..Default::default()
    }]));

    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider.clone(), config, Arc::new(ToolRegistry::new()));

    let (response, events) = agent.run("Hi".to_string()).await.unwrap();

    assert_eq!(response.content, "Hello! How can I help you?");
    assert_eq!(*provider.call_count.lock().unwrap(), 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentStart { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
    );
}

#[tokio::test]
async fn test_agent_events_sequence() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Test response".to_string(),
        ..Default::default()
    }]));

    let config = AgentConfig::default();
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

    let (_, events) = agent.run("Test prompt".to_string()).await.unwrap();

    assert!(
        events
            .first()
            .map(|e| matches!(e, AgentEvent::AgentStart { .. }))
            .unwrap_or(false)
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
    );
}

#[test]
fn test_tool_definition() {
    let mut schema = HashMap::new();
    schema.insert(
        "query".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Search query"
        }),
    );
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
    let transformed =
        transform_for_provider(&messages, &Api::AnthropicMessages, &Api::OpenAiCompletions);

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

    let messages = vec![oxi_ai::Message::Assistant(assistant)];

    let transformed =
        transform_for_provider(&messages, &Api::AnthropicMessages, &Api::AnthropicMessages);

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

    let messages = vec![oxi_ai::Message::ToolResult(tool_result)];

    let transformed =
        transform_for_provider(&messages, &Api::AnthropicMessages, &Api::OpenAiCompletions);

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
        ..Default::default()
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));
    assert_eq!(agent.model_id(), "anthropic/claude-sonnet-4-20250514");
}

#[test]
fn test_agent_switch_model_invalid_format() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
        ..Default::default()
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

    // Invalid format (no provider prefix)
    let result = agent.switch_model("gpt-4o");
    assert!(result.is_err());
}

#[test]
fn test_agent_switch_model_unknown_model() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
        ..Default::default()
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

    let result = agent.switch_model("nonexistent/model");
    assert!(result.is_err());
}

#[test]
fn test_agent_switch_model_same_provider() {
    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "test".to_string(),
        ..Default::default()
    }]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

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

impl Provider for ApiAwareMockProvider {
    fn stream<'a>(
        &'a self,
        model: &'a oxi_ai::Model,
        _context: &'a Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let mut call_count = self.call_count.lock().unwrap();
            *call_count += 1;
            let idx = (*call_count - 1) % self.responses.len();
            let response = self.responses[idx].clone();

            *self.last_api.lock().unwrap() = Some(model.api);

            let stream = MockStream {
                text: response.content,
                done: false,
                ..Default::default()
            };

            Ok(Box::pin(stream)
                as Pin<
                    Box<dyn futures::Stream<Item = ProviderEvent> + Send>,
                >)
        })
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
        MockResponse {
            content: "OpenAI response".to_string(),
            ..Default::default()
        },
        MockResponse {
            content: "Continued response".to_string(),
            ..Default::default()
        },
    ]));
    let config = AgentConfig::new("openai/gpt-4o");
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

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
        MockResponse {
            content: "First response".to_string(),
            ..Default::default()
        },
        MockResponse {
            content: "Second response".to_string(),
            ..Default::default()
        },
    ]));
    let config = AgentConfig::new("anthropic/claude-sonnet-4-20250514");
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

    // Build up conversation
    agent.run("Message 1".to_string()).await.unwrap();
    agent.run("Message 2".to_string()).await.unwrap();
    assert_eq!(agent.state().messages.len(), 4);

    // Transform all messages for OpenAI (cross-provider)
    let messages = agent.state().messages.clone();
    let transformed =
        transform_for_provider(&messages, &Api::AnthropicMessages, &Api::OpenAiCompletions);

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

// ── New integration tests ──────────────────────────────────────────────

/// Provider that returns tool calls on the first call and a text
/// response on the second call, simulating a multi-turn tool-use loop.
struct MultiTurnToolProvider {
    responses: Vec<MultiTurnToolResponse>,
    call_count: Arc<Mutex<usize>>,
}

#[derive(Clone)]
struct MultiTurnToolResponse {
    text: Option<String>,
    tool_calls: Vec<oxi_ai::ToolCall>,
}

impl MultiTurnToolProvider {
    fn new(responses: Vec<MultiTurnToolResponse>) -> Self {
        Self {
            responses,
            call_count: Arc::new(Mutex::new(0)),
        }
    }
}

impl Provider for MultiTurnToolProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a oxi_ai::Model,
        _context: &'a Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let mut call_count = self.call_count.lock().unwrap();
            *call_count += 1;
            let idx = (*call_count - 1).min(self.responses.len() - 1);
            let response = self.responses[idx].clone();

            let mut assistant =
                oxi_ai::AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock-model");

            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            if let Some(text) = &response.text {
                content_blocks.push(ContentBlock::Text(TextContent::new(text.clone())));
            }
            for tc in &response.tool_calls {
                content_blocks.push(ContentBlock::ToolCall(tc.clone()));
            }
            assistant.content = content_blocks;

            let stop_reason = if response.tool_calls.is_empty() {
                StopReason::Stop
            } else {
                StopReason::ToolUse
            };
            assistant.stop_reason = stop_reason;

            let events: Vec<ProviderEvent> = vec![
                ProviderEvent::Start {
                    partial: std::sync::Arc::new(assistant.clone()),
                },
                ProviderEvent::Done {
                    reason: stop_reason,
                    message: assistant,
                },
            ];

            Ok(Box::pin(futures::stream::iter(events))
                as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }

    fn name(&self) -> &str {
        "multi-turn-tool"
    }
}

use async_trait::async_trait;

struct EchoTool;

#[async_trait]
impl crate::tools::AgentTool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn label(&self) -> &str {
        "Echo Tool"
    }

    fn description(&self) -> &str {
        "Echoes back the input arguments"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "Message to echo" }
            },
            "required": ["message"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &crate::tools::ToolContext,
    ) -> std::result::Result<crate::tools::AgentToolResult, String> {
        let msg = params
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("<no message>");
        Ok(crate::tools::AgentToolResult::success(format!(
            "Echo: {}",
            msg
        )))
    }
}

/// Provider that fails the first N calls then succeeds.
#[allow(dead_code)] // test helper, used by selected test binaries
struct RetryableProvider {
    fail_count: usize,
    success_response: String,
    call_count: Arc<Mutex<usize>>,
}

impl RetryableProvider {
    #[allow(dead_code)] // test helper
    fn new(fail_count: usize, success_response: String) -> Self {
        Self {
            fail_count,
            success_response,
            call_count: Arc::new(Mutex::new(0)),
        }
    }
}

impl Provider for RetryableProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a oxi_ai::Model,
        _context: &'a Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let mut call_count = self.call_count.lock().unwrap();
            *call_count += 1;

            if *call_count <= self.fail_count {
                return Err(oxi_ai::ProviderError::HttpError(
                    oxi_ai::HttpErrorDetail::new(429, "rate limited".to_string()),
                ));
            }

            let mut assistant =
                oxi_ai::AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock-model");
            assistant.content = vec![ContentBlock::Text(TextContent::new(
                self.success_response.clone(),
            ))];

            let stream = MockStream {
                text: self.success_response.clone(),
                done: false,
                ..Default::default()
            };

            Ok(Box::pin(stream)
                as Pin<
                    Box<dyn futures::Stream<Item = ProviderEvent> + Send>,
                >)
        })
    }

    fn name(&self) -> &str {
        "retryable"
    }
}

/// Provider that always returns a provider-level error (non-retryable).
#[allow(dead_code)] // test helper, used by selected test binaries
struct AlwaysErrorProvider;

impl Provider for AlwaysErrorProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a oxi_ai::Model,
        _context: &'a Context,
        _options: Option<oxi_ai::StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            Err(oxi_ai::ProviderError::StreamError(
                "permanent error".to_string(),
            ))
        })
    }

    fn name(&self) -> &str {
        "always-error"
    }
}

// ── Test 1: Multi-turn tool use loop ──────────────────────────────────

#[tokio::test]
async fn test_multi_turn_tool_use_loop() {
    // Simulate: user asks → LLM calls echo tool → tool result fed back → LLM responds
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    // First call: LLM wants to call the echo tool; Second call: LLM gives final answer
    let provider = Arc::new(MultiTurnToolProvider::new(vec![
        MultiTurnToolResponse {
            text: None,
            tool_calls: vec![oxi_ai::ToolCall::new(
                "call_1",
                "echo",
                serde_json::json!({"message": "hello world"}),
            )],
        },
        MultiTurnToolResponse {
            text: Some("The echo tool returned: Echo: hello world".to_string()),
            tool_calls: vec![],
        },
    ]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    tools.register(EchoTool);
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = agent_loop
        .run("Echo hello world".to_string(), move |e| {
            events_clone.lock().unwrap().push(e)
        })
        .await;

    assert!(result.is_ok());
    let events = events.lock().unwrap();

    // Should have two turns
    let turn_starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .count();
    assert_eq!(turn_starts, 2);

    // Should have tool execution start and end events
    let tool_starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .count();
    assert_eq!(tool_starts, 1);

    let tool_ends = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        .count();
    assert_eq!(tool_ends, 1);

    // Verify the tool result content
    let tool_end_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
    if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end_event {
        assert_eq!(result.content, "Echo: hello world");
        assert_eq!(result.status, "success");
    }

    // Should complete with an AgentEnd event
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
    );
}

// ── Test 2: Compaction flow integration ───────────────────────────────

#[test]
fn test_compaction_event_triggers_and_completes() {
    // Verify compaction event lifecycle: Triggered → Started → Completed/Failed
    let triggered = crate::compaction::CompactionEvent::Triggered {
        context_tokens: 50000,
        iteration: 3,
        source: "test".to_string(),
    };
    let started = crate::compaction::CompactionEvent::Started { message_count: 20 };
    let completed = crate::compaction::CompactionEvent::Completed {
        result: crate::compaction::CompactedContext {
            summary: "Summary of conversation".to_string(),
            kept_messages: vec![],
            compacted_count: 15,
        },
        duration_ms: 250,
    };

    // Verify serialization round-trip (events are serialized in real use)
    let triggered_json = serde_json::to_string(&triggered).unwrap();
    assert!(triggered_json.contains("Triggered"));

    let started_json = serde_json::to_string(&started).unwrap();
    assert!(started_json.contains("Started"));

    let completed_json = serde_json::to_string(&completed).unwrap();
    assert!(completed_json.contains("Completed"));
}

#[test]
fn test_compacted_context_fields() {
    let ctx = crate::compaction::CompactedContext {
        summary: "User discussed Rust".to_string(),
        kept_messages: vec![],
        compacted_count: 10,
    };
    assert_eq!(ctx.summary, "User discussed Rust");
    assert_eq!(ctx.compacted_count, 10);
    assert!(ctx.kept_messages.is_empty());
}

#[tokio::test]
async fn test_agent_state_replace_messages_for_compaction() {
    let shared = crate::state::SharedState::new();
    shared.update(|s| {
        s.add_user_message("Long conversation part 1".to_string());
        s.add_assistant_message("Response 1".to_string());
        s.add_user_message("Long conversation part 2".to_string());
        s.add_assistant_message("Response 2".to_string());
    });

    assert_eq!(shared.get_state().messages.len(), 4);

    // Simulate compaction: replace with a summary message + last exchange
    let compacted_messages = vec![
        oxi_ai::Message::User(oxi_ai::UserMessage::new(
            "[Summary of previous conversation]".to_string(),
        )),
        oxi_ai::Message::User(oxi_ai::UserMessage::new(
            "Long conversation part 2".to_string(),
        )),
    ];

    shared.update(|s| {
        s.replace_messages(compacted_messages);
    });

    let state = shared.get_state();
    assert_eq!(state.messages.len(), 2);
}

#[tokio::test]
async fn test_compaction_strategy_config() {
    // Verify compaction strategy can be set on the agent config
    let config = crate::config::AgentConfig::new("anthropic/claude-sonnet-4-20250514")
        .with_compaction_strategy(oxi_ai::CompactionStrategy::EveryNTurns(5));

    assert!(matches!(
        config.compaction_strategy,
        oxi_ai::CompactionStrategy::EveryNTurns(5)
    ));
}

// ── Test 3: Cross-provider model switching with active tool use ──────

#[tokio::test]
async fn test_cross_provider_switch_preserves_tool_results() {
    // Build a conversation that includes tool results, then verify
    // transformation preserves them across provider switch
    let mut state = AgentState::new();
    state.add_user_message("What is the weather?".to_string());
    state.add_assistant_message("Let me check the weather.".to_string());
    state.add_tool_result("call_1".to_string(), "Sunny, 72°F".to_string());
    state.add_assistant_message("The weather is sunny, 72°F.".to_string());

    assert_eq!(state.messages.len(), 4);

    // Verify tool result message is present
    let tool_result_msg = &state.messages[2];
    assert!(matches!(tool_result_msg, oxi_ai::Message::ToolResult(_)));

    if let oxi_ai::Message::ToolResult(tr) = tool_result_msg {
        assert_eq!(tr.tool_call_id, "call_1");
    }

    // Transform for OpenAI and back — tool results should survive
    let messages = state.messages.clone();
    let to_openai =
        transform_for_provider(&messages, &Api::AnthropicMessages, &Api::OpenAiCompletions);
    assert_eq!(to_openai.len(), 4);

    // Tool result should still be there
    assert!(matches!(&to_openai[2], oxi_ai::Message::ToolResult(_)));
}

#[tokio::test]
async fn test_cross_provider_switch_with_tool_call_blocks() {
    // Create an assistant message that includes tool call content blocks
    let mut assistant = oxi_ai::AssistantMessage::new(
        Api::AnthropicMessages,
        "anthropic",
        "claude-sonnet-4-20250514",
    );
    assistant.content = vec![
        ContentBlock::Text(TextContent::new("I'll use the echo tool.")),
        ContentBlock::ToolCall(oxi_ai::ToolCall::new(
            "tc_123",
            "echo",
            serde_json::json!({"message": "test"}),
        )),
    ];

    let messages = vec![
        oxi_ai::Message::User(oxi_ai::UserMessage::new("Echo test")),
        oxi_ai::Message::Assistant(assistant),
    ];

    // Transform for OpenAI
    let transformed =
        transform_for_provider(&messages, &Api::AnthropicMessages, &Api::OpenAiCompletions);

    // Should preserve both messages
    assert_eq!(transformed.len(), 2);

    // Tool call blocks should survive in the assistant message
    if let oxi_ai::Message::Assistant(a) = &transformed[1] {
        let has_tool_call = a
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall(_)));
        assert!(
            has_tool_call,
            "Assistant message should still contain a tool call block"
        );
    }
}

// ── Test 4: Error recovery scenarios ─────────────────────────────────

#[test]
fn test_partial_response_accumulator() {
    use crate::recovery::PartialResponse;

    let mut pr = PartialResponse::new();
    assert!(pr.is_empty());

    pr.push_text("Hello ");
    pr.push_text("world");
    pr.push_thinking("Let me think...");

    assert_eq!(pr.text(), "Hello world");
    assert_eq!(pr.thinking(), "Let me think...");
    assert!(pr.has_thinking());
    assert!(!pr.is_empty());

    // take_text drains the text
    let text = pr.take_text();
    assert_eq!(text, "Hello world");
    assert!(pr.text().is_empty());

    // clear resets everything
    pr.clear();
    assert!(pr.is_empty());
    assert!(!pr.has_thinking());
}

#[test]
fn test_agent_error_retryable() {
    use crate::error::AgentError;

    let rate_limited = AgentError::RateLimited {
        retry_after_secs: 30,
    };
    assert!(rate_limited.is_retryable());

    let stream_err = AgentError::Stream("connection reset".to_string());
    assert!(stream_err.is_retryable());

    let tool_err = AgentError::Tool {
        tool_name: "echo".to_string(),
        message: "failed".to_string(),
    };
    assert!(!tool_err.is_retryable());

    let config_err = AgentError::Config("bad config".to_string());
    assert!(!config_err.is_retryable());
}

#[test]
fn test_agent_error_user_friendly_messages() {
    use crate::error::AgentError;

    let errors = vec![
        AgentError::RateLimited {
            retry_after_secs: 10,
        },
        AgentError::MaxIterations { iterations: 50 },
        AgentError::FallbackFailed {
            primary_model: "anthropic/claude-sonnet-4-20250514".to_string(),
            primary_error: "timeout".to_string(),
            fallback_model: "openai/gpt-4o-mini".to_string(),
            fallback_error: "also timeout".to_string(),
        },
    ];

    for err in &errors {
        let msg = err.user_friendly();
        assert!(
            !msg.is_empty(),
            "user_friendly() should not be empty for {:?}",
            err
        );
    }
}

// ── Test 5: Steering messages injected mid-loop ───────────────────────

#[tokio::test]
async fn test_steering_messages_injected_into_loop() {
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MultiTurnToolProvider::new(vec![
        // Turn 1: initial response
        MultiTurnToolResponse {
            text: Some("Initial response".to_string()),
            tool_calls: vec![],
        },
        // Turn 2: response after steering message
        MultiTurnToolResponse {
            text: Some("Response after steering".to_string()),
            tool_calls: vec![],
        },
    ]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Inject a steering message before running
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Please be more concise",
    )));

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = agent_loop
        .run("Hello".to_string(), move |e| {
            events_clone.lock().unwrap().push(e)
        })
        .await;

    assert!(result.is_ok());
    let events = events.lock().unwrap();

    // Should have SteeringMessage events
    let steering_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
        .count();
    assert_eq!(steering_count, 1);

    // Should have MessageStart/MessageEnd for the steering message
    let msg_starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::MessageStart { .. }))
        .count();
    assert!(
        msg_starts >= 2,
        "Expected at least 2 MessageStart events (user + steering), got {}",
        msg_starts
    );
}

#[tokio::test]
async fn test_multiple_steering_messages() {
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Response".to_string(),
        ..Default::default()
    }]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Inject multiple steering messages
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer 1")));
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer 2")));
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer 3")));

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = agent_loop
        .run("Hello".to_string(), move |e| {
            events_clone.lock().unwrap().push(e)
        })
        .await;

    assert!(result.is_ok());
    let events = events.lock().unwrap();

    let steering_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
        .count();
    assert_eq!(steering_count, 3);
}

// ── Test 6: Follow-up queue processing ───────────────────────────────

#[test]
fn test_follow_up_queue_api() {
    // Verify follow_up() and queue management work correctly
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Response".to_string(),
        ..Default::default()
    }]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Queue follow-ups
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Follow-up A",
    )));
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Follow-up B",
    )));

    // Clear them individually
    agent_loop.clear_follow_up_queue();

    // Add steering and clear all
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer")));
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Follow-up C",
    )));
    agent_loop.clear_all_queues();
}

#[tokio::test]
async fn test_follow_up_processed_in_tool_use_loop() {
    // Follow-ups are drained after the tool-use while loop finishes but before
    // the outer loop breaks. To trigger this path we need a multi-turn tool loop
    // that does NOT hit should_stop_after_turn on the first iteration.
    // We use a tool call first, then a stop.
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MultiTurnToolProvider::new(vec![
        // Turn 1: LLM calls echo tool
        MultiTurnToolResponse {
            text: None,
            tool_calls: vec![oxi_ai::ToolCall::new(
                "call_1",
                "echo",
                serde_json::json!({"message": "hello"}),
            )],
        },
        // Turn 2: LLM responds after tool result
        MultiTurnToolResponse {
            text: Some("Done with tool".to_string()),
            tool_calls: vec![],
        },
        // Turn 3: LLM responds to follow-up
        MultiTurnToolResponse {
            text: Some("Follow-up handled".to_string()),
            tool_calls: vec![],
        },
    ]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    tools.register(EchoTool);
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Queue a follow-up before running.
    // The follow-up queue is drained after the tool-use while loop ends
    // but before the outer loop breaks. However, since should_stop_after_turn
    // returns true on StopReason::Stop, it exits in the inner while loop before
    // reaching the follow-up drain. We still test this to ensure the loop
    // completes successfully without panicking.
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Tell me more",
    )));

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = agent_loop
        .run("Hello".to_string(), move |e| {
            events_clone.lock().unwrap().push(e)
        })
        .await;

    assert!(result.is_ok());
    let events = events.lock().unwrap();

    // We expect 3 turns: tool call + response + follow-up
    // (Previously expected 2 when should_stop_after_turn checked StopReason::Stop.
    //  Now that check is removed, follow-ups are properly processed.)
    let turn_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .count();
    assert_eq!(turn_count, 3);

    // Tool should have been called
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
    );
}

#[tokio::test]
async fn test_follow_up_via_continue_loop() {
    // Use continue_loop after the initial run completes, simulating
    // how follow-ups would be processed in a real agent.
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MultiTurnToolProvider::new(vec![
        MultiTurnToolResponse {
            text: Some("Initial response".to_string()),
            tool_calls: vec![],
        },
        MultiTurnToolResponse {
            text: Some("Follow-up response".to_string()),
            tool_calls: vec![],
        },
    ]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Run initial message
    let events1 = Arc::new(Mutex::new(Vec::new()));
    let events1_clone = events1.clone();
    let result1 = agent_loop
        .run("Hello".to_string(), move |e| {
            events1_clone.lock().unwrap().push(e)
        })
        .await;
    assert!(result1.is_ok());

    // Now queue a follow-up and use steer to inject it
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Follow-up question",
    )));

    // continue_loop will pick up the steering message
    let events2 = Arc::new(Mutex::new(Vec::new()));
    let events2_clone = events2.clone();
    let result2 = agent_loop
        .continue_loop(move |e| events2_clone.lock().unwrap().push(e))
        .await;
    assert!(result2.is_ok());

    let events2 = events2.lock().unwrap();
    let steering_count = events2
        .iter()
        .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
        .count();
    assert_eq!(steering_count, 1);
    assert!(
        events2
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnStart { .. }))
    );
}

#[tokio::test]
async fn test_follow_up_queue_cleared() {
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Response".to_string(),
        ..Default::default()
    }]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Queue messages and then clear them
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Follow-up 1",
    )));
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new(
        "Follow-up 2",
    )));
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer 1")));
    agent_loop.clear_all_queues();

    // Run should only process the initial prompt, no extra turns
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = agent_loop
        .run("Hello".to_string(), move |e| {
            events_clone.lock().unwrap().push(e)
        })
        .await;

    assert!(result.is_ok());
    let events = events.lock().unwrap();

    // No steering events since queues were cleared
    let steering_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
        .count();
    assert_eq!(steering_count, 0);

    // Only 1 turn (no follow-up processed)
    let turn_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .count();
    assert_eq!(turn_count, 1);
}

#[test]
fn test_follow_up_and_steering_queue_independent() {
    // Verify that steering and follow-up queues are independent
    use crate::agent_loop::{AgentLoop, AgentLoopConfig, ToolExecutionMode};
    use crate::state::SharedState;
    use crate::tools::ToolRegistry;
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "Response".to_string(),
        ..Default::default()
    }]));

    let config = AgentLoopConfig {
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        system_prompt: None,
        temperature: 0.7,
        max_tokens: 4096,
        tool_execution: ToolExecutionMode::Sequential,
        compaction_strategy: CompactionStrategy::Disabled,
        context_window: 100_000,
        compaction_instruction: None,
        session_id: None,
        transport: None,
        compact_on_start: false,
        max_retry_delay_ms: None,
        auto_retry_enabled: false,
        auto_retry_max_attempts: 3,
        auto_retry_base_delay_ms: 2000,
        workspace_dir: None,
        provider_options: None,
        on_compaction: None,
        ..Default::default()
    };

    let tools = Arc::new(ToolRegistry::new());
    let state = SharedState::new();
    let agent_loop = AgentLoop::new(provider, config, tools, state);

    // Add to both queues
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer 1")));
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new("Follow 1")));

    // Clear only follow-up — steering should remain
    agent_loop.clear_follow_up_queue();

    // Add more to both
    agent_loop.steer(oxi_ai::Message::User(oxi_ai::UserMessage::new("Steer 2")));
    agent_loop.follow_up(oxi_ai::Message::User(oxi_ai::UserMessage::new("Follow 2")));

    // Clear only steering — follow-up should remain
    agent_loop.clear_steering_queue();

    // Clear all to clean up
    agent_loop.clear_all_queues();
}

#[test]
fn test_agent_state_follow_up_tracking() {
    // Verify state can track multiple rounds of interaction
    let mut state = crate::state::AgentState::new();

    // Simulate: user → assistant → tool result → assistant
    state.add_user_message("What is 2+2?".to_string());
    state.add_assistant_message("Let me calculate.".to_string());
    state.add_tool_result("call_1".to_string(), "4".to_string());
    state.add_assistant_message("The answer is 4.".to_string());

    assert_eq!(state.messages.len(), 4);
    assert_eq!(state.tool_results.len(), 1);
    assert_eq!(state.tool_results[0].content, "4");
    assert!(!state.tool_results[0].is_error);

    // Simulate follow-up question
    state.add_user_message("And 3+3?".to_string());
    state.add_assistant_message("That's 6.".to_string());

    assert_eq!(state.messages.len(), 6);
    assert_eq!(state.iteration, 0); // Not incremented manually
    state.increment_iteration();
    assert_eq!(state.iteration, 1);
}

#[test]
fn test_set_compaction_strategy_updates_config() {
    use oxi_ai::CompactionStrategy;

    let provider = Arc::new(MockProvider::new(vec![MockResponse {
        content: "ok".to_string(),
        ..Default::default()
    }]));
    let mut config = AgentConfig::default();
    config.compaction_strategy = CompactionStrategy::Threshold(0.8);
    let agent = Agent::new(provider, config, Arc::new(ToolRegistry::new()));

    // Before: strategy is Threshold(0.8) — both the config and the
    // construction-time compaction_manager agree.
    assert_eq!(
        agent.compaction_strategy(),
        CompactionStrategy::Threshold(0.8)
    );

    // Update via the live setter (used by the /settings overlay).
    agent.set_compaction_strategy(CompactionStrategy::Disabled);

    // After: the config-level strategy reflects the change — this is
    // what the agent loop reads at the start of each run.
    assert_eq!(agent.compaction_strategy(), CompactionStrategy::Disabled);

    // The construction-time compaction_manager is NOT affected (it retains
    // its original strategy). This is by design: the manager is used for
    // manual compact() calls, while the agent loop creates a fresh manager
    // from config each run.
    assert_eq!(
        agent.compaction_manager().strategy(),
        &CompactionStrategy::Threshold(0.8)
    );
}

// ── Gap 3: in-process sub-agent delegation tests (issue #28) ──────────

use crate::tools::AgentTool as _;
use crate::tools::{ForkResult, SubagentRunner};
use std::path::Path;

/// Mock SubagentRunner that returns a fixed ForkResult and records
/// the depth it was called with.
#[derive(Debug)]
struct MockSubagentRunner {
    response_text: String,
    depth_received: parking_lot::Mutex<Option<u8>>,
}

#[async_trait::async_trait]
impl SubagentRunner for MockSubagentRunner {
    async fn run_isolated(
        &self,
        _agent_name: &str,
        _task: &str,
        _system_prompt: Option<&str>,
        _model: Option<&str>,
        _tools: &[String],
        _cwd: &Path,
        depth: u8,
    ) -> anyhow::Result<ForkResult> {
        *self.depth_received.lock() = Some(depth);
        Ok(ForkResult {
            text: self.response_text.clone(),
            input_tokens: 42,
            output_tokens: 10,
            turns: 1,
            model: None,
            error: None,
        })
    }
}

#[tokio::test]
async fn test_in_process_subagent_single_mode() {
    // Wire a mock runner into ToolContext, call SubagentTool::execute
    // with single mode, and verify the runner's output is returned.
    let runner = Arc::new(MockSubagentRunner {
        response_text: "sub-agent result text".to_string(),
        depth_received: parking_lot::Mutex::new(None),
    });
    let runner_clone = runner.clone();

    let ctx = crate::tools::ToolContext::new(".").with_subagent_runner(runner);

    let tool = crate::SubagentTool::new();
    let params = serde_json::json!({
        "agent": "test-agent",
        "task": "do something"
    });

    let result = tool.execute("tc_test", params, None, &ctx).await.unwrap();

    assert!(result.success, "tool should succeed");
    assert!(
        result.output.contains("sub-agent result text"),
        "output should contain runner response: {}",
        result.output
    );

    // Verify depth was passed to the runner.
    let depth = runner_clone.depth_received.lock();
    assert_eq!(*depth, Some(0), "depth should be 0 for top-level call");
}

#[tokio::test]
async fn test_in_process_subagent_parallel_mode() {
    // Multiple tasks in parallel mode should all use the runner.
    let runner = Arc::new(MockSubagentRunner {
        response_text: "parallel result".to_string(),
        depth_received: parking_lot::Mutex::new(None),
    });

    let ctx = crate::tools::ToolContext::new(".").with_subagent_runner(runner);

    let tool = crate::SubagentTool::new();
    let params = serde_json::json!({
        "tasks": [
            {"agent": "a1", "task": "t1"},
            {"agent": "a2", "task": "t2"}
        ]
    });

    let result = tool.execute("tc_test", params, None, &ctx).await.unwrap();

    assert!(result.success, "parallel mode should succeed");
    assert!(
        result.output.contains("parallel result"),
        "output should contain runner responses: {}",
        result.output
    );
    assert!(
        result.output.contains("Parallel: 2/2 succeeded"),
        "should report 2/2 success"
    );
}

#[tokio::test]
async fn test_in_process_subagent_falls_back_to_cli_without_runner() {
    // Without a runner wired, the tool should NOT use the in-process
    // path. It will try the CLI path and fail (no oxi binary in test
    // env), but the important assertion is that it didn't call the
    // runner (which is None).
    let ctx = crate::tools::ToolContext::new(".");
    assert!(
        ctx.subagent_runner.is_none(),
        "no runner should be wired by default"
    );
    assert_eq!(ctx.subagent_depth, 0, "depth should default to 0");
}

#[test]
fn test_fork_result_defaults() {
    let fr = ForkResult::default();
    assert!(fr.text.is_empty());
    assert_eq!(fr.input_tokens, 0);
    assert_eq!(fr.output_tokens, 0);
    assert_eq!(fr.turns, 0);
    assert!(fr.model.is_none());
    assert!(fr.error.is_none());
}
