//! Tool execution for oxi-agent
//!
//! Handles sequential and parallel tool execution with before/after hooks.

use crate::error::Result;
use crate::event::AgentEvent;
use crate::types::{
    AfterToolCallContext, AfterToolCallResult, AgentContext, AgentMessage, AgentTool,
    AgentToolCall, AgentToolResult, BeforeToolCallContext, BeforeToolCallResult,
};
use async_trait::async_trait;
use oxi_ai::{ContentBlock, Message, ToolResultMessage};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::watch;

/// Tool execution mode
pub use crate::types::ToolExecutionMode;

/// Callback for streaming tool execution updates
pub type ToolUpdateCallback = Arc<dyn Fn(AgentToolResult) + Send + Sync>;

/// Agent tool trait
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;
    /// Human-readable label for UI
    fn label(&self) -> &str;
    /// Tool parameter schema (JSON)
    fn schema(&self) -> &JsonValue;
    /// Optional argument preparation
    fn prepare_arguments(&self, args: JsonValue) -> JsonValue {
        args
    }
    /// Execute the tool call
    async fn execute(
        &self,
        tool_call_id: String,
        params: JsonValue,
        signal: Option<&watch::Receiver<bool>>,
        on_update: Option<ToolUpdateCallback>,
    ) -> Result<AgentToolResult>;
    /// Per-tool execution mode override
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }
}

/// Tool executor for the agent
pub struct ToolExecutor {
    config: crate::types::AgentConfig,
    event_tx: broadcast::Sender<AgentEvent>,
}

impl ToolExecutor {
    pub fn new(config: crate::types::AgentConfig, event_tx: broadcast::Sender<AgentEvent>) -> Self {
        Self { config, event_tx }
    }

    /// Execute tool calls from an assistant message
    pub async fn execute_tool_calls(
        &self,
        context: &mut AgentContext,
        assistant_message: &oxi_ai::AssistantMessage,
        tool_calls: Vec<AgentToolCall>,
        signal: Option<&watch::Receiver<bool>>,
    ) -> Result<ExecuteToolCallResult> {
        let has_sequential = tool_calls.iter().any(|tc: &AgentToolCall| {
            self.config
                .tools
                .iter()
                .find(|t: &Arc<dyn AgentTool| t.name() == tc.name)
                .and_then(|t: &Arc<dyn AgentTool| t.execution_mode())
                .map(|m: ToolExecutionMode| m == ToolExecutionMode::Sequential)
                .unwrap_or(false)
        });

        if self.config.tool_execution == ToolExecutionMode::Sequential || has_sequential {
            self.execute_sequential(context, assistant_message, tool_calls, signal).await
        } else {
            self.execute_parallel(context, assistant_message, tool_calls, signal).await
        }
    }

    async fn execute_sequential(
        &self,
        context: &mut AgentContext,
        assistant_message: &oxi_ai::AssistantMessage,
        tool_calls: Vec<AgentToolCall>,
        signal: Option<&watch::Receiver<bool>>,
    ) -> Result<ExecuteToolCallResult> {
        let mut messages = Vec::new();
        let mut finalized = Vec::new();
        let mut terminate = false;

        for tool_call in tool_calls {
            let result: FinalizedToolCall = self
                .execute_single_tool(context, assistant_message, &tool_call, signal)
                .await?;
            
            finalized.push(result.clone());
            messages.push(result.message.clone());
            terminate = terminate || result.result.terminate;
        }

        Ok(ExecuteToolCallResult {
            messages,
            terminate,
        })
    }

