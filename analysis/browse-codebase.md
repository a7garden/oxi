# Browse Tool & Agent Event System — Comprehensive Codebase Analysis

> Generated for exploration-transparency design review.
> Covers all event emission, progress callback, and browser tool internals.

---

## Table of Contents

1. [AgentEvent System](#1-agentevent-system)
2. [Agent Tool Trait & Registry](#2-agent-tool-trait--registry)
3. [Agent Struct & Run Flow](#3-agent-struct--run-flow)
4. [Agent Loop](#4-agent-loop)
5. [Tool Execution Logic](#5-tool-execution-logic)
6. [Browse Module Structure](#6-browse-module-structure)
7. [Browser Engine Abstraction](#7-browser-engine-abstraction)
8. [BrowseTool](#8-browsetool)
9. [BrowseSessionTool](#9-browsesessiontool)
10. [BrowseExtractTool](#10-browseextracttool)
11. [BrowseScriptTool](#11-browsescripttool)
12. [OxicodeBrowser Backend](#12-oxibrowser-backend)
13. [Tab Guard (RAII)](#13-tab-guard-raii)
14. [Browse Config](#14-browse-config)
15. [Helpers](#15-helpers)
16. [Progress Callback Flow — End to End](#16-progress-callback-flow--end-to-end)
17. [Gaps & Opportunities for Exploration Transparency](#17-gaps--opportunities-for-exploration-transparency)

---

## 1. AgentEvent System

**File:** `oxicode-agent/src/events.rs`

### Key Type: `AgentEvent` (enum, `#[non_exhaustive]`)

Tagged with `#[serde(tag = "type", rename_all = "camelCase")]` — serialized with a `"type"` discriminator.

#### Event Categories

| Category | Variants | Fields |
|----------|----------|--------|
| **Lifecycle** | `AgentStart` | `prompts: Vec<Message>`, `session_id: Option<String>` |
| | `AgentEnd` | `messages: Vec<Message>`, `stop_reason: Option<String>`, `session_id` |
| | `TurnStart` | `turn_number: u32` |
| | `TurnEnd` | `turn_number`, `assistant_message: Message`, `tool_results: Vec<ToolResultMessage>` |
| **Message** | `MessageStart` | `message: Message` |
| | `MessageUpdate` | `message: Message`, `delta: Option<String>` |
| | `MessageEnd` | `message: Message` |
| **Tool Execution (new)** | `ToolExecutionStart` | `tool_call_id`, `tool_name`, `args: Value` |
| | `ToolExecutionUpdate` | `tool_call_id`, `tool_name`, `partial_result: String`, **`tab_id: Option<uuid::Uuid>`** |
| | `ToolExecutionEnd` | `tool_call_id`, `tool_name`, `result: ToolResult`, `is_error: bool` |
| **Legacy** | `Start`, `Thinking`, `ThinkingDelta`, `TextChunk`, `ToolCall`, `ToolStart`, `ToolProgress`, `ToolComplete`, `ToolError`, `Complete`, `Error`, `Iteration`, `Usage` | (various) |
| **Resilience** | `Retry`, `Fallback`, `AutoRetryStart`, `AutoRetryEnd` | (retry metadata) |
| **Steering** | `SteeringMessage`, `FollowUpMessage` | `message: Message` |
| | `Cancelled`, `PartialResponse`, `Compaction` | (various) |

#### Public API

- `AgentEvent::is_terminal(&self) -> bool` — returns true only for `AgentEnd`.
- `AgentEvent::type_name(&self) -> &'static str` — returns snake_case variant name.

#### How Events Are Emitted

Events are emitted via an `EmitFn = Arc<dyn Fn(AgentEvent) + Send + Sync>`. The emit function is:
1. Created in `Agent::run_with_channel_inner` or `Agent::run_tokio_stream`.
2. Wraps a `std::sync::mpsc::Sender<AgentEvent>` (sync) or `tokio::sync::mpsc::Sender` (tokio).
3. Passed to `AgentLoop::run()` → `run_messages()` → `run_loop()`.
4. Called synchronously from within the agent loop at every lifecycle point.

#### Key Observation: `ToolExecutionUpdate` with `tab_id`

The `ToolExecutionUpdate` variant already carries an optional `tab_id: Option<uuid::Uuid>`. This field is populated by the progress callback mechanism in `tool_exec.rs` and is specifically designed for browser tools. See [§16](#16-progress-callback-flow--end-to-end) for the full flow.

---

## 2. Agent Tool Trait & Registry

**File:** `oxicode-agent/src/tools.rs`

### Key Types

#### `AgentTool` (trait)

```rust
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn essential(&self) -> bool { false }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError>;

    // Progress callbacks
    fn on_progress(&self, _callback: ProgressCallback) {}         // String-based
    fn on_structured_progress(&self, _callback: StructuredProgressCallback) {} // Structured

    // TUI rendering
    fn render_call(&self, _params: &Value) -> Option<RenderOutput> { None }
    fn render_result(&self, _result: &AgentToolResult) -> Option<RenderOutput> { None }

    // Parallel safety
    fn execution_mode(&self) -> ToolExecutionMode { ParallelSafe }

    // Tab-aware support (for browser tools)
    fn current_tab_id(&self) -> Option<uuid::Uuid> { None }
    fn set_tab_id_slot(&self, _slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>>) {}
}
```

#### `ProgressCallback`
```rust
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;
```
Simple string-based callback. When invoked, the tool_exec.rs wrapper emits a `ToolExecutionUpdate` event.

#### `StructuredProgressCallback`
```rust
pub type StructuredProgressCallback = Arc<dyn Fn(ToolProgress) + Send + Sync>;
```
More detailed progress (status messages, partial output, percentages, file ops). Currently **not wired** in the agent loop — tools define it but the loop only uses `on_progress`.

#### `ToolProgress` (enum)
- `Status { message: String }`
- `PartialOutput { output: String, is_error: bool }`
- `Percentage { current: f64, total: Option<f64>, message: Option<String> }`
- `FileOperation { operation: FileOp, path: PathBuf, bytes_processed: Option<u64>, total_bytes: Option<u64> }`

#### `ToolExecutionMode` (enum)
- `ParallelSafe` — can run concurrently
- `SequentialOnly` — must run alone
- `MutatesFile(PathBuf)` — serialized per-file
- `ReadOnly` — always parallel safe

#### `AgentToolResult`
```rust
pub struct AgentToolResult {
    pub success: bool,
    pub output: String,
    pub metadata: Option<serde_json::Value>,
    pub content_blocks: Option<Vec<oxicode_ai::ContentBlock>>,
    pub terminate: bool,
}
```
- `with_metadata(value)` — attach JSON metadata
- `with_content_blocks(blocks)` — attach image blocks etc.
- `with_terminate()` — signal loop should stop after this batch

#### `ToolContext`
```rust
pub struct ToolContext {
    pub workspace_dir: PathBuf,
    pub root_dir: Option<PathBuf>,
    pub session_id: Option<String>,
}
```

#### `ToolRegistry`
- `HashMap<String, Arc<dyn AgentTool>>` behind `Arc<parking_lot::RwLock<...>>`
- `with_builtins_cwd()` registers: ReadTool, WriteTool, EditTool, BashTool, GrepTool, FindTool, LsTool, WebSearchTool, GetSearchResultsTool, GitHubTool, SubagentTool, McpTool, Context7ResolveLibraryIdTool, Context7QueryDocsTool, GenerateImageTool
- **Browser tools are NOT registered by default** — they're added separately in `oxicode-cli/src/bootstrap.rs`

---

## 3. Agent Struct & Run Flow

**File:** `oxicode-agent/src/agent.rs`

### Key Struct: `Agent`

| Field | Type | Purpose |
|-------|------|---------|
| `inner` | `RwLock<AgentInner>` | Config + provider (mutable for model switching) |
| `tools` | `Arc<ToolRegistry>` | Available tools |
| `state` | `SharedState` | `Arc<parking_lot::RwLock<AgentState>>` — conversation history |
| `compaction_manager` | `CompactionManager` | Context window management |
| `hooks` | `RwLock<AgentHooks>` | Pre/post tool call, steering, follow-up hooks |
| `is_running` | `Arc<AtomicBool>` | Prevents concurrent runs |
| `resolver` | `Arc<dyn ProviderResolver>` | Model/provider lookup |
| `cancel_flag` | `Arc<AtomicBool>` | Shared with AgentLoop for cancellation |
| `pending_model_switch` | `RwLock<Option<PendingModelSwitch>>` | Deferred model switches |

### Run Flow

```
Agent::run_with_channel(prompt, tx)
  ├── Check is_running (CAS)
  ├── Reset cancel_flag
  ├── Build AgentLoopConfig from inner config
  ├── Create fresh SharedState (copy of current)
  ├── Create AgentLoop::new_with_resolver(provider, config, tools, state, resolver)
  ├── Pre-populate steering/follow-up from hooks
  ├── Wire should_stop_after_turn hook → external_stop
  ├── Share cancel_flag with AgentLoop
  ├── AgentLoop::run(prompt, emit_callback)
  │   └── emit_callback:
  │       ├── tx.send(event)  → forward to channel
  │       ├── Check cancel_flag → set external_stop
  │       └── Check should_stop hook → set external_stop
  ├── Sync state back from AgentLoop
  ├── Apply pending model switch
  └── Return Response
```

### Key Methods
- `run(prompt)` → `(Response, Vec<AgentEvent>)`
- `run_with_channel(prompt, tx)` → `Result<Response>` (events go through tx)
- `run_streaming(prompt, on_event)` → callback-based
- `run_tokio_stream(prompt)` → `(tokio::Receiver<AgentEvent>, JoinHandle<Result<Response>>)`
- `cancel()` — sets cancel_flag
- `switch_model(model_id, api_key)` — immediate or deferred

---

## 4. Agent Loop

**File:** `oxicode-agent/src/agent_loop/mod.rs`

### Key Struct: `AgentLoop`

| Field | Type | Purpose |
|-------|------|---------|
| `provider` | `Arc<dyn Provider>` | LLM backend |
| `config` | `AgentLoopConfig` | Loop parameters |
| `tools` | `Arc<ToolRegistry>` | Tool registry |
| `state` | `SharedState` | Conversation state |
| `compaction_manager` | `OxCompactionManager` | Context compaction |
| `before_tool_call` | `Option<BeforeToolCallHook>` | Pre-execution hook |
| `after_tool_call` | `Option<AfterToolCallHook>` | Post-execution hook |
| `steering_queue` | `RwLock<Vec<Message>>` | Injected system messages |
| `follow_up_queue` | `RwLock<Vec<Message>>` | Continuation messages |
| `external_stop` | `Arc<AtomicBool>` | Cancel signal |
| `cancel_signal` | `Option<Arc<AtomicBool>>` | Direct cancel from Agent |
| `steering_hook` | `Option<Arc<dyn Fn() -> Vec<String>>>` | External steering source |
| `follow_up_hook` | `Option<Arc<dyn Fn() -> Vec<String>>>` | External follow-up source |

### Loop Architecture (from run_loop)

```
run_messages(prompts, emit)
  ├── emit(AgentStart)
  └── run_loop(initial_prompts, emit)
      └── Outer loop (follow-up/steering):
          └── Inner loop (tool calls):
              ├── poll_external_queues()
              ├── process_steering_messages() → emit SteeringMessage, MessageStart, MessageEnd
              ├── maybe_compact()
              ├── stream_assistant_response() → emit MessageStart, TextChunk, ToolCall, etc.
              ├── extract_tool_calls()
              ├── execute_tool_calls() → emit ToolExecutionStart, ToolExecutionUpdate, ToolExecutionEnd
              ├── emit TurnEnd
              ├── should_stop_after_turn?
              ├── drain_steering_queue()
              └── (loop or break)
          ├── Check follow-up queue
          └── (break or continue outer)
  └── emit(AgentEnd)
```

### Key Event Emission Points in run_loop

1. **AgentStart** — at the top of `run_messages()`
2. **TurnStart** — at the start of each inner loop iteration
3. **SteeringMessage / MessageStart / MessageEnd** — when steering messages are processed
4. **Compaction** — before streaming (if compaction triggers)
5. **TextChunk / ThinkingDelta / MessageStart / MessageUpdate / MessageEnd** — during streaming
6. **ToolExecutionStart** — before each tool call (emitted in tool_exec.rs)
7. **ToolExecutionUpdate** — during tool execution (via progress callback)
8. **ToolExecutionEnd** — after each tool call (emitted in tool_exec.rs)
9. **TurnEnd** — at the end of each inner loop iteration
10. **AgentEnd** — at the end of `run_messages()`

---

## 5. Tool Execution Logic

**File:** `oxicode-agent/src/agent_loop/tool_exec.rs`

### Key Function: `execute_tool_calls()`

Dispatches to sequential or parallel based on `ToolExecutionMode` in config.

### Sequential Flow (`execute_tool_calls_sequential`)

For each `ToolCall`:
1. Check cancellation
2. Emit `ToolExecutionStart`
3. `prepare_tool_call()` — look up tool, run before_tool_call hook
4. If tool found + not blocked: `execute_prepared_tool_call()`
5. Run `after_tool_call` hook (can modify result)
6. Emit `ToolExecutionEnd`
7. Emit `MessageStart(Message::ToolResult(...))` + `MessageEnd`

### `execute_prepared_tool_call()` — **THE CRITICAL FUNCTION FOR PROGRESS**

This is where the progress callback bridge lives:

```rust
async fn execute_prepared_tool_call(...) -> ExecutedToolCallOutcome {
    // 1. Create shared tab_id slot
    let tab_id_slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>> = ...;

    // 2. Pass slot to tool (so it can write tab_id when it opens a tab)
    tool.set_tab_id_slot(Arc::clone(&tab_id_slot));

    // 3. Create progress callback that:
    //    a. Reads current tab_id from the slot
    //    b. Emits ToolExecutionUpdate { tool_call_id, tool_name, partial_result, tab_id }
    let progress_cb = Arc::new(move |msg: String| {
        let tab_id = *tab_id_slot_cb.lock();
        emit_clone(AgentEvent::ToolExecutionUpdate {
            tool_call_id: ...,
            tool_name: ...,
            partial_result: msg,
            tab_id,
        });
    });

    // 4. Wire progress callback BEFORE execute
    tool.on_progress(progress_callback(move |msg| progress_cb(msg)));

    // 5. Execute the tool
    tool.execute(tool_call_id, args, None, ctx).await
}
```

**Key insight:** The progress callback wraps `EmitFn` (the agent loop's event emitter). Every time the tool calls the progress callback with a string, a `ToolExecutionUpdate` event is emitted through the same channel as all other agent events.

---

## 6. Browse Module Structure

**File:** `oxicode-agent/src/tools/browse/mod.rs`

```
browse/
├── mod.rs                  — Module declarations + re-exports
├── browse_tool.rs          — BrowseTool (single-shot page render)
├── browse_session_tool.rs  — BrowseSessionTool (persistent tab)
├── browse_extract_tool.rs  — BrowseExtractTool (CSS selector extraction)
├── browse_script_tool.rs   — BrowseScriptTool (YAML automation) [feature-gated]
├── engine.rs               — BrowserEngine + BrowserTab traits + TabCallbackRegistry
├── oxibrowser_backend.rs   — OxicodeBrowserEngine + OxicodeTab [feature-gated]
├── config.rs               — BrowseConfig
├── helpers.rs              — JS snippets + result parsing
└── tab_guard.rs            — RAII tab cleanup
```

Feature gate: `native-browser` enables `browse_script_tool` and `oxibrowser_backend`.

### Re-exports
```rust
pub use browse_extract_tool::BrowseExtractTool;
pub use browse_session_tool::BrowseSessionTool;
pub use browse_tool::BrowseTool;
pub use config::BrowseConfig;
pub use engine::{BrowserEngine, BrowserError, BrowserTab, ElementInfo, LinkInfo, PageContent};
pub use tab_guard::TabGuard;

#[cfg(feature = "native-browser")]
pub use browse_script_tool::BrowseScriptTool;
pub use oxibrowser_backend::OxicodeBrowserEngine;
```

---

## 7. Browser Engine Abstraction

**File:** `oxicode-agent/src/tools/browse/engine.rs`

### `BrowserEngine` (trait)

```rust
#[async_trait]
pub trait BrowserEngine: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<PageContent, BrowserError> { ... } // default: new_tab → goto → close
    async fn new_tab(&self) -> Result<Box<dyn BrowserTab>, BrowserError>;
    async fn close(&self) -> Result<(), BrowserError>;
    async fn is_alive(&self) -> bool;
    fn callback_registry(&self) -> Arc<TabCallbackRegistry> { ... } // default: empty registry
}
```

### `BrowserTab` (trait)

```rust
#[async_trait]
pub trait BrowserTab: Send + Sync {
    // Navigation
    async fn goto(&self, url: &str) -> Result<PageContent, BrowserError>;
    async fn back(&self) -> Result<PageContent, BrowserError>;
    async fn forward(&self) -> Result<PageContent, BrowserError>;
    async fn reload(&self) -> Result<PageContent, BrowserError>;

    // DOM interaction
    async fn click(&self, selector: &str) -> Result<(), BrowserError>;
    async fn type_(&self, selector: &str, text: &str) -> Result<(), BrowserError>;
    async fn fill(&self, selector: &str, value: &str) -> Result<(), BrowserError>;
    async fn press(&self, combo: &str) -> Result<(), BrowserError>;
    async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<(), BrowserError>;
    async fn select_option(&self, selector: &str, value: &str) -> Result<(), BrowserError>;
    async fn check(&self, selector: &str) -> Result<(), BrowserError>;
    async fn uncheck(&self, selector: &str) -> Result<(), BrowserError>;

    // Content extraction
    async fn content(&self) -> Result<PageContent, BrowserError>;
    async fn query_all(&self, selector: &str) -> Result<Vec<String>, BrowserError>;
    async fn evaluate(&self, js: &str) -> Result<Value, BrowserError>;
    async fn screenshot(&self, width: u32) -> Result<Vec<u8>, BrowserError>;

    // Advanced (with default impls using evaluate())
    async fn clear(&self, selector: &str) -> Result<(), BrowserError>;
    async fn hover(&self, selector: &str) -> Result<(), BrowserError>;
    async fn double_click(&self, selector: &str) -> Result<(), BrowserError>;
    async fn right_click(&self, selector: &str) -> Result<(), BrowserError>;
    async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), BrowserError>;
    async fn scroll_into_view(&self, selector: &str) -> Result<(), BrowserError>;
    async fn drag(&self, from: &str, to: &str) -> Result<(), BrowserError>;
    async fn upload_file(&self, selector: &str, path: &str) -> Result<(), BrowserError>;
    async fn get_value(&self, selector: &str) -> Result<String, BrowserError>;
    async fn evaluate_await(&self, js: &str) -> Result<Value, BrowserError>;

    // Tab identity
    fn tab_id(&self) -> uuid::Uuid { uuid::Uuid::nil() }
    fn is_closed(&self) -> bool { false }
    fn as_any(&self) -> &dyn Any { ... }
    fn clear_progress_callback(&self) {}
}
```

### `PageContent`
```rust
pub struct PageContent {
    pub url: String,        // Final URL after redirects
    pub title: String,
    pub status: u16,        // HTTP status code
    pub markdown: String,   // Rendered as markdown
    pub html: String,       // Raw HTML
}
```

### `BrowserError` (enum)
- `Navigation(String)`, `ElementNotFound(String)`, `Timeout(String)`, `Evaluation(String)`, `Screenshot(String)`, `TabClosed(String)`, `Backend(String)`, `NoActiveSession`

### `TabCallbackRegistry`

```rust
pub struct TabCallbackRegistry {
    callbacks: Mutex<HashMap<uuid::Uuid, ProgressCallback>>,
}
```
- `set(tab_id, cb)` — register a callback for a tab
- `clear(tab_id)` — remove callback
- `invoke(tab_id, msg)` — call the callback if registered (no-op if not)
- `is_set(tab_id)` — check if registered
- Per-tab isolation: each tab gets its own callback, events route only to the matching tab

---

## 8. BrowseTool

**File:** `oxicode-agent/src/tools/browse/browse_tool.rs`

### Struct

```rust
pub struct BrowseTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    pending_callback: Mutex<Option<ProgressCallback>>,
    tab_id_slot: Mutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}
```

### Parameters

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | **yes** | URL to browse |
| `format` | enum | no | `markdown` (default), `html`, `text`, `links` |
| `selector` | string | no | CSS selector for scoped extraction |
| `wait_for` | string | no | CSS selector to wait for before extraction |
| `screenshot` | boolean | no | Include PNG screenshot (default: false) |

### Execution Mode: `SequentialOnly`

### Progress Callback Flow

1. `on_progress(callback)` is called by `tool_exec.rs` **before** `execute()`.
2. `BrowseTool` stores it in `pending_callback` (Mutex).
3. In `execute()`:
   - Opens a new tab via `engine.new_tab()`
   - Reads `tab_id` from the tab
   - Writes tab_id to `tab_id_slot` (so `tool_exec.rs`'s callback reads it)
   - On `native-browser` feature: downcasts tab to `OxicodeTab`, calls `set_progress_callback(cb)`
   - This registers the callback in `TabCallbackRegistry` keyed by `tab_id`
4. The background event-drain task in `OxicodeBrowserEngine` then routes browser events to this callback.

### Execute Flow

```
execute(tool_call_id, params, signal, ctx)
  ├── Parse url, format, selector, wait_for, screenshot
  ├── engine.new_tab() → raw_tab
  ├── Store tab_id in tab_id_slot
  ├── Register pending_callback on the tab (if native-browser)
  ├── Create TabGuard(raw_tab)
  ├── tab.goto(url) → page: PageContent
  ├── If wait_for: tab.wait_for(selector, timeout)
  ├── Build output based on format:
  │   ├── "html": tab.query_all(selector) or page.html
  │   ├── "links": helpers::extract_links(tab) → format_links()
  │   ├── "text": tab.query_all(selector) or page.markdown
  │   └── "markdown": tab.query_all(selector) or page.markdown
  ├── If screenshot: tab.screenshot(width) → base64 Image content block
  ├── guard.close().await
  ├── Clear tab_id_slot (None)
  └── Return AgentToolResult::success(output).with_metadata({url, title, status})
```

### Key Observation

BrowseTool opens **exactly one tab per request** and closes it before returning. The tab lifecycle is fully contained within a single `execute()` call. Progress events flow from:
- `oxibrowser-core` → `BrowserEvent` → background drain task → `TabCallbackRegistry.invoke(tab_id, label)` → `ProgressCallback(String)` → `tool_exec.rs` progress wrapper → `AgentEvent::ToolExecutionUpdate`

---

## 9. BrowseSessionTool

**File:** `oxicode-agent/src/tools/browse/browse_session_tool.rs`

### Struct

```rust
pub struct BrowseSessionTool {
    engine: Arc<dyn BrowserEngine>,
    tab: Arc<Mutex<Option<TabGuard>>>,
    config: BrowseConfig,
    last_action: Arc<Mutex<Option<Instant>>>,
}
```

### Actions (29 total)

| Category | Actions |
|----------|---------|
| Lifecycle | `open`, `close` |
| Navigation | `goto`, `back`, `forward`, `reload` |
| DOM | `click`, `fill`, `type`, `clear`, `press`, `select`, `check`, `uncheck` |
| Scroll | `scroll`, `scroll_into_view` |
| Advanced | `hover`, `double_click`, `right_click`, `drag`, `upload_file` |
| Wait | `wait_for` |
| Read | `content`, `query_all`, `extract_links` |
| Eval | `evaluate`, `evaluate_await`, `get_value` |
| Screenshot | `screenshot` |

### Key Differences from BrowseTool

1. **Persistent tab** — `tab` field holds an `Arc<Mutex<Option<TabGuard>>>` that survives across `execute()` calls.
2. **No progress callback support** — `BrowseSessionTool` does NOT override `on_progress()`. This means it does NOT emit `ToolExecutionUpdate` events during session actions.
3. **No tab_id_slot** — it doesn't implement `set_tab_id_slot()` or `current_tab_id()`.
4. **Idle timeout** — auto-closes stale sessions after `session_idle_timeout_secs` (default: 300s).
5. **Sequential by default** — single persistent tab means no concurrent access.

### Session Flow

```
open  → engine.new_tab() → store in Arc<Mutex<Option<TabGuard>>>
goto  → require_tab() → tab.goto(url)
click → require_tab() → tab.click(selector)
...
close → guard.close().await → slot.take()
```

---

## 10. BrowseExtractTool

**File:** `oxicode-agent/src/tools/browse/browse_extract_tool.rs`

### Struct

```rust
pub struct BrowseExtractTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
}
```

### Parameters

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | **yes** | URL to extract from |
| `selector` | string | **yes** | CSS selector |
| `extract` | enum | no | `links`, `text` (default), `elements`, `markdown` |
| `all` | boolean | no | Return all matches or just first (default: true) |
| `timeout` | integer | no | Timeout in seconds (default: 30) |

### Key Differences

1. **No progress callback support** — does NOT override `on_progress()`.
2. **No tab_id_slot** — not tab-aware for the agent loop.
3. **Timeout wrapper** — wraps the entire operation in `tokio::time::timeout()`.
4. **Single tab per call** — opens, extracts, closes (same pattern as BrowseTool).

### Extract Modes

| Mode | Implementation |
|------|----------------|
| `links` | `helpers::js_links_within(selector)` → `helpers::parse_link_values()` |
| `elements` | `helpers::js_query_elements(selector)` → `helpers::parse_element_values()` |
| `markdown` | `tab.query_all(selector)` → join with `\n\n` |
| `text` | `tab.query_all(selector)` → join with `\n` |

---

## 11. BrowseScriptTool

**File:** `oxicode-agent/src/tools/browse/browse_script_tool.rs`  
**Feature-gated:** `#[cfg(feature = "native-browser")]`

### Struct

```rust
pub struct BrowseScriptTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
}
```

### Parameters

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `script` | string | **yes** | YAML script (inline or path to .yaml file) |
| `timeout` | integer | no | Max execution time in seconds (default: 60) |

### Step Types (enum `Step`)

`Goto`, `Back`, `Forward`, `Reload`, `Click`, `Fill`, `Type`, `Clear`, `Check`, `Uncheck`, `Select`, `Press`, `Scroll`, `Wait`, `Evaluate`, `Extract`, `Content`, `Screenshot`, `Set`, `Echo`, `Sleep`

### Key Differences

1. **No progress callback support** — does NOT override `on_progress()`.
2. **Single tab for entire script** — opens one tab, runs all steps, closes.
3. **Deadline-based timeout** — checks `tokio::time::Instant` at each step.
4. **Max step limit** — `config.max_script_steps` (default: 100).
5. **YAML parsing** — accepts both `steps:` keyed format and bare list.

### Script Result

```rust
pub struct ScriptResult {
    pub outputs: Vec<String>,
    pub screenshot: Option<Vec<u8>>,
    pub variables: HashMap<String, String>,
}
```

---

## 12. OxicodeBrowser Backend

**File:** `oxicode-agent/src/tools/browse/oxibrowser_backend.rs`  
**Feature-gated:** `#[cfg(feature = "native-browser")]`

### `OxicodeBrowserEngine`

```rust
pub struct OxicodeBrowserEngine {
    browser: oxibrowser_core::Browser,
    config: BrowseConfig,
    progress: Arc<TabCallbackRegistry>,
    event_task: Mutex<Option<JoinHandle<()>>>,
}
```

### Construction Flow

```
OxicodeBrowserEngine::with_config(config)
  ├── Build oxibrowser_core::BrowserConfig (headless, user_agent, obey_robots, js_timeout_ms)
  ├── oxibrowser_core::Browser::new(config) → browser
  ├── Create TabCallbackRegistry
  ├── Subscribe to browser events: browser.subscribe_events() → events_rx
  ├── Spawn background task:
  │   loop {
  │     events_rx.recv().await → event
  │     extract_event_tab_id(&event) → tab_id
  │     registry.invoke(&tab_id, event.short_label())
  │   }
  └── Return OxicodeBrowserEngine
```

### `BrowserEvent` → `tab_id` Mapping

```rust
fn extract_event_tab_id(event: &BrowserEvent) -> uuid::Uuid {
    match event {
        NavigationStarted { tab_id, .. } => *tab_id,
        WaitingForSelector { tab_id, .. } => *tab_id,
        DocumentReady { tab_id, .. } => *tab_id,
        ScreenshotCaptured { tab_id, .. } => *tab_id,
        _ => uuid::Uuid::nil(),
    }
}
```

### `OxicodeTab`

```rust
pub struct OxicodeTab {
    inner: oxibrowser_core::Tab,
    config: BrowseConfig,
    tab_id: uuid::Uuid,
    registry: Arc<TabCallbackRegistry>,
}
```

- `set_progress_callback(cb)` — registers in the registry
- `clear_progress_callback()` — removes from registry
- Implements `BrowserTab` by delegating to `oxibrowser_core::Tab`
- Overrides `tab_id()` to return the stable ID
- Overrides `as_any()` for downcasting

### Event Routing Architecture

```
oxibrowser-core (Browser)
  │
  ├── broadcast::Sender<BrowserEvent>
  │
  ├── Subscribe → events_rx (in OxicodeBrowserEngine)
  │     │
  │     ├── extract_event_tab_id(event)
  │     │
  │     └── registry.invoke(tab_id, event.short_label())
  │           │
  │           └── ProgressCallback(msg) [registered by BrowseTool]
  │                 │
  │                 └── tool_exec.rs progress wrapper
  │                       │
  │                       └── emit(AgentEvent::ToolExecutionUpdate { ..., tab_id })
  │
  └── Each tab gets its own callback → per-tab event isolation
```

---

## 13. Tab Guard (RAII)

**File:** `oxicode-agent/src/tools/browse/tab_guard.rs`

```rust
pub struct TabGuard {
    tab: Option<Box<dyn BrowserTab>>,
    explicitly_consumed: bool,
}
```

- `new(tab)` — wrap an opened tab
- `tab()` — access the underlying `&dyn BrowserTab`
- `close(self)` — async close + consume (calls `tab.clear_progress_callback()` then `tab.close()`)
- `into_inner(self)` — take ownership without closing
- `Drop` impl: if not explicitly consumed, logs a warning (tab may leak)

### Design Purpose

Prevents tab leaks. Since `Drop` can't be async, the guard warns but can't actually close the tab synchronously. Callers must use `guard.close().await`.

---

## 14. Browse Config

**File:** `oxicode-agent/src/tools/browse/config.rs`

### `BrowseConfig`

| Field | Default | Description |
|-------|---------|-------------|
| `default_wait_timeout_ms` | 10,000 | CSS selector wait timeout |
| `page_timeout_secs` | 30 | Page load timeout |
| `screenshot_width` | 800 | Screenshot viewport width |
| `max_script_steps` | 100 | Max steps per browse_script call |
| `cache_ttl_secs` | 300 | Render cache TTL (0 = disabled) |
| `cache_max_entries` | 50 | Max cache entries |
| `max_concurrent_tabs` | 4 | Tab limit |
| `max_output_bytes` | 512,000 | Output truncation threshold |
| `session_idle_timeout_secs` | 300 | Browse session auto-close timeout |
| `user_agent` | None | Custom User-Agent |
| `obey_robots` | true | Respect robots.txt |
| `js_timeout_ms` | 10,000 | JavaScript evaluation timeout |

---

## 15. Helpers

**File:** `oxicode-agent/src/tools/browse/helpers.rs`

### Link Extraction

- `JS_ALL_LINKS` — const JS snippet returning `[{text, href}]` for all `<a href>` on page
- `js_links_within(selector)` — scoped to a CSS selector root
- `parse_link_values(Value)` — parse JSON array → `Vec<(String, String)>`
- `extract_links(tab)` — evaluate JS_ALL_LINKS on tab
- `format_links(links)` — numbered markdown list

### Element Extraction

- `js_query_elements(selector)` — JS returning `[{tag, text, attributes}]`
- `parse_element_values(Value)` — parse → `Vec<(tag, text, HashMap<String,String>)>`

### DOM Interaction JS

- `js_set_select_value(selector, value)` — set `<select>` + fire change event
- `js_check(selector)` — check checkbox (only if not already checked)
- `js_uncheck(selector)` — uncheck (only if currently checked)

---

## 16. Progress Callback Flow — End to End

### Full Chain (BrowseTool with native-browser)

```
1. Agent::run_with_channel_inner()
   └── Creates emit callback wrapping mpsc::Sender

2. AgentLoop::run_loop()
   └── execute_tool_calls()
       └── execute_tool_calls_sequential()
           └── execute_prepared_tool_call()
               │
               ├── Creates tab_id_slot: Arc<Mutex<Option<Uuid>>>
               ├── Calls tool.set_tab_id_slot(slot)
               │
               ├── Creates progress_cb = |msg: String| {
               │       let tab_id = *tab_id_slot.lock();
               │       emit(AgentEvent::ToolExecutionUpdate {
               │           tool_call_id, tool_name, partial_result: msg, tab_id
               │       })
               │   }
               │
               ├── Calls tool.on_progress(wrapped_progress_cb)
               │   └── BrowseTool stores in pending_callback
               │
               └── Calls tool.execute(tool_call_id, args, None, ctx)
                   │
                   ├── engine.new_tab() → raw_tab (OxicodeTab)
                   ├── Stores tab_id in tab_id_slot
                   ├── Downcasts to OxicodeTab → set_progress_callback(cb)
                   │   └── registry.set(tab_id, cb) in TabCallbackRegistry
                   │
                   ├── tab.goto(url)
                   │   └── [oxibrowser-core internal]
                   │       ├── Emits BrowserEvent::NavigationStarted { tab_id, ... }
                   │       ├── Emits BrowserEvent::DocumentReady { tab_id, ... }
                   │       └── Background drain task:
                   │           extract_event_tab_id(event) → tab_id
                   │           registry.invoke(tab_id, event.short_label())
                   │           → stored callback(msg)
                   │           → progress_cb(msg)
                   │           → emit(ToolExecutionUpdate { ..., tab_id })
                   │
                   ├── tab.wait_for() / tab.query_all() / tab.screenshot()
                   │   └── Same event flow for relevant BrowserEvents
                   │
                   ├── guard.close().await
                   │   └── tab.clear_progress_callback() → registry.clear(tab_id)
                   │
                   └── Return AgentToolResult::success(output)
```

### What Events Actually Flow

When BrowseTool navigates to a URL:

1. **`ToolExecutionStart`** — emitted before execute (in tool_exec.rs)
2. **`ToolExecutionUpdate`** (tab_id = Some) — "Opening https://..." (from NavigationStarted)
3. **`ToolExecutionUpdate`** (tab_id = Some) — "Loaded https://..." (from DocumentReady)
4. **`ToolExecutionUpdate`** (tab_id = Some) — possibly "Waiting for selector..." (if wait_for used)
5. **`ToolExecutionEnd`** — emitted after execute returns

### Tools WITHOUT Progress Callbacks

- **BrowseSessionTool** — no `on_progress()` override, no tab_id_slot. Each action (goto, click, etc.) produces only `ToolExecutionStart` and `ToolExecutionEnd`, with **no** `ToolExecutionUpdate` events in between.
- **BrowseExtractTool** — same: no progress callbacks.
- **BrowseScriptTool** — same: no progress callbacks.

This means multi-step browse sessions and scripts are **opaque** — the UI sees only "started" and "finished" with no intermediate progress.

---

## 17. Gaps & Opportunities for Exploration Transparency

### Current State Summary

| Tool | Progress Callbacks | Tab ID Tracking | Inter-step Events |
|------|-------------------|-----------------|-------------------|
| BrowseTool | ✅ Yes (via engine events) | ✅ Yes (tab_id_slot) | ✅ NavigationStarted, DocumentReady |
| BrowseSessionTool | ❌ No | ❌ No | ❌ None |
| BrowseExtractTool | ❌ No | ❌ No | ❌ None |
| BrowseScriptTool | ❌ No | ❌ No | ❌ None |

### Gap 1: BrowseSessionTool Has No Progress Visibility

The session tool manages a persistent tab but never registers a progress callback. Each action (goto, click, fill, etc.) runs silently — the UI only sees `ToolExecutionStart` → `ToolExecutionEnd`. There's no indication of:
- What URL is being navigated to
- Whether an element was found
- What the current page title/URL is after navigation
- Screenshot previews

### Gap 2: BrowseScriptTool Has No Step-Level Events

The script tool executes N steps sequentially but emits zero `ToolExecutionUpdate` events. The UI can't show:
- Which step is currently executing (step 3 of 10)
- What action is being performed
- Partial results from evaluate/extract steps

### Gap 3: BrowseExtractTool Has No Progress

Even simple extraction could benefit from:
- "Navigating to URL..."
- "Extracting elements matching selector..."
- Count of extracted elements

### Gap 4: Structured Progress Callback Is Unused

The `on_structured_progress()` method exists on `AgentTool` but is **never wired** in `tool_exec.rs`. Only `on_progress()` (the string-based version) is connected. This means tools can define structured progress, but it never reaches the event stream.

### Gap 5: BrowserEvent Types Are Lost in Translation

`oxibrowser-core` emits typed events (`NavigationStarted`, `DocumentReady`, etc.), but by the time they reach the agent event stream, they're collapsed into a single string via `event.short_label()`. The structured information (URL, status code, timing) is lost.

### Gap 6: No Screenshot Streaming

BrowseTool can capture screenshots, but only as a final result. There's no mechanism to stream intermediate screenshots during navigation (e.g., "here's what the page looks like right now" while waiting for a selector).

### Architecture Leverage Points for Improvement

1. **TabCallbackRegistry already supports per-tab routing** — extending BrowseSessionTool to register callbacks on its persistent tab would enable inter-action progress without changing the engine.

2. **ToolExecutionUpdate already has tab_id** — the event type is ready for per-tab event aggregation in the UI.

3. **Structured ToolProgress exists** — wiring `on_structured_progress` in `tool_exec.rs` would enable richer progress events without changing the AgentEvent enum.

4. **The emit callback is already available** — passing it through to tools (or wrapping in a trait) would let tools emit custom events directly.

5. **BrowseScriptTool has step-level context** — the `execute_single_step()` function already knows the step index and type, making it straightforward to emit progress at each step boundary.
