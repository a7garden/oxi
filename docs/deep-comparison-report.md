# pi-mono vs oxicode — Deep Architecture Comparison & Fix Report

Generated from exhaustive subagent analysis of all source files.

---

## 🔴 Critical Bugs Fixed (Phase 1)

### 1. `should_terminate_batch` — Wrong Logic (FIXED ✅)
- **pi-mono**: ALL tool results must have `terminate === true` → batch terminates
- **oxicode (before)**: ANY tool result with `terminate` → batch terminates
- **oxicode (after)**: Changed to `all(|f| f.result.terminate)` ✅

### 2. `ToolExecutionUpdate` Never Emitted (FIXED ✅)
- **pi-mono**: Tool progress callback emits structured partial results
- **oxicode (before)**: Created `progress_cb` but immediately discarded (`let _ = progress_cb`)
- **oxicode (after)**: Now wires up `tool.on_progress()` with the callback ✅

### 3. Provider Error Missing `MessageEnd` (FIXED ✅)
- **pi-mono**: On stream error, always emits `message_end` before error
- **oxicode (before)**: Emits `Error` event, breaks loop, NO `MessageEnd`
- **oxicode (after)**: If `message_started`, emits `MessageEnd` with error message first ✅

### 4. `AgentEnd` Event (FIXED ✅)
- **pi-mono**: Emits `agent_end` at end of run (always)
- **oxicode (before)**: Only emitted `Complete`, not `AgentEnd`
- **oxicode (after)**: Now emits `AgentEnd { messages, stop_reason }` before `Complete` ✅

### 5. `AgentStart` Event (FIXED ✅)
- **pi-mono**: Emits `agent_start` at beginning
- **oxicode (before)**: Only emitted legacy `Start { prompt }`
- **oxicode (after)**: Now emits `AgentStart { prompts, session_id }` ✅

---

## 🔴 Critical Issues Remaining

### A. Streaming Event Granularity — `MessageUpdate` carries text delta only

**pi-mono**: `message_update` carries full `AssistantMessage` snapshot PLUS `assistantMessageEvent` (typed streaming event: `text_delta`, `thinking_delta`, `toolcall_delta`). This tells consumers EXACTLY what changed at each step.

**oxicode**: `MessageUpdate { message, delta: Option<String> }` — `delta` is only the text string. Consumers cannot distinguish text streaming from thinking streaming from tool-call events from `message_update` alone.

**Impact**: TUI cannot show "thinking" visual state during reasoning blocks. Tool call streaming (when provider streams partial arguments) is invisible to consumers.

**Fix needed**: Change `MessageUpdate.delta` from `Option<String>` to typed streaming event, OR add a new `MessageStreamDelta` event with the raw provider event type. Alternatively, add an enum field:
```rust
pub enum MessageUpdateDetail {
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStart { tool_call_id: String },
    ToolCallDelta { delta: String },
    ToolCallEnd { tool_call: ToolCall },
}
```

### B. Two Divergent Agent Implementations

**oxicode has TWO agent loops** with different behavior:
1. `agent.rs::Agent` — inline loop, uses legacy events (`Start`, `Complete`, `ToolStart`, `ToolComplete`, `TextChunk`)
2. `agent_loop/mod.rs::AgentLoop` — separate module, uses structured events (`MessageStart`, `MessageUpdate`, `MessageEnd`)

The TUI uses `Agent.run_with_channel()` (the inline one from agent.rs), NOT `AgentLoop`. These have different event semantics, different tool execution, different hook signatures.

**pi-mono**: Single `Agent` class wrapping a single `agent-loop.ts`.

**Fix needed**: Consolidate to ONE agent loop. Either:
- Make `Agent.run_with_channel()` delegate to `AgentLoop`, OR
- Remove `AgentLoop` entirely and make `Agent` the only implementation

**Current state**: Confusing — `agent.rs` uses legacy events, `agent_loop/` uses structured events. The TUI event forwarder must handle BOTH sets.

### C. `shouldStopAfterTurn` Hook — Stub Context

**pi-mono**: Receives `{ message, toolResults, context, newMessages }` — full state snapshot.

**oxicode (agent.rs)**: Receives dummy `AssistantMessage` with empty content + empty `tool_results`.

**Fix needed**: Build actual context before calling hook:
```rust
let ctx = ShouldStopAfterTurnContext {
    message: assistant_message.clone(),
    tool_results: tool_result_messages.clone(),
    iteration: self.state.get_state().iteration,
    // messages: self.state.get_state().messages.clone(), // Add this
};
```

### D. Queue Drain Notification — Not Wired

**pi-mono**: On `message_start(user)` with text matching a queued message → remove from queue → emit `queue_update`.

**oxicode**: Queues are drained by agent hooks, but the event forwarder doesn't detect this (the agent hook doesn't emit a corresponding event). Added `SteeringMessage`/`FollowUpMessage` event forwarding but it checks queues AFTER the event (race condition possible).

**Fix needed**: The proper fix is in `agent.rs`:
```rust
// When agent drains steering messages, emit SteeringMessage event
// so session can detect and emit queue_update
```
Currently no such event is emitted when `drain_steering_messages()` returns items.

### E. `finishRun` / Cleanup Not Guaranteed

**pi-mono**: `runWithLifecycle` has `finally` block that always calls `finishRun()` — clears `isStreaming`, `streamingMessage`, `pendingToolCalls`.

**oxicode (agent.rs)**: No `finally` block. If `run_with_channel` panics mid-stream, state is left dirty.

**Fix needed**: Add `std::panic::catch_unwind` or reorganize with a cleanup guard.

### F. `afterToolCall` Errors Silently Ignored

