<div align="center">

# oxi-agent

**Agent runtime for Rust** — tool-calling loop, event system, context compaction, and MCP client.

[![Version](https://img.shields.io/badge/Version-0.20.0-blue?style=flat-square)](https://crates.io/search?q=oxi)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)
[![Docs](https://img.shields.io/docsrs/oxi-agent/latest?style=flat-square)](https://docs.rs/oxi-agent)

</div>

---

## Overview

`oxi-agent` provides the core agent loop that drives LLM interactions:

1. Sends a user prompt to the LLM
2. Streams the response as `AgentEvent`s
3. If the LLM requests a tool call, executes the tool and feeds the result back
4. Repeats until the LLM produces a final response
5. Emits events for every step (thinking, text, tool calls, completion)

### Key Concepts

- **Agent** — the main runtime that holds a provider, config, tool registry, and shared state
- **AgentTool** — trait for defining tools the LLM can invoke
- **AgentEvent** — streaming events emitted during execution
- **ToolRegistry** — manages available tools and dispatches calls
- **Compaction** — automatic context compaction when conversations get too long

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
oxi-agent = { path = "path/to/oxi-agent" }
```

Basic usage:

```rust
use std::sync::Arc;
use oxi_agent::{Agent, AgentConfig, AgentEvent};
use oxi_ai::get_provider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = get_provider("anthropic")
        .expect("provider not found");

    let config = AgentConfig {
        name: "my-agent".into(),
        description: Some("A helpful agent".into()),
        model_id: "anthropic/claude-sonnet-4-20250514".into(),
        system_prompt: Some("You are helpful.".into()),
        max_iterations: 10,
        timeout_seconds: 300,
        temperature: None,
        max_tokens: None,
        compaction_strategy: oxi_ai::CompactionStrategy::Disabled,
        compaction_instruction: None,
        context_window: 128_000,
    };

    let agent = Agent::new(Arc::from(provider), config);

    // Run with event channel
    let (response, events) = agent.run("Explain Rust ownership".into()).await?;
    println!("{}", response.content);

    Ok(())
}
```

### Streaming Events

```rust
// Run with streaming callback
agent.run_streaming("Hello!".into(), |event| match event {
    AgentEvent::TextChunk { text } => print!("{}", text),
    AgentEvent::Thinking => print!("..."),
    AgentEvent::Complete { content, .. } => println!("\nDone: {}", content),
    AgentEvent::ToolCall { tool_call } => {
        println!("Calling tool: {} ({})", tool_call.name, tool_call.id);
    }
    _ => {}
}).await?;
```

### Channel-Based Events

```rust
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);

tokio::spawn(async move {
    agent.run_with_channel("Hello!".into(), tx).await
});

while let Some(event) = rx.recv().await {
    // Handle events as they arrive
}
```

## Tool Definition Guide

### The AgentTool Trait

All tools implement the `AgentTool` trait:

```rust
use async_trait::async_trait;
use oxi_agent::{AgentTool, AgentToolResult};
use serde_json::Value;
use tokio::sync::oneshot;

pub struct MyTool;

#[async_trait]
impl AgentTool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn label(&self) -> &str { "My Tool" }
    fn description(&self) -> &str { "Does something useful" }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "The input to process"
                }
            },
            "required": ["input"]
        })
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
    ) -> Result<AgentToolResult, String> {
        let input = params["input"].as_str().ok_or("missing input")?;
        Ok(AgentToolResult::success(format!("Processed: {}", input)))
    }
}
```

### Registering Tools

```rust
use oxi_agent::ToolRegistry;

let registry = ToolRegistry::new();
registry.register(MyTool);

// Or with all built-in tools
let registry = ToolRegistry::with_builtins();
// Registers: ReadTool, WriteTool, EditTool, BashTool

// Register via the agent
agent.add_tool(MyTool);
```

### Built-in Tools

| Tool | Name | Description |
|------|------|-------------|
| `ReadTool` | `read` | Read file contents |
| `WriteTool` | `write` | Write content to a file |
| `EditTool` | `edit` | Make targeted edits to files |
| `BashTool` | `bash` | Execute shell commands |

### Tool Results

```rust
// Success
AgentToolResult::success("File contents here")

// Error
AgentToolResult::error("File not found")

// With metadata
AgentToolResult::success("ok")
    .with_metadata(serde_json::json!({"lines": 42}))
```

### Progress Callbacks

Tools can emit progress updates during long-running operations:

```rust
use oxi_agent::tools::ProgressCallback;
use std::sync::Arc;

fn on_progress(message: String) {
    println!("Progress: {}", message);
}

let callback: ProgressCallback = Arc::new(on_progress);
tool.on_progress(callback);
```

## Event System

### AgentEvent Variants

| Event | Fields | Description |
|-------|--------|-------------|
| `Start` | `prompt` | Agent begins processing |
| `Thinking` | — | LLM is reasoning |
| `TextChunk` | `text` | Incremental text output |
| `ToolCall` | `tool_call` | LLM requests tool execution |
| `ToolStart` | `tool_call_id`, `tool_name` | Tool execution begins |
| `ToolProgress` | `tool_call_id`, `message` | Tool progress update |
| `ToolComplete` | `result` | Tool finished |
| `ToolError` | `tool_call_id`, `error` | Tool failed |
| `Complete` | `content`, `stop_reason` | Response finished |
| `Error` | `message` | Error occurred |
| `Iteration` | `number` | Agent loop iteration completed |
| `Usage` | `input_tokens`, `output_tokens` | Token usage update |
| `Compaction` | `event` | Context compaction event |

### Compaction Events

When context compaction is enabled, the agent emits `Compaction` sub-events:

```rust
AgentEvent::Compaction { event } => match event {
    CompactionEvent::Triggered { context_tokens, iteration } => { /* ... */ }
    CompactionEvent::Started { message_count } => { /* ... */ }
    CompactionEvent::Completed { result, duration_ms } => { /* ... */ }
    CompactionEvent::Failed { error } => { /* ... */ }
}
```

## Agent Configuration

```rust
pub struct AgentConfig {
    pub name: String,
    pub description: Option<String>,
    pub model_id: String,                          // "provider/model" format
    pub system_prompt: Option<String>,
    pub max_iterations: usize,                      // Max tool-calling loop iterations
    pub timeout_seconds: u64,
    pub temperature: Option<f64>,
    pub max_tokens: Option<usize>,
    pub compaction_strategy: CompactionStrategy,    // How to compact long contexts
    pub compaction_instruction: Option<String>,     // Custom compaction prompt
    pub context_window: usize,                      // Token limit for context
}
```

## Model Switching

Switch models mid-conversation with automatic cross-provider message transformation:

```rust
// Switch from Anthropic to OpenAI
agent.switch_model("openai/gpt-4o")?;

// Thinking blocks are automatically converted between formats
```

## Agent State

```rust
let state = agent.state();
// state.messages — conversation history
// state.iteration — current loop iteration

// Reset for a new conversation
agent.reset();

// Update system prompt dynamically
agent.set_system_prompt("New instructions...".into());
```

## License

[MIT](../LICENSE.md)
