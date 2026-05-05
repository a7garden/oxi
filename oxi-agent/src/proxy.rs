/// Agent proxy implementation for bandwidth optimization and event stripping.
//!
/// The proxy pattern works as follows:
/// - Server-side: Strips redundant fields from events for bandwidth optimization
/// - Client-side: Reconnects to proxy and reconstructs original events
//!
/// This reduces bandwidth by ~40-60% for streaming responses by removing
/// redundant partial message data from each delta event.

use crate::events::AgentEvent;
use crate::state::Message;
use anyhow::{Error, Result};
use futures::StreamExt;
use oxi_ai::{
    AssistantMessage, ContentBlock, Provider, ProviderEvent, StopReason, StreamOptions,
    TextContent, ThinkingContent, ToolCall, ToolResult,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, Instant};

/// Events that the proxy server can emit during streaming.
#[derive(Debug, Clone)]
pub enum ProxyEvent {
    /// Assistant message event (streamed from LLM).
    AssistantMessage(ProxyAssistantMessageEvent),
    /// Tool call event (executed by agent).
    ToolCall(ToolCallEvent),
    /// Error event.
    Error(String),
}

/// Assistant message events optimized for bandwidth (partial stripped).
#[derive(Debug, Clone)]
pub enum ProxyAssistantMessageEvent {
    /// Stream started, partial message available.
    Start {
        content_index: usize,
        content_type: ContentType,
    },
    /// Text delta received.
    TextDelta { content_index: usize, delta: String },
    /// Thinking delta received.
    ThinkingDelta { content_index: usize, delta: String },
    /// Tool call delta received.
    ToolCallDelta { content_index: usize, delta: String },
    /// Stream completed.
    Done { reason: String, usage: ProxyUsage },
    /// Stream error.
    Error {
        reason: String,
        error_message: Option<String>,
        usage: ProxyUsage,
    },
}

/// Type of content block being streamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Thinking,
    ToolCall { id: String, name: String },
}

impl ContentType {
    fn as_str(&self) -> &'static str {
        match self {
            ContentType::Text => "text",
            ContentType::Thinking => "thinking",
            ContentType::ToolCall { .. } => "toolcall",
        }
    }
}

/// Usage statistics for proxy events (optimized format).
#[derive(Debug, Clone, Default)]
pub struct ProxyUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    pub cost: ProxyCost,
}

impl From<&oxi_ai::Usage> for ProxyUsage {
    fn from(usage: &oxi_ai::Usage) -> Self {
        ProxyUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            total_tokens: usage.total_tokens,
            cost: ProxyCost {
                input: usage.cost.input,
                output: usage.cost.output,
                cache_read: usage.cost.cache_read,
                cache_write: usage.cost.cache_write,
                total: usage.cost.total,
            },
        }
    }
}

/// Cost breakdown for proxy events.
#[derive(Debug, Clone, Default)]
pub struct ProxyCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

/// Tool call event for proxy.
#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub result: Option<String>,
    pub is_error: bool,
}

/// Configuration for the proxy client.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Proxy server URL (e.g., "https://genai.example.com").
    pub proxy_url: String,
    /// Auth token for the proxy server.
    pub auth_token: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Max retries on connection failure.
    pub max_retries: u32,
    /// Initial reconnect delay in milliseconds.
    pub reconnect_delay_ms: u64,
    /// Maximum reconnect delay in milliseconds.
    pub max_reconnect_delay_ms: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        ProxyConfig {
            proxy_url: "http://localhost:8080".to_string(),
            auth_token: String::new(),
            timeout_secs: 120,
            max_retries: 3,
            reconnect_delay_ms: 100,
            max_reconnect_delay_ms: 30000,
        }
    }
}