    async fn execute_parallel(
        &self,
        context: &mut AgentContext,
        assistant_message: &oxi_ai::AssistantMessage,
        tool_calls: Vec<AgentToolCall>,
        signal: Option<&watch::Receiver<bool>>,
    ) -> Result<ExecuteToolCallResult> {
        // Pre-flight: emit tool_execution_start and prepare each tool call
        let mut prepared: Vec<PreparedToolCall> = Vec::new();
        
        for tool_call in &tool_calls {
            let _ = self.event_tx.send(AgentEvent::ToolExecutionStart {
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                args: tool_call.arguments.clone(),
            });

            match self.prepare_tool_call(context, assistant_message, tool_call) {
                Ok(prep) => prepared.push(prep),
                Err(result) => {
                    // Immediate error result
                    prepared.push(PreparedToolCall::Immediate {
                        tool_call: tool_call.clone(),
                        result,
                        is_error: true,
                    });
                }
            }
        }

        // Separate immediate results from those needing execution
        let (immediate, needs_execution): (Vec<_>, Vec<_>) = prepared
            .into_iter()
            .partition(|p| matches!(p, PreparedToolCall::Immediate { .. }));

        // Execute immediate results and emit end events
        let mut finalized: Vec<FinalizedToolCall> = Vec::new();
        for prep in immediate {
            if let PreparedToolCall::Immediate { tool_call, result, is_error } = prep {
                self.emit_tool_execution_end(&tool_call, &result, is_error);
                finalized.push(FinalizedToolCall {
                    tool_call,
                    result,
                    is_error,
                });
            }
        }

        // Execute remaining tools in parallel
        let executions: Vec<_> = needs_execution
            .into_iter()
            .map(|prep| {
                let event_tx = self.event_tx.clone();
                let signal = signal.cloned();
                async move {
                    if let PreparedToolCall::Prepared {
                        tool_call,
                        tool,
                        args,
                    } = prep
                    {
                        let result: AgentToolResult = tool
                            .execute(tool_call.id.clone(), args.clone(), signal.as_ref(), None)
                            .await
                            .unwrap_or_else(|e: crate::error::AgentError| AgentToolResult::error(e.to_string()));

                        (tool_call, tool, args, result)
                    } else {
                        unreachable!()
                    }
                }
            })
            .collect();

        let results = futures::future::join_all(executions).await;

        for (tool_call, _tool, args, result) in results {
            let is_error = result.content.iter().any(|c| {
                matches!(c, ContentBlock::Text(t) if t.text.contains("Error"))
            });
            
            // Call afterToolCall if configured
            let final_result = if let Some(ref hook) = self.config.after_tool_call {
                let ctx = AfterToolCallContext {
                    assistant_message: assistant_message.clone(),
                    tool_call: tool_call.clone(),
                    args,
                    result: result.clone(),
                    is_error,
                    context: context.clone(),
                };
                let hook_result = hook(ctx);
                if let Some(override_result) = self.apply_after_tool_result(result, hook_result) {
                    override_result
                } else {
                    result
                }
            } else {
                result
            };

            self.emit_tool_execution_end(&tool_call, &final_result, is_error);
            finalized.push(FinalizedToolCall {
                tool_call,
                result: final_result,
                is_error,
            });
        }

        // Emit tool result messages in source order
        let mut messages = Vec::new();
        let mut terminate = true;

        for finalized in &finalized {
            let tool_result_message = ToolResultMessage {
                role: oxi_ai::ToolResultRole::ToolResult,
                tool_call_id: finalized.tool_call.id.clone(),
                tool_name: finalized.tool_call.name.clone(),
                content: finalized.result.content.clone(),
                details: Some(finalized.result.details.clone()),
                is_error: finalized.is_error,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };

            let _ = self.event_tx.send(AgentEvent::MessageStart {
                message: AgentMessage::Llm(Message::ToolResult(tool_result_message.clone())),
            });
            let _ = self.event_tx.send(AgentEvent::MessageEnd {
                message: AgentMessage::Llm(Message::ToolResult(tool_result_message.clone())),
            });

            messages.push(tool_result_message);
            terminate = terminate && finalized.result.terminate;
        }

        Ok(ExecuteToolCallResult {
            messages,
            terminate,
        })
    }

    async fn execute_single_tool(
        &self,
        context: &mut AgentContext,
        assistant_message: &oxi_ai::AssistantMessage,
        tool_call: &AgentToolCall,
        signal: Option<&watch::Receiver<bool>>,
    ) -> Result<FinalizedToolCall> {
        // Emit start
        let _ = self.event_tx.send(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        });

        // Prepare
        let prepared = match self.prepare_tool_call(context, assistant_message, tool_call) {
            Ok(p) => p,
            Err(result) => {
                let is_error = true;
                self.emit_tool_execution_end(tool_call, &result, is_error);
                return Ok(FinalizedToolCall {
                    tool_call: tool_call.clone(),
                    result,
                    is_error,
                });
            }
        };

        // Execute
        let result: AgentToolResult = match prepared {
            PreparedToolCall::Prepared { tool_call, tool, args } => {
                tool.execute(tool_call.id.clone(), args.clone(), signal, None)
                    .await
                    .unwrap_or_else(|e: crate::error::AgentError| AgentToolResult::error(e.to_string()))
            }
            PreparedToolCall::Immediate { result, .. } => result,
        };

        let is_error = result.content.iter().any(|c| {
            matches!(c, ContentBlock::Text(t) if t.text.contains("Error"))
        });

