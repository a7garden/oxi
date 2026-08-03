# Design: oxicode — A Rust Coding Agent

**Date:** 2026-05-02
**Status:** Draft

---

## Overview

oxicode is a terminal-based AI coding assistant built in Rust. It is inspired by modern agent architectures and provides provider-agnostic LLM access, event-driven streaming, and an extensible tool system.

---

## Package Architecture

```
oxicode (CLI harness)
├── oxicode-ai (LLM abstraction layer)
│   ├── Model registry + provider trait
│   ├── Streaming event system
│   ├── Tool definitions + validation
│   └── Token/cost tracking
├── oxicode-agent (Agent runtime)
│   ├── State management
│   ├── Tool execution loop
│   └── Event emitters
├── oxicode-tui (Terminal UI)
│   ├── Component framework
│   ├── Differential rendering
│   └── Built-in components
└── oxicode (Harness)
    ├── Session management
    ├── Built-in tools
    └── Extension system
```

---

## 1. oxicode-ai: LLM Abstraction Layer

### Core Types

```rust
// src/types.rs

/// Provider API identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Api {
    OpenAiCompletions,
    OpenAiResponses,
    AnthropicMessages,
    GoogleGenerativeAi,
    GoogleVertex,
    MistralConversations,
    AzureOpenAiResponses,
    BedrockConverseStream,
}

/// Provider name
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Provider(pub String);

/// Model thinking/reasoning level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

/// Input modalities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputModality {
    Text,
    Image,
}

/// Cost structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,        // $/million tokens
    pub output: f64,       // $/million tokens
    pub cache_read: f64,   // $/million tokens
    pub cache_write: f64,  // $/million tokens
}

/// LLM model definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: String,
    pub base_url: String,
    pub reasoning: bool,
    pub input: Vec<InputModality>,
    pub cost: Cost,
    pub context_window: usize,
    pub max_tokens: usize,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    // Provider-specific compatibility settings
    #[serde(default)]
    pub compat: Option<CompatSettings>,
}

/// Compatibility settings for OpenAI-compatible APIs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompatSettings {
    #[serde(default = "default_true")]
    pub supports_store: bool,
    #[serde(default = "default_true")]
    pub supports_developer_role: bool,
    #[serde(default = "default_true")]
    pub supports_reasoning_effort: bool,
    // ... other compat flags
}

fn default_true() -> bool { true }
```

### Message Types

```rust
// src/messages.rs

/// Text content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub content_type: TextContentType,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "text")]
pub enum TextContentType { Text }

/// Thinking content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingContent {
    #[serde(rename = "type")]
    pub content_type: ThinkingContentType,
    pub thinking: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "thinking")]
pub enum ThinkingContentType { Thinking }

/// Image content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    #[serde(rename = "type")]
    pub content_type: ImageContentType,
    pub data: String,        // base64
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "image")]
pub enum ImageContentType { Image }

/// Tool call content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(rename = "type")]
    pub content_type: ToolCallType,
    pub id: String,
    pub name: String,
    pub arguments: Value,    // JSON object
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "toolCall")]
pub enum ToolCallType { ToolCall }

/// Content block union
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    Image(ImageContent),
    ToolCall(ToolCall),
}

/// Token usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input: usize,
    pub output: usize,
    pub cache_read: usize,
    pub cache_write: usize,
    pub total_tokens: usize,
    pub cost: Cost,
}

/// Stop reason
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// User message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub role: UserRole,
    pub content: MessageContent,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "user")]
pub enum UserRole { User }

/// Assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub role: AssistantRole,
    pub content: Vec<ContentBlock>,
    pub api: Api,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "assistant")]
pub enum AssistantRole { Assistant }

/// Tool result message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub role: ToolResultRole,
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub is_error: bool,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "toolResult")]
pub enum ToolResultRole { ToolResult }

/// Message union
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

/// Message content (string or content blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}
```

### Tool Definitions

```rust
// src/tools.rs

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tool definition with JSON Schema parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,  // JSON Schema
}

/// Tool argument validation result
pub type ValidatedArgs = Value;

/// Validate tool arguments against schema
pub fn validate_args(tool: &Tool, args: &Value) -> Result<ValidatedArgs, ValidationError> {
    // Use jsonschema crate for validation
}
```