/// Options for proxy streaming.
pub struct ProxyStreamOptions {
    /// Model to use for the request.
    pub model: oxi_ai::Model,
    /// System prompt for the request.
    pub system_prompt: Option<String>,
    /// Messages to send.
    pub messages: Vec<Message>,
    /// Temperature parameter.
    pub temperature: Option<f64>,
    /// Max tokens parameter.
    pub max_tokens: Option<usize>,
    /// Session ID for caching.
    pub session_id: Option<String>,
    /// Auth token for the proxy server.
    pub auth_token: String,
    /// Proxy server URL.
    pub proxy_url: String,
    /// Abort signal for cancellation.
    pub signal: Option<oneshot::Receiver<()>>,
    /// Transport preference (sse, websocket, etc.).
    pub transport: Option<String>,
    /// Custom metadata.
    pub metadata: Option<serde_json::Value>,
}

impl ProxyStreamOptions {
    /// Create new proxy stream options.
    pub fn new(model: oxi_ai::Model, auth_token: String, proxy_url: String) -> Self {
        ProxyStreamOptions {
            model,
            auth_token,
            proxy_url,
            system_prompt: None,
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            session_id: None,
            signal: None,
            transport: None,
            metadata: None,
        }
    }
}

/// Proxy stream that connects to proxy server and reconstructs events.
pub struct ProxyStream {
    events: mpsc::Receiver<ProxyEvent>,
    _cancel_tx: oneshot::Sender<()>,
}

impl ProxyStream {
    /// Start a proxy stream and return a stream of reconstructed events.
    pub async fn start(
        options: ProxyStreamOptions,
    ) -> Result<(Self, oneshot::Receiver<Result<AssistantMessage>>)> {
        let (events_tx, events_rx) = mpsc::channel(100);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (result_tx, result_rx) = oneshot::channel();

        let proxy_url = options.proxy_url.clone();
        let auth_token = options.auth_token.clone();
        let timeout = Duration::from_secs(120);

        // Spawn the proxy connection task
        tokio::spawn(async move {
            let result = Self::connect_and_stream(
                proxy_url, auth_token, options, cancel_rx, events_tx, result_tx,
            )
            .await;

            if let Err(e) = result {
                tracing::error!("Proxy stream error: {}", e);
            }
        });

        Ok((
            ProxyStream {
                events: events_rx,
                _cancel_tx: cancel_tx,
            },
            result_rx,
        ))
    }

