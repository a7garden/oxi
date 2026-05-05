# oxi-agent Architecture

This document describes the internal architecture of the `oxi-agent` crate.

## AgentLoop Event Flow

The `AgentLoop` orchestrates the complete agent-runtime cycle:

```
User Prompt
    │
    ▼
┌─────────────────────────┐
│  run_loop()             │
│                         │
│  while has_tool_calls || │
│        pending_messages  │
│         │               │
│         ▼               │
│  maybe_compact()        │ ──► CompactionEvent::Triggered
│         │               │                  │
│         ▼               │                  ▼
│  stream_assistant()     │  CompactionEvent::Started
│         │               │                  │
│         ▼               │                  ▼
│  extract_tool_calls()  │  CompactionEvent::Completed/Failed
│         │               │
│         ▼               │
│  execute_tool_calls()  │ ──► ToolExecutionStart/End
│         │               │
│         ▼               │
│  should_stop()?        │
│         │               │
│         ▼               │
│  TurnEnd               │
└─────────────────────────┘
    │
    ▼
AgentEnd
```

### Turn Structure

Each turn consists of:

1. **TurnStart** — Turn begins
2. **MessageStart** — Assistant message begins
3. **MessageUpdate** — Partial updates (text deltas)
4. **MessageEnd** — Assistant message complete
5. **ToolExecutionStart/End** — For each tool call
6. **TurnEnd** — Turn complete

## Tool Execution Pipeline

### Parallel Execution (default)

```
┌────────────────────────────────────────────────────────────┐
│                    tool_calls: [A, B, C]                   │
└────────────────────────────────────────────────────────────┘
                          │
                          ▼
        ┌─────────────────┼─────────────────┐
        ▼                 ▼                 ▼
    prepare(A)        prepare(B)        prepare(C)
        │                 │                 │
        ▼                 ▼                 ▼
    execute(A)  ═════  execute(B)  ═════  execute(C)    (concurrent)
        │                 │                 │
        ▼                 ▼                 ▼
    finalize(A)       finalize(B)       finalize(C)
        │                 │                 │
        └─────────────────┼─────────────────┘
                          │
                          ▼
           ┌──────────────────────────────┐
           │  tool_result_messages: [R_A,  │
           │                              │
           │           R_B, R_C]           │
           │                              │
           │  terminate: should_stop()    │
           └──────────────────────────────┘
                          │
                          ▼
              Continue loop or stop
```

### Sequential Execution

When `ToolExecutionMode::Sequential`:

1. Execute tool A to completion
2. Add result to messages
3. Execute tool B
4. Add result to messages
5. Continue until all tools done

### Hook System

Before/after hooks allow interception:

```rust
pub type BeforeToolCallHook = Arc<dyn Fn(&str, &Value) -> Pin<Future<Output = Result<Option<AgentToolResult>>>>
    + Send + Sync>;

pub type AfterToolCallHook = Arc<dyn Fn(&str, &AgentToolResult) -> Pin<Future<Output = Result<Option<AgentToolResult>>>>
    + Send + Sync>;
```

- **Before hook**: Can block execution, return immediate result
- **After hook**: Can modify result before it's sent to LLM

## Circuit Breaker + Retry Logic

### Provider-Level Retries

```rust
async fn stream_with_retry(&self, ...) -> Result<Stream> {
    for attempt in 0..=MAX_RETRIES {
        if let Err(open) = self.circuit_breaker.allow_request() {
            return Err(StreamError::CircuitOpen(open));
        }
        
        match self.provider.stream(...).await {
            Ok(stream) => {
                self.circuit_breaker.record_success();
                return Ok(stream);
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                
                if attempt < MAX_RETRIES {
                    let delay = BACKOFF_BASE.pow(attempt);
                    emit(RetryEvent { attempt, delay });
                    sleep(delay).await;
                }
            }
        }
    }
}
```

### Circuit Breaker Configuration

```rust
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,      // Failures before opening (default: 5)
    pub success_threshold: u32,     // Successes before closing (default: 3)
    pub open_duration_secs: u64,    // Time circuit stays open (default: 30)
}
```

### Auto-Retry on Assistant Errors

When the LLM returns an error in `stop_reason: Error`:

