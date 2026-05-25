# RFC: Browser Interactive Sessions

> **Status**: Proposed
> **Affects**: `oxi-agent/src/tools/browse/`, `oxi-sdk/src/agent_builder.rs`, `oxi-sdk/src/tool_factory.rs`
> **Depends on**: existing `BrowserEngine` / `BrowserTab` trait layer, `TabGuard`

## Problem

The current browser tools (`BrowseTool`, `BrowseExtractTool`, `BrowseScriptTool`) are all **per-request**: each tool call opens a new tab, performs work, and closes it. This covers most use cases, but agents sometimes need to maintain browser state **across multiple tool calls** — with reasoning steps in between.

Example: An agent fills out a multi-page form, reads intermediate results, decides what to do next, then continues — all within the same browser tab.

Today this is only possible via `BrowseScriptTool` (all steps in one YAML script, one tool call). But scripts can't reason between steps.

### Use cases

1. **Multi-page form filling** — navigate → fill → submit → read result → decide → continue.
2. **Login-protected content** — open login page → fill credentials → submit → wait for redirect → browse authenticated content.
3. **Search-and-refine** — search → extract results → refine query → search again, all in the same tab with cookies preserved.
4. **SPA interaction** — navigate to a single-page app, interact with dynamic elements, extract data that only appears after specific click sequences.

## Proposal

Add a fourth tool — `BrowseSessionTool` (`browse_session`) — that manages a **persistent tab** across tool calls. The agent opens a session, performs operations one by one (each as a separate tool call), and closes when done.

### Design

```
Agent → browse_session(action="open")                  → session started, tab created
Agent → browse_session(action="goto", url=...)          → navigates the same tab, returns page metadata
Agent → browse_session(action="click", selector=...)    → clicks on the same page
Agent → browse_session(action="fill", selector=..., value=...) → fills an input
Agent → browse_session(action="content", format=...)    → reads current page (markdown/html/text/links)
Agent → browse_session(action="evaluate", javascript=...) → runs JS, returns JSON result
Agent → browse_session(action="close")                  → tab closed, session ended
```

Each action returns a structured result with relevant metadata. The agent can reason between calls.

### Tool Schema

```json
{
  "name": "browse_session",
  "description": "Interactive browser session with a persistent tab across calls. Open a session, perform multiple operations, then close when done. The tab retains cookies, localStorage, and DOM state between actions. Use for multi-step interactions like form filling, login flows, and SPA exploration where reasoning is needed between steps.",
  "parameters": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": [
          "open",
          "goto",
          "back",
          "forward",
          "reload",
          "click",
          "fill",
          "type",
          "clear",
          "press",
          "select",
          "check",
          "uncheck",
          "scroll",
          "wait_for",
          "content",
          "query_all",
          "extract_links",
          "evaluate",
          "screenshot",
          "close"
        ],
        "description": "Session action to perform"
      },
      "url": {
        "type": "string",
        "description": "URL to navigate to (goto action)"
      },
      "selector": {
        "type": "string",
        "description": "CSS selector (click, fill, type, clear, select, check, uncheck, wait_for, query_all)"
      },
      "value": {
        "type": "string",
        "description": "Value to fill/type/select (fill, type, select actions)"
      },
      "combo": {
        "type": "string",
        "description": "Key combo (press action, e.g. 'Enter', 'Control+a')"
      },
      "pixels": {
        "type": "integer",
        "description": "Scroll distance in pixels (scroll action, positive = down)"
      },
      "javascript": {
        "type": "string",
        "description": "JS expression to evaluate (evaluate action)"
      },
      "format": {
        "type": "string",
        "enum": ["markdown", "html", "text", "links"],
        "default": "markdown",
        "description": "Output format for content action"
      },
      "timeout_ms": {
        "type": "integer",
        "default": 10000,
        "description": "Timeout in ms (wait_for action)"
      },
      "width": {
        "type": "integer",
        "default": 800,
        "description": "Viewport width for screenshot (default: 800)"
      }
    },
    "required": ["action"]
  }
}
```

### Action catalog

Feature-parity with `BrowseScriptTool`'s `Step` enum. Every script step has an equivalent session action.