### Context

```rust
// src/context.rs

use super::{Message, Tool};

/// Conversation context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<Tool>,
}

impl Context {
    pub fn new() -> Self {
        Self {
            system_prompt: None,
            messages: Vec::new(),
            tools: Vec::new(),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }
}
```

### Provider Trait

```rust
// src/providers/mod.rs

use async_trait::async_trait;
use futures::Stream;
use super::{Context, Model, StreamOptions};

/// Streaming events from the provider
#[derive(Debug)]
pub enum ProviderEvent {
    Start { partial: AssistantMessage },
    TextStart { content_index: usize, partial: AssistantMessage },
    TextDelta { content_index: usize, delta: String, partial: AssistantMessage },
    TextEnd { content_index: usize, content: String, partial: AssistantMessage },
    ThinkingStart { content_index: usize, partial: AssistantMessage },
    ThinkingDelta { content_index: usize, delta: String, partial: AssistantMessage },
    ThinkingEnd { content_index: usize, content: String, partial: AssistantMessage },
    ToolCallStart { content_index: usize, partial: AssistantMessage },
    ToolCallDelta { content_index: usize, delta: String, partial: AssistantMessage },
    ToolCallEnd { content_index: usize, tool_call: ToolCall, partial: AssistantMessage },
    Done { reason: StopReason, message: AssistantMessage },
    Error { reason: StopReason, error: AssistantMessage },
}

/// Provider streaming options
#[derive(Debug, Clone)]
pub struct StreamOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<usize>,
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    // ... other options
}

/// Cache retention preference
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

/// LLM provider trait
#[async_trait]
pub trait Provider: Send + Sync {
    /// Stream assistant message events
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<impl Stream<Item = ProviderEvent> + Send, ProviderError>;
}
```

### OpenAI Provider Implementation

```rust
// src/providers/openai.rs

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use super::{Context, Model, Provider, ProviderEvent, ProviderError, StreamOptions};
use crate::types::*;

pub struct OpenAiProvider {
    client: Client,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<impl Stream<Item = ProviderEvent> + Send, ProviderError> {
        // Implementation
    }
}
```

### Event Stream API

```rust
// src/lib.rs

pub mod types;
pub mod messages;
pub mod context;
pub mod tools;
pub mod providers;

pub use types::*;
pub use messages::*;
pub use context::Context;
pub use tools::{Tool, validate_args};
pub use providers::{Provider, ProviderEvent, StreamOptions};

// High-level streaming API
pub async fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<impl Stream<Item = ProviderEvent> + Send, ProviderError> {
    // Get provider from registry and call stream()
}
```

---

## 2. oxicode-agent: Agent Runtime

### Core Types

```rust
// src/types.rs

use super::{AgentToolResult, ToolCall};

/// Agent tool result
pub struct AgentToolResult<T = Value> {
    pub content: Vec<ContentBlock>,
    pub details: T,
    pub terminate: bool,
}

/// Agent tool definition
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> &Value;
    
    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<AbortSignal>,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send>>,
    ) -> Result<AgentToolResult, ToolError>;
}

/// Tool execution mode
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

/// Agent event types
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart { message: AgentMessage },
    MessageUpdate { message: AgentMessage, assistant_message_event: ProviderEvent },
    MessageEnd { message: AgentMessage },
    ToolExecutionStart { tool_call_id: String, tool_name: String, args: Value },
    ToolExecutionUpdate { tool_call_id: String, tool_name: String, args: Value, partial_result: Value },
    ToolExecutionEnd { tool_call_id: String, tool_name: String, result: Value, is_error: bool },
    TurnEnd { message: AgentMessage, tool_results: Vec<ToolResultMessage> },
    AgentEnd { messages: Vec<AgentMessage> },
}

/// Agent state
pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: Arc<RwLock<HashSet<String>>>,
}
```

### Agent Struct