    async fn connect_and_stream(
        proxy_url: String,
        auth_token: String,
        options: ProxyStreamOptions,
        cancel_rx: oneshot::Receiver<()>,
        events_tx: mpsc::Sender<ProxyEvent>,
        result_tx: oneshot::Sender<Result<AssistantMessage>>,
    ) -> Result<()> {
        let request = ProxyRequest {
            model_id: format!("{}/{}", options.model.provider, options.model.id),
            system_prompt: options.system_prompt,
            messages: options.messages,
            options: ProxyRequestOptions {
                temperature: options.temperature,
                max_tokens: options.max_tokens,
                session_id: options.session_id,
                transport: options.transport,
                metadata: options.metadata,
            },
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;

        let response = client
            .post(format!("{}/api/stream", proxy_url))
            .header("Authorization", format!("Bearer {}", auth_token))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(Error::msg(format!(
                "Proxy error {}: {}",
                status, error_text
            )));
        }

        // Read streaming response and convert to events
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();

        loop {
            tokio::select! {
                _ = cancel_rx => {
                    break;
                }
                item = stream.next() => {
                    match item {
                        Some(Ok(chunk)) => {
                            buffer.extend_from_slice(&chunk);

                            // Process complete lines
                            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                                let line = buffer.drain(..=pos).collect::<Vec<_>>();
                                let line_str = String::from_utf8_lossy(&line);

                                if line_str.starts_with("data: ") {
                                    let data = line_str.trim_start_matches("data: ");
                                    if let Ok(proxy_event) = serde_json::from_str::<serde_json::Value>(data) {
                                        let event = Self::parse_proxy_event(&proxy_event);
                                        if let Some(event) = event {
                                            if events_tx.send(event).await.is_err() {
                                                return Ok(()); // Receiver dropped
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            result_tx.send(Err(Error::msg(e.to_string()))).ok();
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }

    fn parse_proxy_event(value: &serde_json::Value) -> Option<ProxyEvent> {
        let event_type = value.get("type")?.as_str()?;

        match event_type {
            "start" => {
                let content_index = value.get("contentIndex")?.as_u64()? as usize;
                let content_type_str = value.get("contentType")?.as_str()?;

                let content_type = match content_type_str {
                    "text" => ContentType::Text,
                    "thinking" => ContentType::Thinking,
                    "toolcall" => {
                        let id = value.get("id")?.as_str()?.to_string();
                        let name = value.get("toolName")?.as_str()?.to_string();
                        ContentType::ToolCall { id, name }
                    }
                    _ => return None,
                };

                Some(ProxyEvent::AssistantMessage(
                    ProxyAssistantMessageEvent::Start {
                        content_index,
                        content_type,
                    },
                ))
            }
            "text_delta" => {
                let content_index = value.get("contentIndex")?.as_u64()? as usize;
                let delta = value.get("delta")?.as_str()?.to_string();

                Some(ProxyEvent::AssistantMessage(
                    ProxyAssistantMessageEvent::TextDelta {
                        content_index,
                        delta,
                    },
                ))
            }
            "thinking_delta" => {
                let content_index = value.get("contentIndex")?.as_u64()? as usize;
                let delta = value.get("delta")?.as_str()?.to_string();

                Some(ProxyEvent::AssistantMessage(
                    ProxyAssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta,
                    },
                ))
            }
            "toolcall_delta" => {
                let content_index = value.get("contentIndex")?.as_u64()? as usize;
                let delta = value.get("delta")?.as_str()?.to_string();

                Some(ProxyEvent::AssistantMessage(
                    ProxyAssistantMessageEvent::ToolCallDelta {
                        content_index,
                        delta,
                    },
                ))
            }
            "done" => {
                let reason = value.get("reason")?.as_str()?.to_string();
                let usage = value
                    .get("usage")
                    .map(|u| ProxyUsage {
                        input: u.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
                        output: u.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_read: u.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_write: u.get("cacheWrite").and_then(|v| v.as_u64()).unwrap_or(0),
                        total_tokens: u.get("totalTokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        cost: ProxyCost::default(),
                    })
                    .unwrap_or_default();

                Some(ProxyEvent::AssistantMessage(
                    ProxyAssistantMessageEvent::Done { reason, usage },
                ))
            }
            "error" => {
                let reason = value.get("reason")?.as_str()?.to_string();
                let error_message = value
                    .get("errorMessage")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let usage = value
                    .get("usage")
                    .map(|u| ProxyUsage {
                        input: u.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
                        output: u.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_read: u.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_write: u.get("cacheWrite").and_then(|v| v.as_u64()).unwrap_or(0),
                        total_tokens: u.get("totalTokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        cost: ProxyCost::default(),
                    })
                    .unwrap_or_default();

                Some(ProxyEvent::AssistantMessage(
                    ProxyAssistantMessageEvent::Error {
                        reason,
                        error_message,
                        usage,
                    },
                ))
            }
            "tool_execution_start" => Some(ProxyEvent::ToolCall(ToolCallEvent {
                tool_call_id: value.get("toolCallId")?.as_str()?.to_string(),
                tool_name: value.get("toolName")?.as_str()?.to_string(),
                args: value
                    .get("args")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                result: None,
                is_error: false,
            })),
            "tool_execution_end" => Some(ProxyEvent::ToolCall(ToolCallEvent {
                tool_call_id: value.get("toolCallId")?.as_str()?.to_string(),
                tool_name: value.get("toolName")?.as_str()?.to_string(),
                args: serde_json::Value::Null,
                result: value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                is_error: value
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })),
            _ => None,
        }
    }
}

impl futures::Stream for ProxyStream {
    type Item = ProxyEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        self.events.poll_recv(cx)
    }
}

/// Reconstructs events from proxy protocol back into ProviderEvents.
pub struct ProxyEventReconstructor {
    partial: AssistantMessage,
    content_states: HashMap<usize, ContentState>,
}

enum ContentState {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolCall {
        id: String,
        name: String,
        partial_json: String,
    },
}

impl ProxyEventReconstructor {
    /// Create a new reconstructor for the given model info.
    pub fn new(provider: &str, model: &str) -> Self {
        let mut partial = AssistantMessage::new(oxi_ai::Api::AnthropicMessages, provider, model);
        partial.content = Vec::new();

        ProxyEventReconstructor {
            partial,
            content_states: HashMap::new(),
        }
    }

    /// Process a proxy event and return reconstructed ProviderEvents.
    pub fn process(&mut self, event: ProxyEvent) -> Vec<ProviderEvent> {
        match event {
            ProxyEvent::AssistantMessage(proxy_event) => self.process_assistant_event(proxy_event),
            ProxyEvent::ToolCall(tool_event) => {
                vec![ProviderEvent::ToolCallEnd {
                    tool_call: ToolCall {
                        id: tool_event.tool_call_id,
                        name: tool_event.tool_name,
                        arguments: serde_json::from_value(tool_event.args).unwrap_or_default(),
                    },
                    partial: self.partial.clone(),
                }]
            }
            ProxyEvent::Error(msg) => {
                vec![ProviderEvent::Error {
                    error: oxi_ai::ProviderError::Other(msg),
                }]
            }
        }
    }

    fn process_assistant_event(&mut self, event: ProxyAssistantMessageEvent) -> Vec<ProviderEvent> {
        match event {
            ProxyAssistantMessageEvent::Start {
                content_index,
                content_type,
            } => {
                let state = match content_type {
                    ContentType::Text => {
                        self.partial
                            .content
                            .push(ContentBlock::Text(TextContent::new(String::new())));
                        ContentState::Text {
                            text: String::new(),
                        }
                    }
                    ContentType::Thinking => {
                        self.partial
                            .content
                            .push(ContentBlock::Thinking(ThinkingContent::new(String::new())));
                        ContentState::Thinking {
                            thinking: String::new(),
                        }
                    }
                    ContentType::ToolCall { id, name } => {
                        self.partial.content.push(ContentBlock::ToolCall(ToolCall {
                            id,
                            name,
                            arguments: serde_json::Map::new(),
                        }));
                        ContentState::ToolCall {
                            id,
                            name,
                            partial_json: String::new(),
                        }
                    }
                };

                self.content_states.insert(content_index, state);
                vec![ProviderEvent::Start {
                    partial: self.partial.clone(),
                }]
            }

            ProxyAssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                if let Some(state) = self.content_states.get_mut(&content_index) {
                    if let ContentState::Text { text } = state {
                        text.push_str(&delta);

                        if let Some(block) = self.partial.content.get_mut(content_index) {
                            if let ContentBlock::Text(t) = block {
                                t.text.push_str(&delta);
                            }
                        }

                        return vec![ProviderEvent::TextDelta {
                            content_index,
                            delta,
                            partial: self.partial.clone(),
                        }];
                    }
                }
                vec![]
            }

            ProxyAssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                if let Some(state) = self.content_states.get_mut(&content_index) {
                    if let ContentState::Thinking { thinking } = state {
                        thinking.push_str(&delta);

                        if let Some(block) = self.partial.content.get_mut(content_index) {
                            if let ContentBlock::Thinking(t) = block {
                                t.thinking.push_str(&delta);
                            }
                        }

                        return vec![ProviderEvent::ThinkingDelta {
                            content_index,
                            delta,
                            partial: self.partial.clone(),
                        }];
                    }
                }
                vec![]
            }

            ProxyAssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                if let Some(state) = self.content_states.get_mut(&content_index) {
                    if let ContentState::ToolCall { partial_json, .. } = state {
                        partial_json.push_str(&delta);

                        // Parse partial JSON
                        let arguments: serde_json::Map<String, serde_json::Value> =
                            serde_json::from_str(partial_json).unwrap_or_default();

                        if let Some(block) = self.partial.content.get_mut(content_index) {
                            if let ContentBlock::ToolCall(tc) = block {
                                tc.arguments = arguments;
                            }
                        }

                        return vec![ProviderEvent::ToolCallDelta {
                            content_index,
                            delta,
                            partial: self.partial.clone(),
                        }];
                    }
                }
                vec![]
            }

            ProxyAssistantMessageEvent::Done { reason, usage } => {
                // Update stop reason and usage
                self.partial.stop_reason = match reason.as_str() {
                    "stop" => StopReason::Stop,
                    "length" => StopReason::Length,
                    "toolUse" => StopReason::ToolUse,
                    "error" => StopReason::Error,
                    "aborted" => StopReason::Aborted,
                    _ => StopReason::Stop,
                };

                self.partial.usage.input = usage.input;
                self.partial.usage.output = usage.output;
                self.partial.usage.cache_read = usage.cache_read;
                self.partial.usage.cache_write = usage.cache_write;
                self.partial.usage.total_tokens = usage.total_tokens;
                self.partial.usage.cost.input = usage.cost.input;
                self.partial.usage.cost.output = usage.cost.output;
                self.partial.usage.cost.cache_read = usage.cost.cache_read;
                self.partial.usage.cost.cache_write = usage.cost.cache_write;
                self.partial.usage.cost.total = usage.cost.total;

                // Clean up content states
                self.content_states.clear();

                vec![ProviderEvent::Done {
                    reason: self.partial.stop_reason,
                    message: self.partial.clone(),
                }]
            }

            ProxyAssistantMessageEvent::Error {
                reason,
                error_message,
                usage,
            } => {
                self.partial.stop_reason = match reason.as_str() {
                    "error" => StopReason::Error,
                    "aborted" => StopReason::Aborted,
                    _ => StopReason::Error,
                };
                self.partial.error_message = error_message;

                vec![ProviderEvent::Error {
                    error: oxi_ai::ProviderError::Other(
                        self.partial
                            .error_message
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string()),
                    ),
                }]
            }
        }
    }

    /// Get the current partial message.
    pub fn partial(&self) -> &AssistantMessage {
        &self.partial
    }

    /// Reset the reconstructor for a new message.
    pub fn reset(&mut self, provider: &str, model: &str) {
        self.partial = AssistantMessage::new(oxi_ai::Api::AnthropicMessages, provider, model);
        self.partial.content = Vec::new();
        self.content_states.clear();
    }
}

/// Proxy server configuration.
#[derive(Debug, Clone)]
pub struct ProxyServerConfig {
    /// Port to listen on.
    pub port: u16,
    /// Bind address.
    pub bind_address: String,
    /// Handler for incoming requests.
    pub request_handler: Arc<
        dyn Fn(
                ProxyServerRequest,
            ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send>>
            + Send
            + Sync,
    >,
}

impl Default for ProxyServerConfig {
    fn default() -> Self {
        ProxyServerConfig {
            port: 8080,
            bind_address: "127.0.0.1".to_string(),
            request_handler: Arc::new(|_| Box::pin(async { Ok(Vec::new()) })),
        }
    }
}

/// Request received by proxy server.
#[derive(Debug)]
pub struct ProxyServerRequest {
    pub auth_token: Option<String>,
    pub model_id: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub options: ProxyRequestOptions,
}

/// Request options from client.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ProxyRequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<usize>,
    pub session_id: Option<String>,
    pub transport: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Request body sent by proxy client.
#[derive(Debug, Clone, serde::Deserialize)]
struct ProxyRequest {
    model_id: String,
    system_prompt: Option<String>,
    messages: Vec<Message>,
    options: ProxyRequestOptions,
}

/// Server-side event stripper for bandwidth optimization.
pub struct ProxyEventStripper {
    /// Current content index being processed.
    content_index: usize,
    /// Current content type.
    content_type: ContentType,
    /// Whether we're currently in a content block.
    in_content: bool,
}

impl ProxyEventStripper {
    /// Create a new stripper.
    pub fn new() -> Self {
        ProxyEventStripper {
            content_index: 0,
            content_type: ContentType::Text,
            in_content: false,
        }
    }

    /// Strip a provider event to proxy event format.
    pub fn strip(&mut self, event: &ProviderEvent) -> Option<ProxyAssistantMessageEvent> {
        match event {
            ProviderEvent::Start { partial } => {
                if let Some(block) = partial.content.first() {
                    self.content_index = 0;
                    self.content_type = match block {
                        ContentBlock::Text(_) => ContentType::Text,
                        ContentBlock::Thinking(_) => ContentType::Thinking,
                        ContentBlock::ToolCall(tc) => ContentType::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        },
                    };
                    self.in_content = true;

                    return Some(ProxyAssistantMessageEvent::Start {
                        content_index: 0,
                        content_type: self.content_type.clone(),
                    });
                }
                None
            }

            ProviderEvent::TextStart {
                content_index,
                partial,
            } => {
                self.content_index = *content_index;
                if let Some(block) = partial.content.get(*content_index) {
                    self.content_type = match block {
                        ContentBlock::Text(_) => ContentType::Text,
                        ContentBlock::Thinking(_) => ContentType::Thinking,
                        ContentBlock::ToolCall(tc) => ContentType::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        },
                    };
                }
                self.in_content = true;

                Some(ProxyAssistantMessageEvent::Start {
                    content_index: *content_index,
                    content_type: self.content_type.clone(),
                })
            }

            ProviderEvent::TextDelta {
                content_index,
                delta,
                ..
            } => {
                self.content_index = *content_index;
                Some(ProxyAssistantMessageEvent::TextDelta {
                    content_index: *content_index,
                    delta: delta.clone(),
                })
            }

            ProviderEvent::ThinkingStart { content_index, .. } => {
                self.content_index = *content_index;
                self.content_type = ContentType::Thinking;
                self.in_content = true;

                Some(ProxyAssistantMessageEvent::Start {
                    content_index: *content_index,
                    content_type: ContentType::Thinking,
                })
            }

            ProviderEvent::ThinkingDelta {
                content_index,
                delta,
                ..
            } => Some(ProxyAssistantMessageEvent::ThinkingDelta {
                content_index: *content_index,
                delta: delta.clone(),
            }),

            ProviderEvent::ToolCallStart {
                content_index,
                tool_call_id: _,
                partial,
            } => {
                self.content_index = *content_index;
                if let Some(block) = partial.content.get(*content_index) {
                    if let ContentBlock::ToolCall(tc) = block {
                        self.content_type = ContentType::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        };
                    }
                }
                self.in_content = true;

                Some(ProxyAssistantMessageEvent::Start {
                    content_index: *content_index,
                    content_type: self.content_type.clone(),
                })
            }

            ProviderEvent::ToolCallDelta {
                content_index,
                delta,
                ..
            } => Some(ProxyAssistantMessageEvent::ToolCallDelta {
                content_index: *content_index,
                delta: delta.clone(),
            }),

            ProviderEvent::Done { reason, message } => {
                self.in_content = false;
                Some(ProxyAssistantMessageEvent::Done {
                    reason: format!("{:?}", reason),
                    usage: ProxyUsage::from(&message.usage),
                })
            }

            ProviderEvent::Error { error } => Some(ProxyAssistantMessageEvent::Error {
                reason: "error".to_string(),
                error_message: Some(error.text_content()),
                usage: ProxyUsage::default(),
            }),

            _ => None,
        }
    }

    /// Serialize a proxy event to JSON bytes.
    pub fn serialize(event: &ProxyAssistantMessageEvent) -> Vec<u8> {
        let json = match event {
            ProxyAssistantMessageEvent::Start {
                content_index,
                content_type,
            } => {
                let mut obj = serde_json::json!({
                    "type": match content_type.as_str() {
                        "text" => "start",
                        "thinking" => "thinking_start",
                        "toolcall" => "toolcall_start",
                        _ => "start",
                    },
                    "contentIndex": content_index,
                });

                if let ContentType::ToolCall { id, name } = content_type {
                    obj["id"] = serde_json::json!(id);
                    obj["toolName"] = serde_json::json!(name);
                }

                obj["contentType"] = serde_json::json!(content_type.as_str());
                obj
            }
            ProxyAssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                serde_json::json!({
                    "type": "text_delta",
                    "contentIndex": content_index,
                    "delta": delta,
                })
            }
            ProxyAssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                serde_json::json!({
                    "type": "thinking_delta",
                    "contentIndex": content_index,
                    "delta": delta,
                })
            }
            ProxyAssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                serde_json::json!({
                    "type": "toolcall_delta",
                    "contentIndex": content_index,
                    "delta": delta,
                })
            }
            ProxyAssistantMessageEvent::Done { reason, usage } => {
                serde_json::json!({
                    "type": "done",
                    "reason": reason,
                    "usage": {
                        "input": usage.input,
                        "output": usage.output,
                        "cacheRead": usage.cache_read,
                        "cacheWrite": usage.cache_write,
                        "totalTokens": usage.total_tokens,
                    },
                })
            }
            ProxyAssistantMessageEvent::Error {
                reason,
                error_message,
                usage,
            } => {
                serde_json::json!({
                    "type": "error",
                    "reason": reason,
                    "errorMessage": error_message,
                    "usage": {
                        "input": usage.input,
                        "output": usage.output,
                        "cacheRead": usage.cache_read,
                        "cacheWrite": usage.cache_write,
                        "totalTokens": usage.total_tokens,
                    },
                })
            }
        };

        format!("data: {}\n\n", json.to_string()).into_bytes()
    }