| Action | Required params | Optional params | Returns | BrowserTab method |
|--------|----------------|-----------------|---------|-------------------|
| `open` | — | — | `{ "status": "ok", "session_id": "..." }` | `engine.new_tab()` |
| `goto` | `url` | — | `{ "status": "ok", "url": "...", "title": "...", "status_code": 200 }` | `tab.goto(url)` |
| `back` | — | — | `{ "status": "ok" }` | `tab.evaluate("history.back()")` |
| `forward` | — | — | `{ "status": "ok" }` | `tab.evaluate("history.forward()")` |
| `reload` | — | — | `{ "status": "ok" }` | `tab.evaluate("location.reload()")` |
| `click` | `selector` | — | `{ "status": "ok" }` | `tab.click(selector)` |
| `fill` | `selector`, `value` | — | `{ "status": "ok" }` | `tab.fill(selector, value)` |
| `type` | `selector`, `value` | — | `{ "status": "ok" }` | `tab.type_(selector, value)` |
| `clear` | `selector` | — | `{ "status": "ok" }` | `tab.fill(selector, "")` |
| `press` | `combo` | — | `{ "status": "ok" }` | `tab.press(combo)` |
| `select` | `selector`, `value` | — | `{ "status": "ok" }` | JS via `helpers::js_set_select_value` |
| `check` | `selector` | — | `{ "status": "ok" }` | JS via `helpers::js_check` |
| `uncheck` | `selector` | — | `{ "status": "ok" }` | JS via `helpers::js_uncheck` |
| `scroll` | — | `pixels` (default 300) | `{ "status": "ok" }` | `tab.evaluate("window.scrollBy(0, N)")` |
| `wait_for` | `selector` | `timeout_ms` | `{ "status": "ok" }` | `tab.wait_for(selector, timeout)` |
| `content` | — | `format` | `{ "status": "ok", "url": "...", "title": "...", "content": "..." }` | `tab.content()` + format |
| `query_all` | `selector` | — | `{ "status": "ok", "results": ["...", "..."] }` | `tab.query_all(selector)` |
| `extract_links` | — | `selector` (scoped) | `{ "status": "ok", "links": [{ "text": "...", "href": "..." }] }` | JS via `helpers` |
| `evaluate` | `javascript` | — | `{ "status": "ok", "result": <value> }` | `tab.evaluate(javascript)` |
| `screenshot` | — | `width` | Image block (base64 PNG) | `tab.screenshot(width)` |
| `close` | — | — | `{ "status": "ok" }` | `tab.close()` |

### Action responses

All actions return a JSON result. The `content` field of `AgentToolResult` contains the JSON. For `screenshot`, an additional `ContentBlock::Image` is appended.

**Success example** (`goto`):
```json
{
  "status": "ok",
  "url": "https://example.com/dashboard",
  "title": "Dashboard",
  "status_code": 200
}
```

**Error example** (no session):
```json
{
  "status": "error",
  "error": "No active session. Call action='open' first."
}
```

## Implementation

### New file

`oxi-agent/src/tools/browse/browse_session_tool.rs`

```rust
pub struct BrowseSessionTool {
    engine: Arc<dyn BrowserEngine>,
    tab: Arc<Mutex<Option<TabGuard>>>,
    config: BrowseConfig,
    opened_at: Arc<Mutex<Option<tokio::time::Instant>>>,
}
```

Key design decisions:

- **`TabGuard` reuse**: The internal tab is wrapped in `TabGuard` (already defined in `tab_guard.rs`). This gives us RAII leak-prevention with `tracing::warn` on implicit drop, consistent with `BrowseTool` and `BrowseScriptTool`.
- **`Arc<Mutex<Option<TabGuard>>>`**: Allows the same `BrowseSessionTool` instance to be shared across `execute()` calls (which is `&self`). The `Mutex` serializes concurrent action invocations.
- **`opened_at`**: Tracks when the session was opened, enabling idle-timeout enforcement.

### Action dispatch

