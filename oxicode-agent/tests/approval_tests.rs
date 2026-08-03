//! Approval/tier integration tests
//!
//! Verifies the approval gate fires `ApprovalRequired` events for tools
//! whose tier is in `require_approval_for`, and that denied/approved tools
//! produce the expected ToolExecutionEnd status.

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use oxicode_agent::{
        AgentEvent, AgentLoop, AgentLoopConfig, ProviderResolver, SharedState,
        agent_loop::config::{ApprovalConfig, ApprovalDecision},
        tools::{AgentTool, AgentToolResult, ToolRegistry, ToolTier},
    };
    use oxicode_ai::{
        AssistantMessage, ContentBlock, Model, Provider, ProviderEvent, StopReason, StreamResult,
        TextContent, ToolCall,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    // ── Mock Resolver ────────────────────────────────────────────────

    struct MockResolver;

    impl ProviderResolver for MockResolver {
        fn resolve_provider(&self, _name: &str) -> Option<Arc<dyn Provider>> {
            None
        }

        fn resolve_model(&self, _model_id: &str) -> Option<Model> {
            Some(Model::new(
                "mock/model",
                "Mock Model",
                oxicode_ai::Api::AnthropicMessages,
                "mock",
                "https://mock.example.com",
            ))
        }
    }

    // ── Mock Provider ──────────────────────────────────────────────────

    /// Mock provider that returns a tool call, then a text response.
    struct MockProvider {
        call_count: Arc<AtomicUsize>,
        tool_name: String,
    }

    impl MockProvider {
        fn new(tool_name: &str) -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
                tool_name: tool_name.to_string(),
            }
        }
    }

    impl Provider for MockProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a oxicode_ai::Context,
            _options: Option<oxicode_ai::StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            let call_count = Arc::clone(&self.call_count);
            let tool_name = self.tool_name.clone();
            Box::pin(async move {
                let count = call_count.fetch_add(1, Ordering::Relaxed);

                if count == 0 {
                    // First call: return a tool call
                    let tc = ToolCall::new(
                        "call_1",
                        tool_name,
                        serde_json::json!({"command": "echo hello"}),
                    );
                    let mut assistant = AssistantMessage::new(
                        oxicode_ai::Api::AnthropicMessages,
                        "mock",
                        "mock-model",
                    );
                    assistant.content = vec![ContentBlock::ToolCall(tc)];
                    assistant.stop_reason = StopReason::ToolUse;

                    let events = vec![
                        ProviderEvent::Start {
                            partial: Arc::new(assistant.clone()),
                        },
                        ProviderEvent::Done {
                            reason: StopReason::ToolUse,
                            message: assistant,
                        },
                    ];
                    Ok(Box::pin(futures::stream::iter(events))
                        as Pin<
                            Box<dyn futures::Stream<Item = ProviderEvent> + Send>,
                        >)
                } else {
                    // Second call: return text response
                    let mut assistant = AssistantMessage::new(
                        oxicode_ai::Api::AnthropicMessages,
                        "mock",
                        "mock-model",
                    );
                    assistant.content = vec![ContentBlock::Text(TextContent::new("Done."))];
                    assistant.stop_reason = StopReason::Stop;

                    let events = vec![
                        ProviderEvent::Start {
                            partial: Arc::new(assistant.clone()),
                        },
                        ProviderEvent::Done {
                            reason: StopReason::Stop,
                            message: assistant,
                        },
                    ];
                    Ok(Box::pin(futures::stream::iter(events))
                        as Pin<
                            Box<dyn futures::Stream<Item = ProviderEvent> + Send>,
                        >)
                }
            })
        }
    }

    // ── Mock Tool ──────────────────────────────────────────────────────

    struct ExecTool;

    #[async_trait]
    impl AgentTool for ExecTool {
        fn name(&self) -> &str {
            "bash"
        }

        fn label(&self) -> &str {
            "Bash"
        }

        fn description(&self) -> &str {
            "Execute shell commands"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            })
        }

        fn tool_tier(&self) -> ToolTier {
            ToolTier::Exec
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::oneshot::Receiver<()>>,
            _ctx: &oxicode_agent::tools::ToolContext,
        ) -> Result<AgentToolResult, String> {
            Ok(AgentToolResult::success("executed"))
        }
    }

    // ── Tests ──────────────────────────────────────────────────────────

    fn make_loop(config: AgentLoopConfig) -> AgentLoop {
        let tool_registry = ToolRegistry::new();
        tool_registry.register(ExecTool);

        let provider = Arc::new(MockProvider::new("bash"));
        let state = SharedState::new();

        AgentLoop::new_with_resolver(
            provider,
            config,
            Arc::new(tool_registry),
            state,
            Arc::new(MockResolver),
        )
    }

    #[tokio::test]
    async fn test_approval_required_event_emitted_for_exec_tool() {
        let approval_hook: oxicode_agent::agent_loop::config::ApprovalHook =
            Arc::new(|tool_name: &str, _args: &serde_json::Value| {
                let name = tool_name.to_string();
                Box::pin(async move {
                    Ok(ApprovalDecision::RequireApproval(format!(
                        "Need approval for {}",
                        name
                    )))
                })
            });

        let config = AgentLoopConfig {
            model_id: "mock/model".into(),
            approval_config: ApprovalConfig {
                require_approval_for: vec![ToolTier::Exec],
                hook: Some(approval_hook),
            },
            ..Default::default()
        };

        let loop_instance = make_loop(config);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        let result = loop_instance
            .run("Run a command".into(), move |event| {
                events_clone.lock().unwrap().push(event);
            })
            .await;

        assert!(
            result.is_ok(),
            "Agent loop should complete: {:?}",
            result.err()
        );

        let captured = events.lock().unwrap();
        let approval_events: Vec<_> = captured
            .iter()
            .filter(|e| matches!(e, AgentEvent::ApprovalRequired { .. }))
            .collect();

        assert!(
            !approval_events.is_empty(),
            "Expected at least one ApprovalRequired event"
        );

        if let AgentEvent::ApprovalRequired {
            tool_name, reason, ..
        } = &approval_events[0]
        {
            assert_eq!(tool_name, "bash");
            assert!(reason.contains("Need approval for bash"));
        }
    }

    #[tokio::test]
    async fn test_approval_denied_blocks_tool() {
        let approval_hook: oxicode_agent::agent_loop::config::ApprovalHook =
            Arc::new(|_: &str, _: &serde_json::Value| {
                Box::pin(async move { Ok(ApprovalDecision::Deny("Not allowed".into())) })
            });

        let config = AgentLoopConfig {
            model_id: "mock/model".into(),
            approval_config: ApprovalConfig {
                require_approval_for: vec![ToolTier::Exec],
                hook: Some(approval_hook),
            },
            ..Default::default()
        };

        let loop_instance = make_loop(config);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        let result = loop_instance
            .run("Run a command".into(), move |event| {
                events_clone.lock().unwrap().push(event);
            })
            .await;

        assert!(
            result.is_ok(),
            "Agent loop should complete: {:?}",
            result.err()
        );

        let captured = events.lock().unwrap();
        let denied_results: Vec<_> = captured
            .iter()
            .filter_map(|e| {
                if let AgentEvent::ToolExecutionEnd {
                    is_error, result, ..
                } = e
                {
                    Some((*is_error, result.content.clone()))
                } else {
                    None
                }
            })
            .collect();

        assert!(!denied_results.is_empty(), "Expected ToolExecutionEnd");
        let (is_error, content) = &denied_results[0];
        assert!(*is_error, "Denied tool should have is_error=true");
        assert!(
            content.contains("Not allowed"),
            "Should mention denial reason: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_approval_allows_tool_execution() {
        let approval_hook: oxicode_agent::agent_loop::config::ApprovalHook =
            Arc::new(|_: &str, _: &serde_json::Value| {
                Box::pin(async move { Ok(ApprovalDecision::Allow) })
            });

        let config = AgentLoopConfig {
            model_id: "mock/model".into(),
            approval_config: ApprovalConfig {
                require_approval_for: vec![ToolTier::Exec],
                hook: Some(approval_hook),
            },
            ..Default::default()
        };

        let loop_instance = make_loop(config);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        let result = loop_instance
            .run("Run a command".into(), move |event| {
                events_clone.lock().unwrap().push(event);
            })
            .await;

        assert!(
            result.is_ok(),
            "Agent loop should complete: {:?}",
            result.err()
        );

        let captured = events.lock().unwrap();
        let results: Vec<_> = captured
            .iter()
            .filter_map(|e| {
                if let AgentEvent::ToolExecutionEnd {
                    is_error, result, ..
                } = e
                {
                    Some((*is_error, result.content.clone()))
                } else {
                    None
                }
            })
            .collect();

        assert!(!results.is_empty(), "Expected ToolExecutionEnd");
        let (is_error, content) = &results[0];
        assert!(
            !*is_error,
            "Allowed tool should have is_error=false, content: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_approval_disabled_by_default() {
        let config = AgentLoopConfig {
            model_id: "mock/model".into(),
            ..Default::default()
        };

        let loop_instance = make_loop(config);

        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        let result = loop_instance
            .run("Run a command".into(), move |event| {
                events_clone.lock().unwrap().push(event);
            })
            .await;

        assert!(
            result.is_ok(),
            "Agent loop should complete: {:?}",
            result.err()
        );

        let captured = events.lock().unwrap();
        let approval_events: Vec<_> = captured
            .iter()
            .filter(|e| matches!(e, AgentEvent::ApprovalRequired { .. }))
            .collect();

        assert!(
            approval_events.is_empty(),
            "Expected no ApprovalRequired when disabled"
        );
    }
}
