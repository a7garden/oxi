# Research: pi-mono Architecture Analysis for oxi (Rust Reimplementation)

**Date:** 2026-05-02
**Question:** What is pi-mono's architecture, core concepts, and design philosophy for clean-room Rust implementation?
**Status:** Complete

---

## Executive Summary

pi-mono is a TypeScript monorepo for building AI coding agents with 5 packages. Its core value proposition is **provider-agnostic LLM abstraction**, **event-driven streaming**, and **minimal hackable design**. The project aims to be the "harness" you adapt to your workflow, not the other way around.

---

## 1. Architecture Overview

### Package Dependencies

```
pi-coding-agent (CLI harness)
    ├── pi-tui (Terminal UI)
    ├── pi-agent-core (Agent runtime)
    │   └── pi-ai (LLM abstraction)
    └── pi-ai (LLM abstraction)
```

### Core Value Chain

1. **pi-ai** provides unified LLM API access
2. **pi-agent-core** orchestrates conversation + tool execution
3. **pi-tui** renders interactive terminal interface
4. **pi-coding-agent** glues it together with session management and extensions

---

## 2. Core Domain Models

### 2.1 pi-ai Types

```typescript
// Model = provider + API + capabilities + pricing
interface Model<TApi> {
  id: string;                    // e.g., "gpt-4o-mini"
  name: string;                  // e.g., "GPT-4o Mini"
  api: TApi;                      // e.g., "openai-completions"
  provider: string;               // e.g., "openai"
  baseUrl: string;
  reasoning: boolean;              // supports reasoning/thinking
  input: ("text" | "image")[];    // input modalities
  cost: { input, output, cacheRead, cacheWrite };  // $/million tokens
  contextWindow: number;
  maxTokens: number;
}

// Context = conversation state
interface Context {
  systemPrompt?: string;
  messages: Message[];
  tools?: Tool[];
}

// Message content blocks
type ContentBlock = TextContent | ThinkingContent | ImageContent | ToolCall;
```

### 2.2 pi-agent-core Types

```typescript
// AgentTool = definition + execution
interface AgentTool<T> {
  name: string;
  label: string;
  description: string;
  parameters: TSchema;
  execute: (toolCallId, params, signal, onUpdate) => Promise<AgentToolResult>;
}

// AgentState
interface AgentState {
  systemPrompt: string;
  model: Model<any>;
  thinkingLevel: ThinkingLevel;
  tools: AgentTool<any>[];
  messages: AgentMessage[];
  isStreaming: boolean;
  streamingMessage?: AgentMessage;
  pendingToolCalls: ReadonlySet<string>;
}
```

### 2.3 Event System

```typescript
// pi-ai streaming events
type AssistantMessageEvent =
  | { type: "start"; partial: AssistantMessage }
  | { type: "text_delta"; delta: string; ... }
  | { type: "toolcall_delta"; delta: string; ... }
  | { type: "done"; message: AssistantMessage }
  | { type: "error"; reason: "error" | "aborted"; error: AssistantMessage };

// pi-agent-core agent events
type AgentEvent =
  | { type: "agent_start" }
  | { type: "turn_start" }
  | { type: "message_start"; message: AgentMessage }
  | { type: "message_update"; message: AgentMessage; assistantMessageEvent: ... }
  | { type: "message_end"; message: AgentMessage }
  | { type: "tool_execution_start"; toolCallId: string; toolName: string; args: any }
  | { type: "tool_execution_end"; toolCallId: string; toolName: string; result: any; isError: boolean }
  | { type: "turn_end"; message: AgentMessage; toolResults: ToolResultMessage[] }
  | { type: "agent_end"; messages: AgentMessage[] };
```

---

## 3. Key Design Patterns

### 3.1 Provider Abstraction

- **Registry pattern**: `registerApiProvider()` with lazy loading
- **Capability-based**: Models expose capabilities (reasoning, vision, etc.)
- **Compatibility overrides**: `compat` field for OpenAI-compatible variations

### 3.2 Event-Driven Streaming

- Async generators for streaming responses
- Delta events for real-time UI updates
- AbortSignal for cancellation

### 3.3 Tool Execution

- **Two modes**: sequential vs parallel
- **Hooks**: `beforeToolCall`, `afterToolCall`
- **Validation**: TypeBox schema validation before execution
- **Streaming**: Tools can emit progress updates

### 3.4 State Management

- Immutable-style updates with copy semantics
- Context transformation before LLM calls
- Steering/follow-up queues for message injection

### 3.5 Extension System (coding-agent)

```typescript
interface ExtensionAPI {
  registerTool(tool: AgentTool): void;
  registerCommand(name: string, config: CommandConfig): void;
  on(event: string, handler: EventHandler): void;
  // ...
}
```

---

## 4. Technology Stack Mapping

