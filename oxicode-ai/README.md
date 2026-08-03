<div align="center">

# oxicode-ai

**Unified LLM API for Rust** — streaming, multi-provider, tool calling, and context management.

[![Version](https://img.shields.io/badge/Version-0.20.0-blue?style=flat-square)](https://crates.io/search?q=oxicode)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](../LICENSE.md)
[![Docs](https://img.shields.io/docsrs/oxicode-ai/latest?style=flat-square)](https://docs.rs/oxicode-ai)

</div>

---

## Overview

`oxicode-ai` provides a single, provider-agnostic interface for interacting with large language models. It handles streaming responses, tool/function calling, conversation context, token estimation, message compaction, and cross-provider message transformation.

### Design Principles

- **Provider-agnostic** — same `Context` and `Message` types work across all providers
- **Streaming-first** — all LLM calls return async streams of `ProviderEvent`s
- **Type-safe** — strongly typed messages, tool definitions, and content blocks
- **Zero-cost** — no runtime overhead for provider abstraction

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
oxicode-ai = { path = "path/to/oxicode-ai" }
```

Basic usage:

```rust
use oxicode_ai::prelude::*;
use oxicode_ai::create_builtin_provider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a built-in provider (uses env vars for API keys)
    let provider = create_builtin_provider("anthropic")
        .ok_or_else(|| anyhow::anyhow!("Unknown provider"))?;

    // Build a model descriptor
    let model = oxicode_ai::Model::new(
        "claude-sonnet-4-20250514",
        "Claude Sonnet 4",
        oxicode_ai::Api::AnthropicMessages,
        "anthropic",
        "https://api.anthropic.com",
    );

    // Build context
    let mut ctx = Context::new();
    ctx.add_message(Message::user("Hello, world!"));

    // Stream the response
    use futures::StreamExt;
    let stream = provider.stream(&model, &ctx, None).await?;
    tokio::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::TextDelta { delta, .. } => print!("{}", delta),
            ProviderEvent::Done { .. } => println!("\n[Done]"),
            _ => {}
        }
    }

    Ok(())
}
```

## Providers

### Supported Providers

| Provider | API | Environment Variable |
|----------|-----|---------------------|
| **OpenAI** | `openai-completions` | `OPENAI_API_KEY` |
| **Anthropic** | `anthropic-messages` | `ANTHROPIC_API_KEY` |
| **Google** | `google-generative-ai` | `GOOGLE_API_KEY` |
| **DeepSeek** | `openai-completions` | `DEEPSEEK_API_KEY` |
| **Mistral** | `openai-completions` | `MISTRAL_API_KEY` |
| **Groq** | `openai-completions` | `GROQ_API_KEY` |
| **Cerebras** | `openai-completions` | `CEREBRAS_API_KEY` |
| **xAI** | `openai-completions` | `XAI_API_KEY` |
| **OpenRouter** | `openai-completions` | `OPENROUTER_API_KEY` |
| **Azure OpenAI** | `azure-openai-responses` | `AZURE_OPENAI_API_KEY` |

Providers that use the `openai-completions` API share the same `OpenAiProvider` implementation with different base URLs.

### Provider Trait

Implement the `Provider` trait to add custom providers:

```rust
use async_trait::async_trait;
use oxicode_ai::{Provider, Model, Context, StreamOptions, ProviderEvent, ProviderError};

pub struct MyProvider {
    client: reqwest::Client,
}

#[async_trait]
impl Provider for MyProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        // Implement streaming logic
        todo!()
    }

    fn name(&self) -> &str {
        "my-provider"
    }
}
```

### Provider Events

All streaming responses produce `ProviderEvent` variants:

| Event | Description |
|-------|-------------|
| `TextStart` | Text content block begins |
| `TextDelta { delta }` | Incremental text chunk |
| `TextEnd` | Text content block ends |
| `ThinkingStart` | Thinking/reasoning block begins |
| `ThinkingDelta { delta }` | Incremental thinking text |
| `ThinkingEnd` | Thinking block ends |
| `ToolCallStart` | Tool call begins |
| `ToolCallDelta { delta }` | Incremental tool call arguments |
| `ToolCallEnd { tool_call }` | Complete tool call received |
| `Done { message }` | Response complete |
| `Error { error }` | Error response |

## API Reference

### Core Types

```rust
// Model definition
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
    // ...
}

// Thinking levels
pub enum ThinkingLevel { Off, Minimal, Low, Medium, High, XHigh }

// Cache retention
pub enum CacheRetention { None, Short, Long }

// Stop reasons
pub enum StopReason { Stop, Length, ToolUse, Error, Aborted }
```

### Messages

```rust
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    Image(ImageContent),
    ToolCall(ToolCall),
}
```

### Context

```rust
let mut ctx = Context::new()
    .with_system_prompt("You are helpful.");

ctx.add_message(Message::user("Hello!"));
ctx.add_tool(Tool::new("get_weather", "Get weather", schema));
```

### Tools

```rust
use oxicode_ai::{Tool, validate_args};

let tool = Tool::new(
    "read_file",
    "Read a file from disk",
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "File path" }
        },
        "required": ["path"]
    })
);

// Validate arguments
validate_args(&tool, &args)?;
```

### Model Registry

```rust
use oxicode_ai::{get_model, get_providers, get_models, ModelRegistry};

// Get a specific model
let model = get_model("openai", "gpt-4o");

// List providers
let providers = get_providers(); // ["anthropic", "cerebras", "deepseek", ...]

// List models for a provider
let models = get_models("openai");

// Search by pattern
let results = ModelRegistry::search("claude");
```

### Token Estimation

```rust
use oxicode_ai::estimate_tokens;

let tokens = estimate_tokens(&text);
```

### Context Compaction

```rust
use oxicode_ai::{CompactionStrategy, CompactionManager, LlmCompactor};

let manager = CompactionManager::new(CompactionStrategy::Threshold(0.8), 128_000);
// Automatically compacts context when it exceeds 80% of the context window
```

### High-Level API

```rust
use oxicode_ai::complete;

let response = complete(model, context, None).await?;
```

### Streaming Options

```rust
let options = StreamOptions {
    temperature: Some(0.7),
    max_tokens: Some(4096),
    signal: None,
    api_key: None,
    cache_retention: Some(CacheRetention::Short),
    session_id: Some("my-session".into()),
    ..Default::default()
};
```

## License

[MIT](../LICENSE.md)