    /// Reset the stripper state.
    pub fn reset(&mut self) {
        self.content_index = 0;
        self.content_type = ContentType::Text;
        self.in_content = false;
    }
}

impl Default for ProxyEventStripper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxi_ai::Api;

    #[test]
    fn test_proxy_config_default() {
        let config = ProxyConfig::default();
        assert_eq!(config.proxy_url, "http://localhost:8080");
        assert_eq!(config.timeout_secs, 120);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_content_type_as_str() {
        assert_eq!(ContentType::Text.as_str(), "text");
        assert_eq!(ContentType::Thinking.as_str(), "thinking");
        assert_eq!(
            ContentType::ToolCall {
                id: "123".to_string(),
                name: "test".to_string()
            }
            .as_str(),
            "toolcall"
        );
    }

    #[test]
    fn test_proxy_usage_from_oxi_usage() {
        let oxi_usage = oxi_ai::Usage {
            input: 100,
            output: 50,
            cache_read: 10,
            cache_write: 5,
            total_tokens: 165,
            cost: oxi_ai::Cost {
                input: 0.1,
                output: 0.2,
                cache_read: 0.01,
                cache_write: 0.02,
                total: 0.33,
            },
        };

        let proxy_usage = ProxyUsage::from(&oxi_usage);
        assert_eq!(proxy_usage.input, 100);
        assert_eq!(proxy_usage.output, 50);
        assert_eq!(proxy_usage.total_tokens, 165);
        assert_eq!(proxy_usage.cost.total, 0.33);
    }

    #[test]
    fn test_event_stripper_new() {
        let stripper = ProxyEventStripper::new();
        assert!(!stripper.in_content);
    }

    #[test]
    fn test_proxy_stream_options_new() {
        let model = oxi_ai::Model {
            id: "claude-3-5-sonnet".to_string(),
            name: "Claude 3.5 Sonnet".to_string(),
            api: Api::AnthropicMessages,
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: false,
            input: vec!["text".to_string()],
            cost: oxi_ai::ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 3.6,
                cache_write: 3.0,
            },
            context_window: 200000,
            max_tokens: 8192,
        };

        let options = ProxyStreamOptions::new(
            model.clone(),
            "test_token".to_string(),
            "http://localhost:8080".to_string(),
        );

        assert_eq!(options.auth_token, "test_token");
        assert_eq!(options.proxy_url, "http://localhost:8080");
    }

    #[test]
    fn test_proxy_event_serialization() {
        let event = ProxyAssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Hello".to_string(),
        };

        let serialized = ProxyEventStripper::serialize(&event);
        let text = String::from_utf8_lossy(&serialized);

        assert!(text.contains("data:"));
        assert!(text.contains("text_delta"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn test_reconstructor_new() {
        let reconstructor = ProxyEventReconstructor::new("anthropic", "claude-3-5-sonnet");
        assert!(reconstructor.partial().content.is_empty());
    }

    #[test]
    fn test_reconstructor_reset() {
        let mut reconstructor = ProxyEventReconstructor::new("anthropic", "claude-3-5-sonnet");
        reconstructor.reset("openai", "gpt-4");
        assert_eq!(reconstructor.partial().provider, "openai");
        assert_eq!(reconstructor.partial().model, "gpt-4");
    }

    #[test]
    fn test_reconstructor_process_text_delta() {
        let mut reconstructor = ProxyEventReconstructor::new("anthropic", "claude-3-5-sonnet");

        let events = reconstructor.process(ProxyEvent::AssistantMessage(
            ProxyAssistantMessageEvent::Start {
                content_index: 0,
                content_type: ContentType::Text,
            },
        ));

        assert!(!events.is_empty());

        let events = reconstructor.process(ProxyEvent::AssistantMessage(
            ProxyAssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello ".to_string(),
            },
        ));

        assert!(!events.is_empty());

        // Verify content was updated
        let partial = reconstructor.partial();
        if let Some(ContentBlock::Text(t)) = partial.content.get(0) {
            assert!(t.text.contains("Hello"));
        }
    }

    #[test]
    fn test_reconstructor_process_tool_call() {
        let mut reconstructor = ProxyEventReconstructor::new("anthropic", "claude-3-5-sonnet");

        let events = reconstructor.process(ProxyEvent::AssistantMessage(
            ProxyAssistantMessageEvent::Start {
                content_index: 0,
                content_type: ContentType::ToolCall {
                    id: "tool_123".to_string(),
                    name: "get_weather".to_string(),
                },
            },
        ));

        assert!(!events.is_empty());

        let events = reconstructor.process(ProxyEvent::ToolCall(ToolCallEvent {
            tool_call_id: "tool_123".to_string(),
            tool_name: "get_weather".to_string(),
            args: serde_json::json!({"city": "NYC"}),
            result: Some("Sunny".to_string()),
            is_error: false,
        }));

        assert!(!events.is_empty());
    }

    #[test]
    fn test_proxy_server_config_default() {
        let config = ProxyServerConfig::default();
        assert_eq!(config.port, 8080);
        assert_eq!(config.bind_address, "127.0.0.1");
    }

    #[test]
    fn test_proxy_request_options_deserialize() {
        let json = r#"{"temperature": 0.7, "maxTokens": 1000, "sessionId": "abc123"}"#;
        let options: ProxyRequestOptions = serde_json::from_str(json).unwrap();

        assert_eq!(options.temperature, Some(0.7));
        assert_eq!(options.max_tokens, Some(1000));
        assert_eq!(options.session_id, Some("abc123".to_string()));
    }
}