```rust
// src/agent.rs

use tokio::sync::{broadcast, mpsc};
use futures::StreamExt;

pub struct Agent {
    state: RwLock<AgentState>,
    event_tx: broadcast::Sender<AgentEvent>,
    config: AgentConfig,
}

pub struct AgentConfig {
    pub model: Model,
    pub system_prompt: String,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub tool_execution: ToolExecutionMode,
    pub before_tool_call: Option<Box<dyn Fn(BeforeToolCallCtx) -> BeforeToolCallResult + Send + Sync>>,
    pub after_tool_call: Option<Box<dyn Fn(AfterToolCallCtx) -> AfterToolCallResult + Send + Sync>>,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            state: RwLock::new(AgentState::new(config)),
            event_tx,
            config,
        }
    }

    pub async fn prompt(&self, content: impl Into<MessageContent>) -> Result<(), AgentError> {
        let message = UserMessage {
            role: UserRole::User,
            content: content.into(),
            timestamp: Utc::now().timestamp_millis(),
        };
        self.run_loop(vec![AgentMessage::User(message)]).await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    async fn run_loop(&self, initial_messages: Vec<AgentMessage>) -> Result<(), AgentError> {
        // Main agent loop implementation
    }
}
```

### Tool Execution

```rust
// src/tool_execution.rs

pub async fn execute_tools(
    tool_calls: Vec<ToolCall>,
    tools: &[Arc<dyn AgentTool>],
    mode: ToolExecutionMode,
    before: Option<&Hook>,
    after: Option<&Hook>,
    event_tx: &broadcast::Sender<AgentEvent>,
) -> Result<Vec<ToolResultMessage>, AgentError> {
    match mode {
        ToolExecutionMode::Sequential => {
            // Execute one by one
        }
        ToolExecutionMode::Parallel => {
            // Execute concurrently
        }
    }
}
```

---

## 3. oxicode-tui: Terminal UI

### Core Framework

```rust
// src/lib.rs

pub mod component;
pub mod tui;
pub mod components;

pub use component::{Component, Focusable};
pub use tui::TUI;
pub use components::*;
```

### Component Trait

```rust
// src/component.rs

/// Component interface
pub trait Component: Send {
    /// Render the component to lines
    fn render(&self, width: usize) -> Vec<String>;
    
    /// Handle keyboard input when focused
    fn handle_input(&mut self, data: &str) -> bool { false }
    
    /// Invalidate cached render state
    fn invalidate(&mut self) {}
}

/// Focusable components (for IME support)
pub trait Focusable: Component {
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
}
```

### TUI Main

```rust
// src/tui.rs

use crossterm::{Terminal, Event, ExecutableCommand};
use tokio::sync::mpsc;

pub struct TUI {
    terminal: Terminal,
    children: Vec<Box<dyn Component>>,
    focus_index: usize,
    overlay_stack: Vec<OverlayHandle>,
    dirty: bool,
}

impl TUI {
    pub fn new() -> Result<Self, TuiError> {
        let terminal = Terminal::new()?;
        Ok(Self {
            terminal,
            children: Vec::new(),
            focus_index: 0,
            overlay_stack: Vec::new(),
            dirty: true,
        })
    }

    pub fn add_child(&mut self, component: impl Component + 'static) -> usize {
        self.children.push(Box::new(component));
        self.dirty = true;
        self.children.len() - 1
    }

    pub fn set_focus(&mut self, index: usize) {
        self.focus_index = index;
    }

    pub fn start(&mut self) -> Result<(), TuiError> {
        self.terminal.enter_raw_mode()?;
        self.terminal.hide_cursor()?;
        
        loop {
            self.render_if_needed()?;
            
            // Wait for input
            if let Some(event) = self.read_event()? {
                self.handle_event(event)?;
            }
        }
    }

    fn render_if_needed(&mut self) -> Result<(), TuiError> {
        if !self.dirty {
            return Ok(());
        }
        
        // Differential rendering
        self.render()?;
        self.dirty = false;
        Ok(())
    }

    fn render(&mut self) -> Result<(), TuiError> {
        let width = self.terminal.columns() as usize;
        let mut all_lines = Vec::new();
        
        for child in &self.children {
            all_lines.extend(child.render(width));
        }
        
        // Synchronized output for flicker-free rendering
        print!("\x1b[?2026h");  // CSI 2026 synchronized update start
        self.terminal.clear_screen()?;
        for line in &all_lines {
            println!("{}", line);
        }
        print!("\x1b[?2026l");  // CSI 2026 synchronized update end
        
        Ok(())
    }
}
```