```rust
match action {
    "open"      => self.do_open().await,
    "goto"      => self.with_tab(|t| t.goto(url)).await,
    "back"      => self.with_tab(|t| t.evaluate("history.back()")).await,
    "forward"   => self.with_tab(|t| t.evaluate("history.forward()")).await,
    "reload"    => self.with_tab(|t| t.evaluate("location.reload()")).await,
    "click"     => self.with_tab(|t| t.click(selector)).await,
    "fill"      => self.with_tab(|t| t.fill(selector, value)).await,
    "type"      => self.with_tab(|t| t.type_(selector, value)).await,
    "clear"     => self.with_tab(|t| t.fill(selector, "")).await,
    "press"     => self.with_tab(|t| t.press(combo)).await,
    "select"    => self.with_tab(|t| t.evaluate(&js_set_select_value(sel, val))).await,
    "check"     => self.with_tab(|t| t.evaluate(&js_check(sel))).await,
    "uncheck"   => self.with_tab(|t| t.evaluate(&js_uncheck(sel))).await,
    "scroll"    => self.with_tab(|t| t.evaluate(&format!("window.scrollBy(0, {})", px))).await,
    "wait_for"  => self.with_tab(|t| t.wait_for(sel, timeout)).await,
    "content"   => self.do_content(format).await,
    "query_all" => self.with_tab(|t| t.query_all(sel)).await,
    "extract_links" => self.do_extract_links(selector).await,
    "evaluate"  => self.with_tab(|t| t.evaluate(js)).await,
    "screenshot"=> self.do_screenshot(width).await,
    "close"     => self.do_close().await,
    _ => Err("Unknown action".into()),
}
```

The `with_tab()` helper:
1. Locks the `Mutex`.
2. Returns error if `tab` is `None` (no session).
3. Calls the closure with `tab.tab()` (borrows the `BrowserTab` from the `TabGuard`).
4. Maps `BrowserError` to a JSON error result.

### Idle timeout

Sessions auto-close after `BrowseConfig::session_idle_timeout_secs` of inactivity. Checked at the start of each `execute()` call:

```rust
if let Some(opened) = *self.opened_at.lock().await {
    let elapsed = opened.elapsed().as_secs();
    if elapsed > self.config.session_idle_timeout_secs as u64 {
        tracing::warn!("Session expired after {}s of inactivity", elapsed);
        self.do_close().await?;
        return Err("Session expired due to inactivity. Open a new session.".into());
    }
}
```

### `content` action detail

The `content` action is the main "reading" action. It supports the same formats as `BrowseTool`:

| Format | Behavior |
|--------|----------|
| `markdown` (default) | Returns `page.markdown` |
| `html` | Returns `page.html` |
| `text` | Returns `page.markdown` (plain text) |
| `links` | Extracts all links via `helpers::extract_links()` and formats as numbered list |

When `selector` is also provided, narrows extraction to matching elements (same as `BrowseTool` behavior).

### Screenshot action

Returns both:
1. A text summary in the JSON result: `{ "status": "ok", "size_bytes": 12345 }`.
2. A `ContentBlock::Image` with base64-encoded PNG appended to the result.

This matches how `BrowseTool` handles screenshots.

### Drop behavior

On `Drop`, if the `TabGuard` is still `Some`, it gets dropped naturally — which triggers the existing `TabGuard` drop warning. The `Mutex` makes this safe.

## Changes to existing files

### `config.rs` — new field

```rust
pub struct BrowseConfig {
    // ... existing fields ...

    /// Maximum idle time (seconds) before a session auto-closes.
    /// 0 = no timeout (not recommended).
    #[serde(default = "default_session_idle_timeout_secs")]
    pub session_idle_timeout_secs: u64,
}

fn default_session_idle_timeout_secs() -> u64 {
    300 // 5 minutes
}
```

### `mod.rs` — new module + re-export

```rust
// Feature-gated: requires oxibrowser-core (same as BrowseScriptTool)
#[cfg(feature = "native-browser")]
pub mod browse_session_tool;

#[cfg(feature = "native-browser")]
pub use browse_session_tool::BrowseSessionTool;
```

**Why feature-gated**: `BrowseSessionTool` requires a concrete `BrowserEngine` implementation (to call `new_tab()`). Without the `native-browser` feature, there is no engine to construct — the tool would be inert. This matches `BrowseScriptTool`'s existing pattern. The trait layer (`BrowserEngine`, `BrowserTab`) remains always-compiled.

### `helpers.rs` — no changes needed

