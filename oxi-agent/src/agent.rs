//! Core agent implementation

use crate::config::AgentConfig;
use crate::events::AgentEvent;
use crate::state::{AgentState, SharedState};
use crate::tools::ToolRegistry;
use crate::types::{StopReason, ToolCall, ToolResult, Response};
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{Context, get_model, Provider, ProviderEvent, StreamOptions};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Agent runtime
pub struct Agent {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    state: SharedState,
}

impl Agent {
    /// Create a new agent with the given provider and config
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
        Self {
            provider,
            config,
            tools: ToolRegistry::new(),
            state: SharedState::new(),
        }
    }

    /// Get the agent configuration
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Get the tool registry
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Get a clone of the current state
    pub fn state(&self) -> AgentState {
        self.state.get_state()
    }

    /// Reset agent state for a new conversation
    pub fn reset(&self) {
        self.state.reset();
    }

    /// Add a tool to the agent
    pub fn add_tool<F, Fut>(&self, name: String, description: String, handler: F)
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = String> + Send + 'static,
    {
        let definition = crate::types::ToolDefinition::new(
            name,
            description,
            std::collections::HashMap::new(),
        );
        self.tools.register(definition, handler);
    }

    /// Run the agent with a prompt, returning events via a channel
    pub async fn run(&self, prompt: String) -> Result<(Response, Vec<AgentEvent>)> {
        let mut events = Vec::new();
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);
        let result = self.run_with_channel(prompt, tx).await;
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        result.map(|r| (r, events))
    }

    /// Run agent with event channel
    pub async fn run_with_channel(&self, prompt: String, tx: mpsc::Sender<AgentEvent>) -> Result<Response> {
        let _ = tx.send(AgentEvent::Start { prompt: prompt.clone() }).await;
        let _ = tx.send(AgentEvent::Thinking).await;

        self.state.update(|s| {
            s.add_user_message(prompt);
        });

        let model = get_model("anthropic", &self.config.model_id)?;
        let messages = {
            let state = self.state.get_state();
            state.messages.clone()
        };
        let mut context = Context::new(messages);

        if let Some(ref system_prompt) = self.config.system_prompt {
            context.add_system(system_prompt.clone());
        }

        let tools = self.tools.get_tools();
        if !tools.is_empty() {
            for tool in &tools {
                context.add_tool(oxi_ai::Tool::new(
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.input_schema.clone(),
                ));
            }
        }

        let stream_options = StreamOptions {
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            ..Default::default()
        };

        let mut stream = self.provider.stream(&model, &context, Some(stream_options)).await
            .map_err(|e| Error::msg(e.to_string()))?;

        let mut response_content = String::new();
        let mut tool_calls = Vec::new();

        while let Some(event) = stream.next().await {
            let event = event.map_err(|e| Error::msg(e.to_string()))?;
            match event {
                ProviderEvent::ContentBlock { content } => {
                    if let oxi_ai::ContentBlock::Text(text_block) = content {
                        response_content.push_str(&text_block.text);
                        let _ = tx.send(AgentEvent::TextChunk { text: text_block.text }).await;
                    }
                }
                ProviderEvent::ToolUse { id, name, input } => {
                    let arguments = serde_json::to_string(&input).unwrap_or_default();
                    let tool_call = ToolCall::new(id, name, arguments);
                    tool_calls.push(tool_call.clone());
                    let _ = tx.send(AgentEvent::ToolCall { tool_call }).await;
                }
                ProviderEvent::MessageStop { stop_reason, content, .. } => {
                    for tool_call in &tool_calls {
                        let _ = tx.send(AgentEvent::ToolStart {
                            tool_call_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                        }).await;
                        let result = format!("Tool '{}' executed", tool_call.name);
                        let tool_result = ToolResult::success(&tool_call.id, result);
                        self.state.update(|s| {
                            s.add_tool_result(tool_call.id.clone(), tool_result.content.clone());
                        });
                        let _ = tx.send(AgentEvent::ToolComplete { result: tool_result }).await;
                    }
                    let _ = tx.send(AgentEvent::Complete {
                        content: content.clone(),
                        stop_reason: stop_reason.to_string(),
                    }).await;
                    self.state.update(|s| {
                        s.add_assistant_message(content.clone());
                        s.increment_iteration();
                    });
                    let _ = tx.send(AgentEvent::Iteration { number: self.state.get_state().iteration }).await;
                    return Ok(Response {
                        content,
                        stop_reason: StopReason::Stop,
                    });
                }
                ProviderEvent::Error { error } => {
                    let _ = tx.send(AgentEvent::Error { message: error }).await;
                    return Err(Error::msg("Provider error"));
                }
                _ => {}
            }
        }

        Ok(Response {
            content: response_content,
            stop_reason: StopReason::Stop,
        })
    }

    /// Run agent with streaming callback
    pub async fn run_streaming<F>(&self, prompt: String, mut on_event: F) -> Result<Response>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);
        let tx_clone = tx;
        let result = self.run_with_channel(prompt, tx_clone).await;
        while let Some(event) = rx.recv().await {
            on_event(event);
        }
        result
    }
}