### Built-in Components

```rust
// src/components/mod.rs

mod text;
mod input;
mod editor;
mod markdown;
mod select_list;

pub use text::Text;
pub use input::Input;
pub use editor::Editor;
pub use markdown::Markdown;
pub use select_list::{SelectList, SelectItem};
```

---

## 4. oxicode: CLI Harness

### Session Management

```rust
// src/session.rs

use tokio::io::AsyncBufReadExt;
use serde::{Serialize, Deserialize};

/// Session entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    pub message: AgentMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Session manager
pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    pub async fn save(&self, session: &[SessionEntry]) -> Result<(), SessionError> {
        // Write as JSONL
    }

    pub async fn load(&self, id: Uuid) -> Result<Vec<SessionEntry>, SessionError> {
        // Read from JSONL
    }
}
```

### Built-in Tools

```rust
// src/tools/mod.rs

mod read;
mod write;
mod edit;
mod bash;

pub use read::ReadTool;
pub use write::WriteTool;
pub use edit::EditTool;
pub use bash::BashTool;
```

---

## 5. Error Handling

```rust
// src/error.rs

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OxicodeError {
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    
    #[error("Agent error: {0}")]
    Agent(#[from] AgentError),
    
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    
    #[error("Validation error: {0}")]
    Validation(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

---

## 6. Implementation Checklist

### Phase 1: oxicode-ai

- [ ] Core types (types.rs, messages.rs)
- [ ] Context management
- [ ] Tool definitions + validation
- [ ] Provider trait
- [ ] Event types
- [ ] OpenAI provider implementation
- [ ] Anthropic provider implementation
- [ ] Model registry

### Phase 2: oxicode-agent

- [ ] AgentState
- [ ] Agent struct
- [ ] Event system
- [ ] Tool execution loop
- [ ] Context transformation
- [ ] Hooks (before/after)

### Phase 3: oxicode-tui

- [ ] TUI framework
- [ ] Component trait
- [ ] Differential rendering
- [ ] Text component
- [ ] Input component
- [ ] Editor component
- [ ] Overlay system

### Phase 4: oxicode

- [ ] Session management
- [ ] Built-in tools
- [ ] Extension API
- [ ] CLI entry point
- [ ] Settings

---

## 7. Crate Dependencies

```toml
[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }
futures = "0.3"
async-trait = "0.1"

# HTTP client
reqwest = { version = "0.12", features = ["json", "stream"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# JSON Schema
schemars = "0.8"
jsonschema = "0.26"

# Terminal UI
crossterm = "0.28"
ratatui = "0.28"

# CLI
clap = { version = "4", features = ["derive"] }

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Error handling
anyhow = "1"
thiserror = "2"
anyhow-context = "0.2"

# Utilities
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
once_cell = "1"
parking_lot = "0.12"
```

---

## 8. Files Structure

```
oxicode/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   └── main.rs
├── oxicode-ai/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── types.rs
│       ├── messages.rs
│       ├── context.rs
│       ├── tools.rs
│       ├── providers/
│       │   ├── mod.rs
│       │   ├── trait.rs
│       │   ├── openai.rs
│       │   └── anthropic.rs
│       └── error.rs
├── oxicode-agent/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── agent.rs
│       ├── state.rs
│       ├── loop.rs
│       ├── events.rs
│       ├── tools.rs
│       ├── tool_execution.rs
│       └── error.rs
├── oxicode-tui/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── tui.rs
│       ├── component.rs
│       ├── render.rs
│       ├── overlay.rs
│       ├── theme.rs
│       └── components/
│           ├── mod.rs
│           ├── text.rs
│           ├── input.rs
│           ├── editor.rs
│           ├── markdown.rs
│           └── select_list.rs
└── oxicode-cli/
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── main.rs
        ├── session.rs
        ├── tools/
        │   ├── mod.rs
        │   ├── read.rs
        │   ├── write.rs
        │   ├── edit.rs
        │   └── bash.rs
        ├── extensions.rs
        └── settings.rs
```
