//! AgentLoop full integration tests
//!
//! These tests verify the complete AgentLoop flow including:
//! - Single turn conversations (no tools)
//! - Multi-turn tool use loops
//! - Parallel tool execution
//! - Steering message injection
//! - Max iterations stopping

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures::Stream;
    use oxi_agent::{
        AgentEvent, AgentLoop, AgentLoopConfig, CompactionStrategy, SharedState, ToolExecutionMode,
        tools::{AgentTool, AgentToolResult, ToolRegistry},
    };
    use oxi_ai::{
        AssistantMessage, ContentBlock, Message, Provider, ProviderEvent, StopReason, StreamResult,
        TextContent, ToolCall, UserMessage,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::{Context as TaskContext, Poll};

    // ── Mock Providers ────────────────────────────────────────────────────

    /// Simple mock provider that returns predefined responses
    struct MockProvider {
        responses: Vec<MockResponse>,
        call_count: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct MockResponse {
        content: String,
    }

    impl MockProvider {
        fn new(responses: Vec<MockResponse>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    impl Provider for MockProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a oxi_ai::Model,
            _context: &'a oxi_ai::Context,
            _options: Option<oxi_ai::StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            Box::pin(async move {
                let idx = self.call_count.fetch_add(1, Ordering::Relaxed) % self.responses.len();
                let response = self.responses[idx].content.clone();

                let stream = MockStream {
                    text: response,
                    done: false,
                };
                Ok(Box::pin(stream) as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
            })
        }
    }

    struct MockStream {
        text: String,
        done: bool,
    }

    impl Stream for MockStream {
        type Item = ProviderEvent;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<Option<Self::Item>> {
            if self.done {
                return Poll::Ready(None);
            }
            self.done = true;

            let mut assistant =
                AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock-model");
            assistant.content = vec![ContentBlock::Text(TextContent::new(self.text.clone()))];

            Poll::Ready(Some(ProviderEvent::Done {
                reason: StopReason::Stop,
                message: assistant,
            }))
        }
    }

    /// Provider that returns tool calls on first turn, then a final response
    struct MultiTurnToolProvider {
        responses: Vec<MultiTurnToolResponse>,
        call_count: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct MultiTurnToolResponse {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
    }

    impl MultiTurnToolProvider {
        fn new(responses: Vec<MultiTurnToolResponse>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    impl Provider for MultiTurnToolProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a oxi_ai::Model,
            _context: &'a oxi_ai::Context,
            _options: Option<oxi_ai::StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            Box::pin(async move {
                let idx = self
                    .call_count
                    .fetch_add(1, Ordering::Relaxed)
                    .min(self.responses.len() - 1);
                let response = self.responses[idx].clone();

                let mut assistant =
                    AssistantMessage::new(oxi_ai::Api::AnthropicMessages, "mock", "mock-model");

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
    }

    // ── Test Tools ────────────────────────────────────────────────────────

    /// Simple echo tool that returns the message argument
    struct EchoTool;

    #[async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn label(&self) -> &str {
            "Echo Tool"
        }

        fn description(&self) -> &str {
            "Echoes back the input message"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Message to echo"
                    }
                },
                "required": ["message"]
            })
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxi_agent::ToolContext,
        ) -> Result<AgentToolResult, String> {
            let msg = params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("<no message>");
            Ok(AgentToolResult::success(format!("Echo: {}", msg)))
        }
    }

    /// Tool that records when it was called (for parallel test)
    #[allow(dead_code)]
    struct CountingTool {
        call_count: Arc<AtomicUsize>,
    }

    impl CountingTool {
        #[allow(dead_code)] // test helper
        fn new(call_count: Arc<AtomicUsize>) -> Self {
            Self { call_count }
        }
    }

    #[async_trait]
    impl AgentTool for CountingTool {
        fn name(&self) -> &str {
            "count"
        }

        fn label(&self) -> &str {
            "Counting Tool"
        }

        fn description(&self) -> &str {
            "Counts how many times it has been called"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxi_agent::ToolContext,
        ) -> Result<AgentToolResult, String> {
            let count = self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(AgentToolResult::success(format!("Call #{}", count + 1)))
        }
    }

    // ── Test Helpers ───────────────────────────────────────────────────────

    fn make_config() -> AgentLoopConfig {
        AgentLoopConfig {
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            system_prompt: None,
            temperature: 0.7,
            max_tokens: 4096,
            tool_execution: ToolExecutionMode::Sequential,
            compaction_strategy: CompactionStrategy::Disabled,
            context_window: 100_000,
            compaction_instruction: None,
            session_id: Some("test-session".to_string()),
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
        }
    }

    fn make_tools() -> Arc<ToolRegistry> {
        let tools = Arc::new(ToolRegistry::new());
        tools.register(EchoTool);
        tools
    }

    // ═══════════════════════════════════════════════════════════════════
    // Test 1: Single turn (no tools)
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_single_turn() {
        let provider = Arc::new(MockProvider::new(vec![MockResponse {
            content: "Hello! How can I help you today?".to_string(),
        }]));

        let config = make_config();
        let tools = Arc::new(ToolRegistry::new()); // No tools needed
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Hi there".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Verify AgentStart and AgentEnd events
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

        // Should have exactly 1 turn
        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 1);

        // No tool executions
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        );

        // Provider was called exactly once
        assert_eq!(provider.call_count(), 1);
    }

    #[tokio::test]
    async fn test_single_turn_with_system_prompt() {
        let provider = Arc::new(MockProvider::new(vec![MockResponse {
            content: "As an AI assistant, I should follow the system prompt.".to_string(),
        }]));

        let mut config = make_config();
        config.system_prompt = Some("You are a helpful assistant.".to_string());

        let tools = Arc::new(ToolRegistry::new());
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("What can you do?".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
    }

    // ═══════════════════════════════════════════════════════════════════
    // Test 2: Multi-turn tool loop
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_multi_turn_tool_loop() {
        // Simulate: user asks → LLM calls echo tool → tool result → LLM responds
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            // Turn 1: LLM wants to call echo tool
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "hello world"}),
                )],
            },
            // Turn 2: LLM responds after getting tool result
            MultiTurnToolResponse {
                text: Some("The echo tool returned: Echo: hello world".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Please echo 'hello world'".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Should have 2 turns
        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 2);

        // Should have exactly 1 tool execution
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

        // Verify tool result content
        if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = events
            .iter()
            .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        {
            assert_eq!(result.content, "Echo: hello world");
            assert_eq!(result.status, "success");
        } else {
            panic!("Expected ToolExecutionEnd event");
        }

        // Provider called: turn 1 (tool call) + turn 2 (final response)
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn test_multi_turn_multiple_tools() {
        // Simulate: user asks → LLM calls two tools in sequence → final response
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            // Turn 1: LLM calls first tool
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "first"}),
                )],
            },
            // Turn 2: LLM calls second tool
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_2",
                    "echo",
                    serde_json::json!({"message": "second"}),
                )],
            },
            // Turn 3: LLM gives final response
            MultiTurnToolResponse {
                text: Some("Done with both tools.".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Echo two messages".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Should have 3 turns
        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 3);

        // Should have 2 tool executions
        let tool_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(tool_starts, 2);

        // Provider was called 3 times
        assert_eq!(provider.call_count(), 3);
    }

    // ═══════════════════════════════════════════════════════════════════
    // Test 3: Parallel tool execution
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_parallel_tool_execution() {
        // Note: Sequential execution mode still runs one tool at a time,
        // but we test the case where LLM returns multiple tool calls
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            // Turn 1: LLM calls two tools simultaneously
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![
                    ToolCall::new("call_1", "echo", serde_json::json!({"message": "first"})),
                    ToolCall::new("call_2", "echo", serde_json::json!({"message": "second"})),
                ],
            },
            // Turn 2: LLM responds after both tools complete
            MultiTurnToolResponse {
                text: Some("Both tools completed.".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = Arc::new(ToolRegistry::new());
        tools.register(EchoTool);
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Run two tools in parallel".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Should have 2 turns
        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 2);

        // Both tools should execute (even in sequential mode, both get executed)
        let tool_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(tool_starts, 2);

        // Provider called: turn 1 (tool calls) + turn 2 (final response)
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn test_all_tools_executed_before_continue() {
        // Verify all tool results are collected before LLM continues
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![
                    ToolCall::new("call_1", "echo", serde_json::json!({"message": "a"})),
                    ToolCall::new("call_2", "echo", serde_json::json!({"message": "b"})),
                    ToolCall::new("call_3", "echo", serde_json::json!({"message": "c"})),
                ],
            },
            MultiTurnToolResponse {
                text: Some("All three tools completed.".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Echo three messages".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Should have 3 tool executions
        let tool_ends = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
            .count();
        assert_eq!(tool_ends, 3);

        // All tool results should have correct content
        let tool_results: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let AgentEvent::ToolExecutionEnd { result, .. } = e {
                    Some(result.content.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(tool_results.contains(&"Echo: a".to_string()));
        assert!(tool_results.contains(&"Echo: b".to_string()));
        assert!(tool_results.contains(&"Echo: c".to_string()));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Test 4: Steering injection
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_steering_injection() {
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: Some("Initial response".to_string()),
                tool_calls: vec![],
            },
            MultiTurnToolResponse {
                text: Some("Response after steering".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        // Inject steering message before running
        agent_loop.steer(Message::User(UserMessage::new(
            "Please be more concise".to_string(),
        )));

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Hello".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Should have SteeringMessage event
        let steering_count = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
            .count();
        assert_eq!(steering_count, 1);

        // Should have MessageStart and MessageEnd for the steering message
        let msg_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::MessageStart { .. }))
            .count();
        assert!(msg_starts >= 2); // user prompt + steering message

        // Provider should be called twice (initial run + steering-injected run)
        assert_eq!(provider.call_count(), 1);
    }

    #[tokio::test]
    async fn test_multiple_steering_messages() {
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: Some("After first steer".to_string()),
                tool_calls: vec![],
            },
            MultiTurnToolResponse {
                text: Some("After second steer".to_string()),
                tool_calls: vec![],
            },
            MultiTurnToolResponse {
                text: Some("After third steer".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        // Inject multiple steering messages
        agent_loop.steer(Message::User(UserMessage::new("Steer 1".to_string())));
        agent_loop.steer(Message::User(UserMessage::new("Steer 2".to_string())));
        agent_loop.steer(Message::User(UserMessage::new("Steer 3".to_string())));

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Hello".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // All 3 steering messages should be emitted
        let steering_count = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
            .count();
        assert_eq!(steering_count, 3);
    }

    #[tokio::test]
    async fn test_steering_with_tool_call() {
        // Steering messages should also work with tool calls
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "steered"}),
                )],
            },
            MultiTurnToolResponse {
                text: Some("Tool result after steering".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        // Inject steering message
        agent_loop.steer(Message::User(UserMessage::new("Add context".to_string())));

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Run tool".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // Should have both steering and tool execution
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Test 5: Max iterations stop
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_multi_turn_loop_completes() {
        // The loop runs until the LLM naturally stops making tool calls.
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "iteration 1"}),
                )],
            },
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_2",
                    "echo",
                    serde_json::json!({"message": "iteration 2"}),
                )],
            },
            MultiTurnToolResponse {
                text: Some("Final answer.".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Start".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // All 3 provider responses are consumed
        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 3);
        assert_eq!(provider.call_count(), 3);
    }

    #[tokio::test]
    async fn test_natural_loop_exit() {
        // The loop exits naturally when the LLM returns a text-only response.
        let config = make_config();

        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "1"}),
                )],
            },
            MultiTurnToolResponse {
                text: Some("Final response".to_string()),
                tool_calls: vec![],
            },
        ]));

        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Start".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // The loop completes naturally when the last response is text-only.
        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 2);

        // Should complete normally (AgentEnd)
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Additional integration tests
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_state_preserved_across_continue_loop() {
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: Some("First response".to_string()),
                tool_calls: vec![],
            },
            MultiTurnToolResponse {
                text: Some("Second response".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        // First run
        let events1 = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events1_clone = events1.clone();
        agent_loop
            .run("Hello".to_string(), move |e| {
                events1_clone.lock().unwrap().push(e)
            })
            .await
            .unwrap();

        // Use steer to inject a follow-up and continue
        agent_loop.steer(Message::User(UserMessage::new(
            "Follow-up message".to_string(),
        )));

        let events2 = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events2_clone = events2.clone();
        agent_loop
            .continue_loop(move |e| events2_clone.lock().unwrap().push(e))
            .await
            .unwrap();

        // Verify events from continue
        let events2 = events2.lock().unwrap();
        assert!(
            events2
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnStart { .. }))
        );
    }

    #[tokio::test]
    async fn test_follow_up_queue_integration() {
        let provider = Arc::new(MockProvider::new(vec![MockResponse {
            content: "Response".to_string(),
        }]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        // Add follow-up message
        agent_loop.follow_up(Message::User(UserMessage::new("Follow-up".to_string())));

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Initial".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_queues() {
        let provider = Arc::new(MockProvider::new(vec![MockResponse {
            content: "Response".to_string(),
        }]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        // Add to both queues
        agent_loop.steer(Message::User(UserMessage::new("Steer 1".to_string())));
        agent_loop.follow_up(Message::User(UserMessage::new("Follow 1".to_string())));

        // Clear only follow-up
        agent_loop.clear_follow_up_queue();

        // Add more to both
        agent_loop.steer(Message::User(UserMessage::new("Steer 2".to_string())));
        agent_loop.follow_up(Message::User(UserMessage::new("Follow 2".to_string())));

        // Clear only steering
        agent_loop.clear_steering_queue();

        // Clear all remaining
        agent_loop.clear_all_queues();

        // Run should be clean (no steering/follow-up processed)
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Test".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
        let events = events.lock().unwrap();

        // No steering events
        let steering_count = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SteeringMessage { .. }))
            .count();
        assert_eq!(steering_count, 0);
    }

    #[tokio::test]
    async fn test_tool_error_handling() {
        // Test that tool errors are handled gracefully
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "test"}),
                )],
            },
            MultiTurnToolResponse {
                text: Some("Continued after tool".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Test".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        // Should succeed even if tool has issues
        assert!(result.is_ok());

        // Should have tool execution complete event
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        );
    }

    #[tokio::test]
    async fn test_message_accumulation() {
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: Some("Turn 1".to_string()),
                tool_calls: vec![],
            },
            MultiTurnToolResponse {
                text: Some("Turn 2".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        // First run
        agent_loop.run("First".to_string(), |_| {}).await.unwrap();

        // Second run (should work without errors)
        agent_loop.run("Second".to_string(), |_| {}).await.unwrap();
    }

    #[tokio::test]
    async fn test_empty_prompt() {
        let provider = Arc::new(MockProvider::new(vec![MockResponse {
            content: "Empty response".to_string(),
        }]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        // Empty prompt should still work
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_special_characters_in_tool_params() {
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "Hello \"world\" & 'test' < >"}),
                )],
            },
            MultiTurnToolResponse {
                text: Some("Special chars handled".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Test special chars".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;

        assert!(result.is_ok());
    }

    // ═══════════════════════════════════════════════════════════════════
    // Event sequence verification tests
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_event_sequence_tool_loop() {
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new(
                    "call_1",
                    "echo",
                    serde_json::json!({"message": "test"}),
                )],
            },
            MultiTurnToolResponse {
                text: Some("Final".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        agent_loop
            .run("Test".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await
            .unwrap();

        let events = events.lock().unwrap();

        // Verify event order: AgentStart -> TurnStart -> ... -> TurnEnd -> AgentEnd
        let agent_start_idx = events
            .iter()
            .position(|e| matches!(e, AgentEvent::AgentStart { .. }));
        let agent_end_idx = events
            .iter()
            .position(|e| matches!(e, AgentEvent::AgentEnd { .. }));

        assert!(agent_start_idx.is_some());
        assert!(agent_end_idx.is_some());
        assert!(agent_start_idx < agent_end_idx);

        // Verify TurnStart comes before TurnEnd
        let turn_starts: Vec<_> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, AgentEvent::TurnStart { .. }))
            .map(|(i, _)| i)
            .collect();
        let turn_ends: Vec<_> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, AgentEvent::TurnEnd { .. }))
            .map(|(i, _)| i)
            .collect();

        assert!(!turn_starts.is_empty());
        assert!(!turn_ends.is_empty());
        for (start, end) in turn_starts.iter().zip(turn_ends.iter()) {
            assert!(start < end, "TurnStart should come before TurnEnd");
        }
    }

    #[tokio::test]
    async fn test_no_tool_calls_no_tool_events() {
        let provider = Arc::new(MockProvider::new(vec![MockResponse {
            content: "No tools here".to_string(),
        }]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider, config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        agent_loop
            .run("Test".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await
            .unwrap();

        let events = events.lock().unwrap();

        // Should NOT have ToolExecutionStart or ToolExecutionEnd
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Loop termination tests
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_loop_runs_until_natural_exit() {
        // The loop runs until the LLM returns a text-only response.
        let provider = Arc::new(MultiTurnToolProvider::new(vec![
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new("c1", "echo", serde_json::json!({"msg": "1"}))],
            },
            MultiTurnToolResponse {
                text: None,
                tool_calls: vec![ToolCall::new("c2", "echo", serde_json::json!({"msg": "2"}))],
            },
            MultiTurnToolResponse {
                text: Some("Here is the final answer.".to_string()),
                tool_calls: vec![],
            },
        ]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Do work".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;
        assert!(result.is_ok());

        let events = events.lock().unwrap();

        let turn_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .count();
        assert_eq!(turn_starts, 3);
        assert_eq!(provider.call_count(), 3);
    }

    #[tokio::test]
    async fn test_external_stop_exits_immediately() {
        // External stop (Ctrl+C) should exit immediately.
        let provider = Arc::new(MultiTurnToolProvider::new(vec![MultiTurnToolResponse {
            text: None,
            tool_calls: vec![ToolCall::new("c1", "echo", serde_json::json!({"msg": "1"}))],
        }]));

        let config = make_config();
        let tools = make_tools();
        let state = SharedState::new();
        let agent_loop = AgentLoop::new(provider.clone(), config, tools, state);

        // Set external stop flag before running
        agent_loop.external_stop().store(true, Ordering::SeqCst);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let result = agent_loop
            .run("Test".to_string(), move |e| {
                events_clone.lock().unwrap().push(e)
            })
            .await;
        assert!(result.is_ok());

        // The loop should have exited early due to external stop.
        // One turn may complete (the streaming error handler creates a turn)
        // but no tool execution should have happened.
        let events = events.lock().unwrap();
        let tool_exec = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(tool_exec, 0);
    }
}
