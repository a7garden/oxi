# Feature: Expose streaming lifecycle events as AgentEvent

## Summary

`oxicode-ai` already emits `ProviderEvent::ToolCallDelta` and `ProviderEvent::ThinkingEnd` during streaming, but `oxicode-agent`'s streaming loop does not forward them as `AgentEvent` variants. Downstream consumers (Oxios kernel, any UI bridge) cannot see tool-argument construction or reasoning-end boundaries.

Three changes, all in `oxicode-agent/src/agent_loop/streaming.rs`:

## 1. `AgentEvent::ThinkingEnd` (P1, ~5 lines)

### Problem

`ProviderEvent::ThinkingEnd` exists in oxicode-ai but is **not handled** in the streaming loop. It falls through to the catch-all, so the agent never signals "reasoning finished, answer starting."

```rust
// oxicode-ai/src/high_level.rs:102 — already exists
ProviderEvent::ThinkingEnd { content_index, content, .. }
```

```rust
// oxicode-agent/src/agent_loop/streaming.rs — NOT handled
// (only ThinkingStart at :305 and ThinkingDelta at :313)
```

### Proposed

Add to `AgentEvent`:

```rust
/// The model finished its reasoning/thinking span and is about to
/// produce the answer. Signal-only (no payload).
ThinkingEnd,
```

Add to streaming loop:

```rust
ProviderEvent::ThinkingEnd { partial, .. } if added_partial => {
    let last_idx = messages.len() - 1;
    if let Message::Assistant(ref mut m) = messages[last_idx] {
        *m = (*partial).clone();
    }
    emit(super::AgentEvent::ThinkingEnd);
}
```

### Impact

Oxios currently synthesizes `reasoning.end` from the first `TextChunk` after reasoning (Phase B gateway collector). This works for single-span reasoning (most models) but is imprecise for interleaved reasoning models (Claude 4, o3) that alternate reasoning ↔ text multiple times. `ThinkingEnd` gives the exact boundary.

---

## 2. `AgentEvent::ToolCallDelta` (P0, ~10 lines)

### Problem

`ProviderEvent::ToolCallDelta` is received and consumed for internal message accumulation, but **no `AgentEvent` is emitted**. Downstream consumers see only the final parsed args in `ToolExecutionStart` — they never see the LLM constructing the tool call token-by-token.

```rust
// oxicode-agent/src/agent_loop/streaming.rs:360 — currently silent
ProviderEvent::ToolCallDelta { partial, .. } if added_partial => {
    let last_idx = messages.len() - 1;
    if let Message::Assistant(ref mut m) = messages[last_idx] {
        *m = (*partial).clone();
    }
    // ← no emit() call — downstream never sees this
}
```

### Proposed

Add to `AgentEvent`:

```rust
/// Partial tool-call arguments streamed by the LLM before the call
/// is finalized. Emitted between `ToolCallStart` and `ToolCallEnd`
/// in the provider stream. The `delta` is a raw JSON fragment
/// (not valid JSON on its own) — accumulate per `tool_call_id`.
ToolCallDelta {
    /// Tool call identifier (from ToolCallStart).
    tool_call_id: String,
    /// Raw JSON argument fragment from the LLM stream.
    args_delta: String,
},
```

Add to streaming loop (in the `ToolCallDelta` handler):

```rust
ProviderEvent::ToolCallDelta { delta, partial, content_index, .. } => {
    if added_partial {
        let last_idx = messages.len() - 1;
        if let Message::Assistant(ref mut m) = messages[last_idx] {
            *m = (*partial).clone();
        }
    }
    // NEW: extract tool_call_id from the partial message's content block
    // at content_index, then emit the delta.
    let tool_call_id = extract_tool_call_id(&messages, content_index);
    if let Some(id) = tool_call_id {
        emit(super::AgentEvent::ToolCallDelta {
            tool_call_id: id,
            args_delta: delta,
        });
    }
}
```

Where `extract_tool_call_id` reads the `ToolCall` content block at `content_index` from the accumulated assistant message and returns its `id`.

### Impact

This is the **single most impactful** change for LobeHub chat UX parity. With this event, the frontend can show the LLM constructing tool arguments in real-time (LobeHub's signature feature). Without it, tool args appear as a complete block only when `ToolExecutionStart` fires — the user sees nothing during construction.

Oxios kernel → WS chunk mapping (already built, waiting for this event):
```
AgentEvent::ToolCallDelta → KernelEvent::ToolArgsDelta → WS { type: "tool_call_delta", tool_call_id, args_delta }
```

Frontend adapter (already typed, waiting for this chunk):
```typescript
// web/src/lib/stream/ChatEvent.ts
| { kind: 'tool.args_delta'; messageId: string; toolCallId: string; argsDelta: string }
```

---

## 3. Periodic `AgentEvent::Usage` (P2, optional)

### Problem

`AgentEvent::Usage` is emitted once at turn end. No mid-stream token counter.

### Proposed

In the streaming loop, emit `AgentEvent::Usage` every N deltas (e.g. every 100 tokens) using a counter:

```rust
token_count += delta.len(); // rough char-based estimate
if token_count % 400 == 0 {
    emit(super::AgentEvent::Usage {
        input_tokens: 0,  // unknown mid-stream
        output_tokens: token_count / 4, // rough estimate
    });
}
```

Note: this is a rough estimate. True mid-stream usage requires provider-level support (OpenAI sends `usage` in the final chunk only; some providers support `stream_options: { include_usage: true }`).

### Impact

Low priority. Oxios kernel can estimate locally from delta count. Only needed if the frontend wants a live counter matching the provider's numbers.

---

## Implementation Notes

- All three changes are in `oxicode-agent/src/agent_loop/streaming.rs` (streaming loop) and `oxicode-agent/src/events.rs` (enum definition).
- `ProviderEvent` variants already exist for all three — no oxicode-ai changes needed.
- `AgentEvent` is `#[non_exhaustive]`, so adding variants is a minor (non-breaking) version bump.
- Backward compat: existing consumers that pattern-match on `AgentEvent` will see new variants and must have a `_ =>` catch-all (which they should already have for `#[non_exhaustive]` enums).

## Testing

- Unit test: feed a mock `ProviderEvent` stream containing `ToolCallDelta` and `ThinkingEnd`, assert the corresponding `AgentEvent`s are emitted.
- Integration: run a real agent with a reasoning-capable model (Claude/GLM), capture the event sequence, verify `ThinkingEnd` fires between reasoning and text, and `ToolCallDelta` fires before `ToolExecutionStart`.

## Context

This request comes from the Oxios LobeHub chat port effort. 15 commits shipped frontend Phases 1-6 + backend Phases A/B/E/D. The frontend adapter and kernel WS handler are already built to consume these events — they just never arrive because oxicode-agent doesn't emit them.

Design docs:
- `oxios/docs/designs/2026-07-21-lobehub-chat-port-design.md` (frontend)
- `oxios/docs/designs/2026-07-21-lobehub-backend-streaming-design.md` (backend)
- `oxios/docs/designs/2026-07-21-lobehub-port-remaining-work.md` (remaining work tracker)