        // Apply afterToolCall hook
        let final_result = if let Some(ref hook) = self.config.after_tool_call {
            let ctx = AfterToolCallContext {
                assistant_message: assistant_message.clone(),
                tool_call: tool_call.clone(),
                args: prepared.args(),
                result: result.clone(),
                is_error,
                context: context.clone(),
            };
            let hook_result = hook(ctx);
            self.apply_after_tool_result(result, hook_result)
                .unwrap_or(result)
        } else {
            result
        };

        let final_is_error = final_result.content.iter().any(|c| {
            matches!(c, ContentBlock::Text(t) if t.text.contains("Error"))
        });

        // Emit end
        self.emit_tool_execution_end(tool_call, &final_result, final_is_error);

        Ok(FinalizedToolCall {
            tool_call: tool_call.clone(),
            result: final_result,
            is_error: final_is_error,
        })
    }

    fn prepare_tool_call(
        &self,
        context: &AgentContext,
        assistant_message: &oxi_ai::AssistantMessage,
        tool_call: &AgentToolCall,
    ) -> std::result::Result<PreparedToolCall, AgentToolResult> {
        let tool = self
            .config
            .tools
            .iter()
            .find(|t: &Arc<dyn AgentTool| t.name() == tool_call.name)
            .ok_or_else(|| AgentToolResult::error(format!("Tool {} not found", tool_call.name)))?;

        let prepared_args = tool.prepare_arguments(tool_call.arguments.clone());
        
        // Validate arguments against schema
        let validated_args = self.validate_arguments(tool, &prepared_args)?;

        // Call beforeToolCall hook
        if let Some(ref hook) = self.config.before_tool_call {
            let ctx = BeforeToolCallContext {
                assistant_message: assistant_message.clone(),
                tool_call: tool_call.clone(),
                args: validated_args.clone(),
                context: context.clone(),
            };
            let result = hook(ctx);
            if result.block {
                return Err(AgentToolResult::error(
                    result.reason.unwrap_or_else(|| "Tool execution was blocked".to_string()),
                ));
            }
        }

        Ok(PreparedToolCall::Prepared {
            tool_call: tool_call.clone(),
            tool: Arc::clone(tool),
            args: validated_args,
        })
    }

    fn validate_arguments(
        &self,
        tool: &Arc<dyn AgentTool>,
        args: &JsonValue,
    ) -> std::result::Result<JsonValue, AgentToolResult> {
        let schema = tool.schema();
        
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            if let Some(obj) = args.as_object() {
                for req in required {
                    if let Some(req_str) = req.as_str() {
                        if !obj.contains_key(req_str) {
                            return Err(AgentToolResult::error(format!(
                                "Missing required argument: {}",
                                req_str
                            )));
                        }
                    }
                }
            }
        }

        Ok(args.clone())
    }

    fn apply_after_tool_result(
        &self,
        result: AgentToolResult,
        hook_result: AfterToolCallResult,
    ) -> Option<AgentToolResult> {
        if hook_result.content.is_none()
            && hook_result.details.is_none()
            && hook_result.is_error.is_none()
            && hook_result.terminate.is_none()
        {
            return None;
        }

        Some(AgentToolResult {
            content: hook_result.content.unwrap_or(result.content),
            details: hook_result.details.unwrap_or(result.details),
            terminate: hook_result.terminate.unwrap_or(result.terminate),
        })
    }

    fn emit_tool_execution_end(
        &self,
        tool_call: &AgentToolCall,
        result: &AgentToolResult,
        is_error: bool,
    ) {
        let _ = self.event_tx.send(AgentEvent::ToolExecutionEnd {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            result: serde_json::to_value(result).unwrap_or(JsonValue::Null),
            is_error,
        });
    }
}

enum PreparedToolCall {
    Prepared {
        tool_call: AgentToolCall,
        tool: Arc<dyn AgentTool>,
        args: JsonValue,
    },
    Immediate {
        tool_call: AgentToolCall,
        result: AgentToolResult,
        is_error: bool,
    },
}

impl PreparedToolCall {
    fn args(&self) -> JsonValue {
        match self {
            PreparedToolCall::Prepared { args, .. } => args.clone(),
            PreparedToolCall::Immediate { .. } => JsonValue::Null,
        }
    }
}

struct FinalizedToolCall {
    tool_call: AgentToolCall,
    result: AgentToolResult,
    message: ToolResultMessage,
}

pub struct ExecuteToolCallResult {
    pub messages: Vec<ToolResultMessage>,
    pub terminate: bool,
}
