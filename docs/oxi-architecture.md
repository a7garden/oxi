# Architecture: oxicode

**Date:** 2026-05-02
**Status:** Active

---

## Executive Summary

oxicode is a Rust-based terminal AI coding assistant with a modular architecture. Its core value proposition is **provider-agnostic LLM abstraction**, **event-driven streaming**, and **extensible minimalism**. The project aims to be the "harness" you adapt to your workflow, not the other way around.

---

## 1. Architecture Overview

### Package Architecture

```
oxicode (CLI harness)
├── oxicode-tui (Terminal UI)
├── oxicode-agent (Agent runtime)
│   └── oxicode-ai (LLM abstraction)
└── oxicode-ai (LLM abstraction)
```

### Core Value Chain

1. **oxicode-ai** provides unified LLM API access
2. **oxicode-agent** orchestrates conversation + tool execution
3. **oxicode-tui** renders interactive terminal interface
4. **oxicode** glues it together with session management and extensions

---

## 2. Core Domain Models

### 2.1 oxicode-ai Types

```rust
// Model = provider + API + capabilities + pricing
pub struct Model {
    pub id: String,                    // e.g., "gpt-4o-mini"
    pub name: String,                  // e.g., "GPT-4o Mini"
    pub api: Api,                      // e.g., Api::OpenAiCompletions
    pub provider: String,              // e.g., "openai"
    pub base_url: String,
    pub reasoning: bool,               // supports reasoning/thinking
    pub input: Vec<InputModality>,     // input modalities
    pub cost: Cost,                    // $/million tokens
    pub context_window: usize,
    pub max_tokens: usize,
}

// Context = conversation state
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
}

// Message content blocks
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    Image(ImageContent),
    ToolCall(ToolCall),
}
```

### 2.2 oxicode-agent Types

```rust
// AgentTool = definition + execution
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, tool_call_id: &str, params: Value, signal: Option<oneshot::Receiver<()>>) -> Result<AgentToolResult, ToolError>;
}

// AgentState
pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: ReadonlySet<String>,
}
```

### 2.3 Event System

```rust
// oxicode-ai streaming events
pub enum ProviderEvent {
    TextDelta { delta: String, ... },
    ThinkingDelta { delta: String, ... },
    ToolCallDelta { delta: String, ... },
    Done { message: AssistantMessage },
    Error { error: String, ... },
}

// oxicode-agent agent events
pub enum AgentEvent {
    Start,
    Thinking,
    TextChunk { text: String },
    ToolCall { tool_call: ToolCall },
    ToolStart { tool_call_id: String, tool_name: String },
    ToolProgress { tool_call_id: String, message: String },
    ToolComplete { tool_call_id: String, result: Value },
    ToolError { tool_call_id: String, error: String },
    Complete { content: String, stop_reason: StopReason },
    Error { message: String },
}
```

---

## 3. Key Design Patterns

### 3.1 Provider Abstraction

- **Registry pattern**: `ModelRegistry` with lazy loading
- **Capability-based**: Models expose capabilities (reasoning, vision, etc.)
- **Compatibility overrides**: `compat` field for OpenAI-compatible variations

### 3.2 Event-Driven Streaming

- `futures::Stream` for streaming responses
- Delta events for real-time UI updates
- `AbortSignal` for cancellation

### 3.3 Tool Execution

- **Two modes**: sequential vs parallel
- **Hooks**: `before_tool_call`, `after_tool_call`
- **Validation**: JSON Schema validation before execution
- **Streaming**: Tools can emit progress updates

### 3.4 State Management

- Immutable-style updates with copy semantics
- Context transformation before LLM calls
- Steering/follow-up queues for message injection

### 3.5 Extension System

```rust
pub trait Extension {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn register_tools(&self, registry: &mut ToolRegistry);
    fn on_event(&self, event: &AgentEvent);
}
```

---

## 4. Technology Stack

| Component | Technology |
|-----------|------------|
| Language | Rust |
| Async runtime | tokio |
| HTTP client | reqwest |
| Serialization | serde, serde_json |
| JSON Schema | schemars |
| Terminal UI | crossterm, ratatui |
| CLI args | clap |
| Logging | tracing |
| Config | toml |
| Async traits | async-trait |

---

## 5. Core Design Principles

### 5.1 Provider Agnosticism
- Single interface for all LLM providers
- Seamless cross-provider handoffs
- Model registry with capability metadata

### 5.2 Event-Driven Architecture
- Streaming with delta events
- Real-time UI updates without flicker
- Async iteration throughout

### 5.3 Tool Calling
- Type-safe schema definitions (JSON Schema)
- Argument validation
- Parallel/sequential execution modes
- Before/after hooks for interception

### 5.4 Context Serialization
- JSON-serializable conversation state
- Cross-provider portability
- Session persistence (JSONL)

### 5.5 Hackable Minimalism
- Extension points without forking
- Skills system for capability injection
- Theme customization

---

## 6. Implementation Phases

### Phase 1: oxicode-ai (LLM Abstraction) - HIGHEST PRIORITY

**Goals:**
- [ ] Core types (Model, Context, Message, Tool)
- [ ] Provider trait/abstractions
- [ ] Event streaming infrastructure
- [ ] OpenAI-compatible provider (baseline)
- [ ] Anthropic provider
- [ ] JSON Schema validation for tools
- [ ] Token/cost tracking

### Phase 2: oxicode-agent (Agent Runtime) - HIGH PRIORITY

**Goals:**
- [ ] Agent state management
- [ ] Event system with channels
- [ ] Tool execution loop
- [ ] Context transformation
- [ ] Steering/follow-up queues

### Phase 3: oxicode-tui (Terminal UI)

**Goals:**
- [ ] Core TUI framework
- [ ] Differential rendering
- [ ] Built-in components (Text, Input, Editor, etc.)
- [ ] Overlay system
- [ ] Theme support

### Phase 4: oxicode (CLI Harness)

**Goals:**
- [ ] Session management (JSONL)
- [ ] Built-in tools (read, write, edit, bash)
- [ ] Extension system
- [ ] Package management
- [ ] Settings system

---

## 7. Open Questions

1. **WASM support?** Browser usage would require WASM builds. Consider as post-MVP.
2. **Plugin system?** Dynamic loading (libloading) vs compile-time features?
3. **Persistence format?** Keep JSONL or consider alternative (SQLite, sled)?
4. **OAuth handling?** Proxy required or native implementation?
5. **Async ecosystem?** Tokio is standard, but async-std might be simpler?

---

## 8. Key Rust Idioms

**Start with oxicode-ai** as it's the foundation everything else builds on. The core abstractions are:

1. **Provider trait** with `stream()` returning `impl Stream<Item = Event>`
2. **Model registry** with JSON Schema for tool validation
3. **Event types** matching the streaming protocol

**Key Rust idioms applied:**
- Use `async_trait` for provider trait objects
- Use `Pin<Box<dyn Stream>>` for erased streams
- Use `serde_json::Value` for flexible JSON handling
- Use `schemars::Schema` for JSON Schema generation
- Use `tracing` for structured logging
- Use `anyhow` for error handling