//! Core agent implementation

use crate::config::AgentConfig;
use crate::events::AgentEvent;
use crate::state::{AgentState, SharedState};
use crate::tools::ToolRegistry;
use crate::types::{StopReason, Response};
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{get_model, Provider, ProviderEvent, StreamOptions};
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
        let handler = move |input: String| {
            Box::pin(handler(input)) as std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
        };
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

        let model = {
            let parts: Vec<&str> = self.config.model_id.split('/').collect();
            if parts.len() == 2 {
                get_model(parts[0], parts[1])
            } else {
                get_model("anthropic", &self.config.model_id)
            }
        }.ok_or_else(|| Error::msg(format!("Model not found: {}", self.config.model_id)))?;
        
        let mut context = oxi_ai::Context::new();
        
        // Add system prompt
        if let Some(ref system_prompt) = self.config.system_prompt {
            context.set_system_prompt(system_prompt.clone());
        }
        
        // Add previous messages
        for msg in &self.state.get_state().messages {
            context.add_message(msg.clone());
        }

        // Add tools to context
        let tools = self.tools.get_tools();
        if !tools.is_empty() {
            let mut oxi_tools = Vec::new();
            for tool in &tools {
                let schema = serde_json::json!({
                    "type": "object",
                    "properties": {},
                });
                oxi_tools.push(oxi_ai::Tool::new(&tool.name, &tool.description, schema));
            }
            context.set_tools(oxi_tools);
        }

        let stream_options = StreamOptions {
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            ..Default::default()
        };

        let mut stream = self.provider.stream(&model, &context, Some(stream_options)).await
            .map_err(|e| Error::msg(e.to_string()))?;

        let mut response_text = String::new();

        while let Some(event) = stream.next().await {
            match event {
                ProviderEvent::TextDelta { delta, .. } => {
                    response_text.push_str(&delta);
                    let _ = tx.send(AgentEvent::TextChunk { text: delta }).await;
                }
                ProviderEvent::Done { message, .. } => {
                    let content = message.text_content();
                    let _ = tx.send(AgentEvent::Complete {
                        content: content.clone(),
                        stop_reason: format!("{:?}", message.stop_reason),
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
                ProviderEvent::Error { error, .. } => {
                    let msg = error.text_content();
                    let _ = tx.send(AgentEvent::Error { message: msg.clone() }).await;
                    return Err(Error::msg(msg));
                }
                _ => {}
            }
        }

        Ok(Response {
            content: response_text,
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
