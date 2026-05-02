//! Core agent implementation

use crate::compaction::{CompactedContext as AgentCompactedContext, CompactionEvent};
use crate::config::AgentConfig;
use crate::events::AgentEvent;
use crate::state::{AgentState, SharedState};
use crate::tools::{ToolRegistry, AgentTool, AgentToolResult};
use crate::types::{StopReason, Response};
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{
    get_model, CompactionManager, CompactionStrategy, Context, LlmCompactor, 
    Message, Provider, ProviderEvent, StreamOptions, ToolCall, progress_callback,
};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Agent runtime
pub struct Agent {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    state: SharedState,
    compaction_manager: CompactionManager,
}

impl Agent {
    /// Create a new agent with the given provider and config
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig) -> Self {
        let mut compaction_manager = CompactionManager::new(
            config.compaction_strategy.clone(),
            config.context_window,
        );
        
        // Pre-initialize the LLM compactor if compaction is enabled
        if config.compaction_strategy != CompactionStrategy::Disabled {
            let model_id = config.model_id.clone();
            let model = {
                let parts: Vec<&str> = model_id.split('/').collect();
                if parts.len() == 2 {
                    get_model(parts[0], parts[1])
                } else {
                    get_model("anthropic", &model_id)
                }
            };
            
            if let Some(model) = model {
                let llm_compactor = Arc::new(LlmCompactor::new(model.clone(), Arc::clone(&provider)));
                compaction_manager.set_compactor(llm_compactor);
            }
        }
        
        Self {
            provider,
            config,
            tools: Arc::new(ToolRegistry::new()),
            state: SharedState::new(),
            compaction_manager,
        }
    }

    /// Get the agent configuration
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Get the tool registry
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
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
    pub fn add_tool<T: AgentTool + 'static>(&self, tool: T) {
        self.tools.register(tool);
    }

    /// Get the compaction manager
    pub fn compaction_manager(&self) -> &CompactionManager {
        &self.compaction_manager
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
        
        // Check for compaction at the start of each iteration
        let messages = &self.state.get_state().messages;
        let iteration = self.state.get_state().iteration;
        
        // Estimate token count
        let context_text = serde_json::to_string(messages).unwrap_or_default();
        let context_tokens = oxi_ai::estimate_tokens(&context_text);
        
        // Try to compact if needed
        if self.compaction_manager.should_compact(context_tokens, iteration) {
            let _ = tx.send(AgentEvent::Compaction { 
                event: CompactionEvent::Triggered { 
                    context_tokens, 
                    iteration 
                }
            }).await;
            
            // Clone messages for compaction since compact_if_needed takes a reference
            let messages_to_compact: Vec<Message> = messages.iter().cloned().collect();
            
            match self.compaction_manager.compact_if_needed(
                &messages_to_compact,
                self.config.compaction_instruction.as_deref(),
                context_tokens,
                iteration,
            ).await {
                Ok(Some(compacted)) => {
                    let start = Instant::now();
                    let message_count = compacted.compacted_count;
                    let _ = tx.send(AgentEvent::Compaction { 
                        event: CompactionEvent::Started { 
                            message_count 
                        }
                    }).await;
                    
                    // Extract data before moving
                    let kept_messages = compacted.kept_messages;
                    let summary = compacted.summary;
                    let compacted_count = compacted.compacted_count;
                    
                    // Replace old messages with compacted context
                    self.state.update(|s| {
                        s.replace_messages(kept_messages);
                    });
                    
                    let compacted_ctx = AgentCompactedContext {
                        summary,
                        kept_messages: Vec::new(), // Already moved to state
                        compacted_count,
                    };
                    let _ = tx.send(AgentEvent::Compaction { 
                        event: CompactionEvent::Completed { 
                            result: compacted_ctx,
                            duration_ms: start.elapsed().as_millis() as u64,
                        }
                    }).await;
                }
                Ok(None) => {
                    // No compaction needed
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::Compaction { 
                        event: CompactionEvent::Failed { 
                            error: e.to_string() 
                        }
                    }).await;
                }
            }
        }
        
        let mut context = Context::new();
        
        // Add system prompt
        if let Some(ref system_prompt) = self.config.system_prompt {
            context.set_system_prompt(system_prompt.clone());
        }
        
        // Add previous messages
        for msg in &self.state.get_state().messages {
            context.add_message(msg.clone());
        }

        // Add tools to context
        let tool_defs = self.tools.definitions();
        if !tool_defs.is_empty() {
            let mut oxi_tools = Vec::new();
            for def in &tool_defs {
                let schema = serde_json::to_value(&def.input_schema).unwrap_or_else(|_| {
                    serde_json::json!({"type": "object", "properties": {}})
                });
                oxi_tools.push(oxi_ai::Tool::new(&def.name, &def.description, schema));
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
        let tx_clone = tx.clone();

        // Clone tools for async task
        let tools = self.tools.clone();

        while let Some(event) = stream.next().await {
            match event {
                ProviderEvent::TextDelta { delta, .. } => {
                    response_text.push_str(&delta);
                    let _ = tx_clone.send(AgentEvent::TextChunk { text: delta }).await;
                }
                ProviderEvent::ToolCallStart { content_index, partial, .. } => {
                    // Track tool start - extract info from partial message if available
                    // Note: content_index is not directly accessible as tool_call_id
                    // In a full implementation, we'd track this differently
                    let _ = content_index; // Suppress unused warning
                    let _ = partial; // Suppress unused warning
                    // Tool call will be tracked when ToolCallEnd arrives
                }
                ProviderEvent::ToolCallEnd { tool_call, .. } => {
                    // Execute the tool and send results
                    let tool_call_id = tool_call.id.clone();
                    let tool_name = tool_call.name.clone();
                    
                    // Execute tool with progress callback
                    let tool_result = self.execute_tool(&tools, &tool_call, tx_clone.clone()).await;
                    
                    // Send result
                    let _ = tx_clone.send(AgentEvent::ToolComplete { result: tool_result.clone() }).await;
                    
                    // Add tool result to context for next turn
                    context.add_message(Message::User(oxi_ai::UserMessage::new(
                        format!("Tool {} returned: {}", tool_name, tool_result.content)
                    )));
                    
                    // Continue streaming for the next response
                    // Note: This is a simplified loop - a real implementation would handle
                    // continuing the conversation after tool results
                }
                ProviderEvent::Done { message, .. } => {
                    let content = message.text_content();
                    let _ = tx_clone.send(AgentEvent::Complete {
                        content: content.clone(),
                        stop_reason: format!("{:?}", message.stop_reason),
                    }).await;
                    self.state.update(|s| {
                        s.add_assistant_message(content.clone());
                        s.increment_iteration();
                    });
                    let _ = tx_clone.send(AgentEvent::Iteration { number: self.state.get_state().iteration }).await;
                    return Ok(Response {
                        content,
                        stop_reason: StopReason::Stop,
                    });
                }
                ProviderEvent::Error { error, .. } => {
                    let msg = error.text_content();
                    let _ = tx_clone.send(AgentEvent::Error { message: msg.clone() }).await;
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

    /// Execute a tool with progress streaming
    async fn execute_tool(
        &self,
        tools: &Arc<ToolRegistry>,
        tool_call: &ToolCall,
        tx: mpsc::Sender<AgentEvent>,
    ) -> oxi_ai::ToolResult {
        let tool_call_id = tool_call.id.clone();
        let tool_name = tool_call.name.clone();
        
        let tool = match tools.get(&tool_name) {
            Some(t) => t,
            None => {
                return oxi_ai::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    content: format!("Error: Unknown tool '{}'", tool_name),
                    status: "error".to_string(),
                };
            }
        };
        
        // Set up progress callback that emits to the channel
        let tool_call_id_clone = tool_call_id.clone();
        let tx_clone = tx.clone();
        let progress_cb = progress_callback(move |msg: String| {
            let tx = tx_clone.clone();
            let tool_call_id = tool_call_id_clone.clone();
            tokio::spawn(async move {
                let _ = tx.send(AgentEvent::ToolProgress {
                    tool_call_id,
                    message: msg,
                }).await;
            });
        });
        
        // Set the callback on the tool
        tool.on_progress(progress_cb);
        
        // tool_call.arguments is already JsonValue, use it directly
        let params = tool_call.arguments.clone();
        
        // Execute the tool
        match tool.execute(&tool_call_id, params, None).await {
            Ok(AgentToolResult { success, output, .. }) => {
                oxi_ai::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    content: output,
                    status: if success { "success".to_string() } else { "error".to_string() },
                }
            }
            Err(e) => {
                oxi_ai::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    content: e,
                    status: "error".to_string(),
                }
            }
        }
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