All JS helpers (`js_check`, `js_uncheck`, `js_set_select_value`, `extract_links`, etc.) are already shared across tools. `BrowseSessionTool` imports them directly.

### `engine.rs` — no changes needed

No new trait methods required. All session actions delegate to existing `BrowserTab` methods or `evaluate()` with JS snippets.

### `tab_guard.rs` — no changes needed

`TabGuard` already provides exactly what we need: RAII wrapper with leak warning.

## Registration changes

### `oxi-sdk/src/agent_builder.rs`

Add a new builder method alongside existing `browsing()`:

```rust
#[cfg(feature = "native-browser")]
/// Register all browser tools including persistent session support.
///
/// Like [`browsing()`](Self::browsing) but also registers `browse_session`
/// for multi-step interactive sessions with a persistent tab.
pub fn browsing_with_session(self, engine: Arc<dyn BrowserEngine>) -> Self {
    self.tools.register(BrowseTool::new(Arc::clone(&engine)));
    self.tools.register(BrowseExtractTool::new(Arc::clone(&engine)));
    self.tools.register(BrowseScriptTool::new(Arc::clone(&engine)));
    self.tools.register(BrowseSessionTool::new(engine));
    self
}
```

Note: This also registers `BrowseScriptTool`, which the current `browsing()` method does **not** include. This is intentional — `browsing_with_session` is the "full browser suite" registration.

### `oxi-sdk/src/tool_factory.rs`

Add session-aware factory functions:

```rust
#[cfg(feature = "native-browser")]
/// All browser tools including session support.
pub fn browsing_tools_with_session(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    let registry = browsing_tools(Arc::clone(&engine));
    registry.register(BrowseScriptTool::new(Arc::clone(&engine)));
    registry.register(BrowseSessionTool::new(engine));
    registry
}
```

### `oxi-sdk/src/lib.rs`

Add re-export:

```rust
#[cfg(feature = "native-browser")]
pub use oxi_agent::tools::browse::BrowseSessionTool;
```

## Registration pattern (SDK consumer side)

```rust
// Full browser suite with session support
let engine = Arc::new(OxiBrowserEngine::new()?);
let agent = oxi.agent(config)
    .browsing_with_session(engine)  // browse + browse_extract + browse_script + browse_session
    .build()?;

// Without session (current behavior — browse + browse_extract only)
let agent = oxi.agent(config)
    .browsing(engine)
    .build()?;

// Via tool_factory
let tools = browsing_tools_with_session(engine);
```

## Concurrency safety

- The `Mutex<Option<TabGuard>>` ensures only one action runs at a time on the tab.
- A single `BrowseSessionTool` instance is designed for one agent. If multiple agents need concurrent sessions, each gets its own `BrowseSessionTool` instance (one `ToolRegistry` per agent — standard pattern).
- No cross-agent tab sharing. Each session is isolated.

## What this does NOT change

- `BrowseTool` — unchanged, still per-request
- `BrowseExtractTool` — unchanged, still per-request
- `BrowseScriptTool` — unchanged, still single-call multi-step
- `BrowserEngine` / `BrowserTab` traits — no new methods
- `TabGuard` — no changes, reused as-is
- `helpers.rs` — no changes, shared JS helpers reused as-is
- `oxibrowser-core` dependency — no version bump required

## Why a separate tool (not a parameter on BrowseTool)

- **Separation of concerns**: Per-request tools have clean lifecycle (open → work → close in one call). Session tool has explicit open/close spanning multiple calls.
- **Agent clarity**: The model sees `browse` vs `browse_session` and immediately understands the semantic difference.
- **No parameter explosion**: `BrowseTool` already has 5 parameters. Adding session lifecycle would make it harder for the model to use correctly.
- **Registration flexibility**: Consumers opt into session support independently. Existing `browsing()` registrations are unaffected.
- **Feature gate isolation**: `BrowseSessionTool` can be feature-gated independently if needed in the future.

## Future considerations

- **Multi-tab sessions**: Currently one tab per session. A `new_tab` action could be added later for multi-tab workflows (popup handling, cross-tab comparison).
- **Session history**: The `content` action could optionally return a diff-friendly summary of what changed since the last content read.
- **Persistent cookies across sessions**: Currently cookies die with the tab. A `BrowserEngine`-level cookie jar could persist across sessions.
