# oxi-ai Architecture

This document describes the internal architecture of the `oxi-ai` crate.

## Provider Trait Design

The `Provider` trait is the core abstraction for LLM interactions:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;
    
    fn name(&self) -> &str;
}
```

### Design Goals

1. **Zero-cost abstraction**: Each provider implements the trait directly without boxing
2. **Streaming-first**: All responses are streamed asynchronously
3. **Type-safe events**: `ProviderEvent` enum captures all possible streaming states
4. **Provider-agnostic**: Same `Context` and `Model` types work across providers

### Provider Implementations

| Provider | Module | API Style |
|----------|--------|-----------|
| Anthropic | `providers/anthropic` | `messages` endpoint |
| OpenAI | `providers/openai_*` | Completions or Responses |
| Google | `providers/google` | Gemini API |
| Azure | `providers/azure` | Azure OpenAI |
| Mistral | `providers/mistral` | OpenAI-compatible |
| DeepSeek | `providers/deepseek` | OpenAI-compatible |
| Bedrock | `providers/bedrock` | AWS API |
| Cloudflare | `providers/cloudflare` | Workers AI |

## Message Types Hierarchy

```
Message
├── User(UserMessage)
│   ├── role: UserRole
│   ├── content: MessageContent
│   └── timestamp
│
├── Assistant(AssistantMessage)
│   ├── api: Api
│   ├── provider: String
│   ├── model: String
│   ├── content: Vec<ContentBlock>
│   ├── stop_reason: StopReason
│   ├── usage: Usage
│   └── error_message: Option<String>
│
└── ToolResult(ToolResultMessage)
    ├── role: ToolResultRole
    ├── tool_call_id: String
    ├── tool_name: String
    ├── content: Vec<ContentBlock>
    └── is_error: bool

ContentBlock
├── Text(TextContent)
├── Thinking(ThinkingContent)
├── Image(ImageContent)
└── ToolCall(ToolCall)
```

### Content Block Types

- **Text**: Plain text content with optional signature
- **Thinking**: Extended reasoning (Anthropic format)
- **Image**: Base64-encoded image data with MIME type
- **ToolCall**: Function call request with ID, name, and arguments

## Cross-Provider Transformation Flow

When switching models mid-conversation, message formats must be converted:

```
┌─────────────────┐    to_intermediate    ┌──────────────────────┐    from_intermediate    ┌─────────────────┐
│  Source Format  │ ──────────────────►  │  Intermediate (JSON)  │ ──────────────────────►  │ Target Format   │
│  (Anthropic,    │                      │  - text blocks        │                          │  (OpenAI,       │
│   OpenAI, etc.) │                      │  - thinking blocks    │                          │   Google, etc.) │
└─────────────────┘                      │  - image blocks      │                          └─────────────────┘
                                          │  - tool calls        │
                                          └──────────────────────┘
```

### Transform Options

```rust
pub struct TransformOptions {
    pub strip_thinking: bool,      // Remove thinking blocks
    pub convert_tools: bool,       // Include tool calls
    pub convert_images: bool,      // Include image blocks
    pub merge_text: bool,          // Merge adjacent text blocks
}
```

### Directional Converters

- `anthropic_to_openai()` — Claude → GPT
- `openai_to_anthropic()` — GPT → Claude
- `google_to_openai()` — Gemini → GPT
- `anthropic_to_google()` — Claude → Gemini

## Compaction Strategies

Context compaction prevents token limit overflow:

```rust
pub enum CompactionStrategy {
    Disabled,
    Threshold(f32),     // Compact when usage exceeds threshold (e.g., 0.8)
    MaxMessages(usize),  // Compact after N messages
}
```

### CompactionManager

```rust
pub struct CompactionManager {
    strategy: CompactionStrategy,
    context_window: usize,
    compactor: Option<Arc<dyn LlmCompactor>>,
}
```

### LLM Compactor

The `LlmCompactor` trait allows using an actual LLM for context summarization:

```rust
#[async_trait]
pub trait LlmCompactor: Send + Sync {
    async fn compact(
        &self,
        messages: &[Message],
        instruction: Option<&str>,
    ) -> Result<CompactedContext, Error>;
}
```

### Compaction Process

1. Check if compaction is needed (`should_compact`)
2. Extract summary-worthy messages
3. Invoke compactor (LLM-based or simple)
4. Replace messages with summary
5. Emit `CompactionEvent` for observers

## Token Estimation

The token estimator uses a hybrid algorithm:

```rust
pub fn estimate(text: &str) -> usize {
    // CJK: ~1 token per character
    // Punctuation: ~1.5 tokens per char
    // ASCII: ~4 chars per token
    // Whitespace: ~8 words per token
}
```

### Algorithm Details

| Character Type | Estimate |
|---------------|----------|
| CJK (Chinese, Japanese, Korean) | 1 token/char |
| Punctuation & symbols | 1.5 tokens/char |
| ASCII/Latin letters | 4 chars/token |
| Whitespace-separated words | 8 words/token |

### Context Usage

```rust
let usage = context_usage(text, context_window); // Returns 0.0 to 1.0
if usage > 0.8 {
    // Trigger compaction
}
```

## ProviderEvent Stream

All LLM responses are streamed as `ProviderEvent`:

```rust
pub enum ProviderEvent {
    Start { partial: AssistantMessage },
    TextStart { content_index, partial },
    TextDelta { delta, content_index, partial },
    TextEnd { content_index, content, partial },
    ThinkingStart { content_index, partial },
    ThinkingDelta { delta, content_index, partial },
    ThinkingEnd { content_index, content, partial },
    ToolCallStart { tool_call_id, content_index, partial },
    ToolCallDelta { delta, content_index, partial },
    ToolCallEnd { tool_call, content_index, partial },
    Done { reason, message },
    Error { error },
}
```

### Event Flow

```
ProviderEvent::Start ─────────────► ProviderEvent::TextStart ──► ProviderEvent::TextDelta* ──► ProviderEvent::TextEnd
                                    └─► ProviderEvent::ThinkingStart ──► ProviderEvent::ThinkingDelta* ──► ProviderEvent::ThinkingEnd
                                    └─► ProviderEvent::ToolCallStart ──► ProviderEvent::ToolCallDelta* ──► ProviderEvent::ToolCallEnd
                                    │
                                    ▼
                               ProviderEvent::Done ──► ProviderEvent::Error (on error)
```

## Streaming Options

```rust
pub struct StreamOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<usize>,
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
}
```

## Error Handling

All provider errors are wrapped in `ProviderError`:

```rust
pub enum ProviderError {
    HttpError(u16, String),
    ParseError(String),
    StreamError(String),
    AuthError(String),
    RateLimitError { retry_after: Option<u64> },
}
```

Retry logic is handled by the caller (typically `oxi-agent`).