```rust
async fn handle_retryable_error(&self, message: &AssistantMessage, ...) -> bool {
    if !is_retryable_error(message) {
        return false;
    }
    
    // Exponential backoff: 2s, 4s, 8s, ...
    let delay = base_delay * 2^attempt;
    
    emit(AutoRetryStart { attempt, delay });
    sleep(delay).await;
    
    // Remove error message, retry
    messages.pop(); // Remove error assistant message
    return true;
}
```

### Retryable Error Patterns

```rust
Regex: "(?i)overloaded|rate.?limit|429|500|502|503|504|
       network.?error|connection|timeout"
```

## State Management

### SharedState

```rust
pub struct SharedState {
    state: RwLock<AgentState>,
}

pub struct AgentState {
    pub messages: Vec<Message>,
    pub iteration: usize,
    pub last_error: Option<String>,
}
```

### State Access Patterns

```rust
// Read
let state = self.state.get_state();
for msg in &state.messages { ... }

// Write
self.state.update(|s| {
    s.messages.push(new_message);
    s.iteration += 1;
});
```

### State Persistence

Messages are persisted after each turn:

```rust
self.state.update(|s| {
    s.replace_messages(messages.clone());
});
```

## Compaction Integration

Compaction is checked before each LLM call:

```rust
async fn maybe_compact(&self, messages: &mut Vec<Message>, ...) {
    let context_text = serde_json::to_string(&*messages)?;
    let tokens = estimate_tokens(&context_text);
    
    if !self.compaction_manager.should_compact(tokens, iteration) {
        return;
    }
    
    emit(CompactionEvent::Triggered { context_tokens: tokens, iteration });
    
    match self.compaction_manager.compact_if_needed(...).await {
        Ok(Some(compacted)) => {
            *messages = compacted.kept_messages;
            emit(CompactionEvent::Completed { result, duration_ms });
        }
        Ok(None) => { /* No compaction needed */ }
        Err(e) => emit(CompactionEvent::Failed { error }),
    }
}
```

## Steering Queue

Messages can be injected mid-loop:

```rust
impl AgentLoop {
    /// Add a message to be processed in the next turn
    pub fn steer(&self, message: Message) {
        self.steering_queue.write().push(message);
    }
    
    /// Add a message to be processed after the current turn
    pub fn follow_up(&self, message: Message) {
        self.follow_up_queue.write().push(message);
    }
}
```

Drained at the start of each turn loop iteration.

## Tool Registry

```rust
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn AgentTool>>>>,
}

impl ToolRegistry {
    /// Register a tool
    pub fn register(&self, tool: impl AgentTool + 'static);
    
    /// Register a pre-Arc'd tool (for extensions)
    pub fn register_arc(&self, tool: Arc<dyn AgentTool>);
    
    /// Get tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>>;
    
    /// All tool names
    pub fn names(&self) -> Vec<String>;
    
    /// All tool definitions (for LLM)
    pub fn definitions(&self) -> Vec<ToolDefinition>;
    
    /// Built-in tools
    pub fn with_builtins() -> Self;
}
```

## AgentEvent Reference

| Event | Fields | Description |
|-------|--------|-------------|
| `AgentStart` | `prompts`, `session_id` | Agent loop started |
| `AgentEnd` | `messages`, `stop_reason` | Agent loop completed |
| `TurnStart` | `turn_number` | Turn begins |
| `TurnEnd` | `turn_number`, `assistant_message`, `tool_results` | Turn complete |
| `MessageStart` | `message` | Message begins |
| `MessageUpdate` | `message`, `delta` | Partial message update |
| `MessageEnd` | `message` | Message complete |
| `ToolExecutionStart` | `tool_call_id`, `tool_name`, `args` | Tool call begins |
| `ToolExecutionUpdate` | `tool_call_id`, `tool_name`, `partial_result` | Tool progress |
| `ToolExecutionEnd` | `tool_call_id`, `tool_name`, `result`, `is_error` | Tool finished |
| `Compaction` | `event` | Compaction state change |
| `AutoRetryStart` | `attempt`, `max_attempts`, `delay_ms` | Retry initiated |
| `AutoRetryEnd` | `success`, `attempt`, `final_error` | Retry completed |
| `Error` | `message`, `session_id` | Error occurred |