| pi-mono (TypeScript/Node.js) | Rust Equivalent |
|----------------------------|----------------|
| TypeScript | Rust |
| async generators | futures::Stream, async iteration |
| TypeBox (JSON Schema) | schemars, json_schema |
| SSE streaming | reqwest + bytes::Bytes |
| Readline/ANSI | crossterm, ratatui |
| EventEmitter pattern | tokio::sync::broadcast/mpsc |
| AbortController | tokio::sync::watch, CancellationToken |
| fs (Node.js) | tokio::fs |
| ReadableStream | futures::StreamExt |
| TypeScript types | Rust traits + derive macros |

### Key Rust Crates

| Purpose | Crate |
|--------|-------|
| Async runtime | tokio |
| HTTP client | reqwest |
| Serialization | serde, serde_json |
| JSON Schema | schemars |
| Terminal UI | crossterm, ratatui |
| CLI args | clap |
| Logging | tracing |
| Config | config-rs, toml |
| Async traits | async-trait |
| JSONL files | tokio::io::AsyncBufReadExt |

---

## 5. Core Ideas to Preserve in oxi

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

### Phase 1: oxi-ai (LLM Abstraction) - HIGHEST PRIORITY

**Goals:**
- [ ] Core types (Model, Context, Message, Tool)
- [ ] Provider trait/abstractions
- [ ] Event streaming infrastructure
- [ ] OpenAI-compatible provider (baseline)
- [ ] Anthropic provider
- [ ] JSON Schema validation for tools
- [ ] Token/cost tracking

**Rust Structure:**
```
oxi-ai/
├── src/
│   ├── lib.rs
│   ├── types.rs          # Core domain types
│   ├── model.rs          # Model registry
│   ├── context.rs        # Context management
│   ├── providers/        # Provider implementations
│   │   ├── mod.rs
│   │   ├── trait.rs      # Provider trait
│   │   ├── openai.rs
│   │   └── anthropic.rs
│   ├── events.rs         # Event types
│   └── tools.rs          # Tool definitions + validation
```

### Phase 2: oxi-agent (Agent Runtime) - HIGH PRIORITY

**Goals:**
- [ ] Agent state management
- [ ] Event system with channels
- [ ] Tool execution loop
- [ ] Context transformation
- [ ] Steering/follow-up queues

**Rust Structure:**
```
oxi-agent/
├── src/
│   ├── lib.rs
│   ├── agent.rs           # Agent struct + methods
│   ├── state.rs          # AgentState
│   ├── loop.rs           # agent loop implementation
│   ├── events.rs         # AgentEvent types
│   └── tools.rs          # AgentTool trait
```

### Phase 3: oxi-tui (Terminal UI)

**Goals:**
- [ ] Core TUI framework
- [ ] Differential rendering
- [ ] Built-in components (Text, Input, Editor, etc.)
- [ ] Overlay system
- [ ] Theme support

**Rust Structure:**
```
oxi-tui/
├── src/
│   ├── lib.rs
│   ├── tui.rs            # Main TUI
│   ├── component.rs      # Component trait
│   ├── components/       # Built-in components
│   │   ├── mod.rs
│   │   ├── text.rs
│   │   ├── input.rs
│   │   ├── editor.rs
│   │   └── ...
│   ├── overlay.rs        # Overlay system
│   ├── theme.rs         # Theme definitions
│   └── render.rs        # Differential rendering
```

### Phase 4: oxi (CLI Harness)

**Goals:**
- [ ] Session management (JSONL)
- [ ] Built-in tools (read, write, edit, bash)
- [ ] Extension system
- [ ] Package management
- [ ] Settings system

**Rust Structure:**
```
oxi/
├── src/
│   ├── lib.rs
│   ├── main.rs           # CLI entry
│   ├── session.rs        # Session management
│   ├── tools/           # Built-in tools
│   ├── extensions.rs    # Extension API
│   └── settings.rs      # Configuration
```

---

## 7. Open Questions

1. **WASM support?** Browser usage would require WASM builds. Consider as post-MVP.
2. **Plugin system?** Dynamic loading (libloading) vs compile-time features?
3. **Persistence format?** Keep JSONL or consider alternative (SQLite, sled)?
4. **OAuth handling?** Proxy required or native implementation?
5. **Async ecosystem?** Tokio is standard, but async-std might be simpler?

---

## 8. Recommendation

**Start with oxi-ai** as it's the foundation everything else builds on. The core abstractions are clear:

1. **Provider trait** with `stream()` returning `impl Stream<Item = Event>`
2. **Model registry** with JSON Schema for tool validation
3. **Event types** matching pi-ai's streaming protocol

**Key Rust idioms to apply:**
- Use `async_trait` for provider trait objects
- Use `Pin<Box<dyn Stream>>` for erased streams
- Use `serde_json::Value` for flexible JSON handling
- Use `schemars::Schema` for JSON Schema generation
- Use `tracing` for structured logging
- Use `anyhow` for error handling

---

## Sources

| Source | Type | Location |
|--------|------|----------|
| pi-mono monorepo | Primary | pi-mono/ |
| AGENTS.md | Primary | pi-mono/AGENTS.md |
| ai/types.ts | Primary | pi-mono/packages/ai/src/types.ts |
| agent/types.ts | Primary | pi-mono/packages/agent/src/types.ts |
| Package READMEs | Primary | pi-mono/packages/*/README.md |