**pi-mono**: If `afterToolCall` throws, creates error tool result.

**oxicode (AgentLoop)**: `after_tool_call` returns `Result<Option<AgentToolResult>>`, errors are flattened with `.ok().flatten()` — silently dropped.

**Fix needed**: Change error handling to create error result on hook failure.

### G. `turn_end` Not Emitted on Error/Abort

**pi-mono**: Even on error/abort, emits `turn_end` with the assistant message.

**oxicode (agent.rs)**: On `StopReason::Error`, breaks outer loop, does NOT emit `TurnEnd`.

**Fix needed**: Add `TurnEnd` emission before error exit.

---

## 🟡 Important Gaps

### H. No `transformContext` / `convertToLlm` Hooks
- pi-mono: Optional context transformation before LLM call
- oxicode: No equivalent

### I. Static API Key Only
- pi-mono: `getApiKey(provider)` hook per-call
- oxicode: Static `api_key` in config — can't refresh expiring tokens

### J. Argument Validation Missing
- pi-mono: `validateToolArguments(tool, preparedArgs)`
- oxicode: Passes raw JSON args without schema validation

### K. Queue Drain Modes Missing
- pi-mono: `"all"` vs `"one-at-a-time"` per queue
- oxicode: Always drains all

### L. No `prepareNextTurn` Hook
- pi-mono: Can replace context/model/thinkingLevel between turns
- oxicode: No equivalent

### M. `isStreaming` / `streamingMessage` Not Tracked
- pi-mono: Exposes streaming state for consumers to introspect
- oxicode: No tracking of in-flight streaming state

### N. `abort()` Only on AgentLoop, Not Agent
- pi-mono: `agent.abort()` cancels running loop
- oxicode: `Agent` has no abort; `AgentLoop` has `cancel_auto_retry()` only

### O. `waitForIdle()` Missing
- pi-mono: `agent.waitForIdle()` returns promise after `agent_end` settles
- oxicode: No equivalent

### P. Message Start/End for User Prompts Missing
- pi-mono: Emits `message_start/end` for initial user prompts
- oxicode (AgentLoop)`: Does NOT emit events for initial user prompts

---

## 🟢 Session Layer Gaps

### Q. No Serial Event Processing Queue
- pi-mono: `_agentEventQueue` — promise chain serializes all async processing
- oxicode: Events processed in parallel paths (prompt batch vs streaming)

### R. Extension Event Delivery Order Reversed
- pi-mono: Extensions FIRST, then UI listeners
- oxicode: UI listeners FIRST, then extensions

### S. Message Replacement by Extensions Missing
- pi-mono: `message_end` extensions can return replacement message
- oxicode: No equivalent

### T. Overflow Recovery Incomplete
- pi-mono: One-shot overflow recovery with guard
- oxicode: Field exists but no logic

### U. Compaction Disconnect/Reconnect Missing
- pi-mono: Disconnects from agent events during compaction
- oxicode: No disconnect pattern

---

## Summary: Fixed vs Remaining

| # | Issue | Status |
|---|-------|--------|
| 1 | `should_terminate_batch` uses ANY not ALL | ✅ FIXED |
| 2 | `ToolExecutionUpdate` discarded | ✅ FIXED |
| 3 | Provider error missing `MessageEnd` | ✅ FIXED |
| 4 | `AgentEnd` never emitted | ✅ FIXED |
| 5 | `AgentStart` never emitted | ✅ FIXED |
| 6 | MessageUpdate carries text delta only | 🔴 REMAINING |
| 7 | Two divergent agent implementations | 🔴 REMAINING |
| 8 | `shouldStopAfterTurn` stub context | 🔴 REMAINING |
| 9 | Queue drain notification | 🔴 PARTIAL (added events, not wired) |
| 10 | `finishRun` cleanup not guaranteed | 🔴 REMAINING |
| 11 | `afterToolCall` errors silent | 🔴 REMAINING |
| 12 | `turn_end` missing on error | 🔴 REMAINING |
| 13 | No `transformContext`/`convertToLlm` | 🟡 GONE (not needed with direct Message) |
| 14 | Static API key only | 🟡 REMAINING |
| 15 | No argument validation | 🟡 REMAINING |
| 16 | No queue drain modes | 🟡 REMAINING |
| 17 | No `prepareNextTurn` hook | 🟡 REMAINING |
| 18 | `isStreaming` not tracked | 🟡 REMAINING |
| 19 | No `abort()` on Agent | 🟡 REMAINING |
| 20 | No `waitForIdle()` | 🟡 REMAINING |
| 21 | No message events for user prompts | 🟡 REMAINING |
| 22 | No serial event queue | 🟢 SESSION LAYER |
| 23 | Extension delivery order reversed | 🟢 SESSION LAYER |
| 24 | Message replacement by extensions | 🟢 SESSION LAYER |
| 25 | Overflow recovery incomplete | 🟢 SESSION LAYER |
| 26 | Compaction disconnect/reconnect | 🟢 SESSION LAYER |

## Next Steps (Priority Order)

1. **Consolidate to single agent loop** (Issue #7) — pick `AgentLoop` or `Agent` as canonical, remove the other
2. **Fix `MessageUpdate` granularity** (Issue #6) — add typed delta information
3. **Wire queue drain notification** (Issue #9) — emit `SteeringMessage`/`FollowUpMessage` when agent actually consumes
4. **Add `TurnEnd` on error** (Issue #12) — complete the lifecycle
5. **Add `finishRun` guard** (Issue #10) — ensure cleanup on panic
6. **Build real `shouldStopAfterTurn` context** (Issue #8) — pass actual state
7. **Handle `afterToolCall` errors properly** (Issue #11) — create error result on hook failure