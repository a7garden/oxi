# Observability & Enforcement Wiring (2026-06-30)

**Status**: accepted design; implementation in progress as 1+1 PR.

## Context

The 2026-06-30 SDK coverage audit (`docs/audits/2026-06-30-sdk-coverage.md`,
Gap-0) flagged that `Tracer`, `AuditLog`, `CostTracker`, `Authorizer`, and
`AccessGate` shipped through `oxicode-sdk` are API theater — `AgentBuilder::build`
and `SupervisorBuilder::build` accept them via fluent setters and silently drop
them. Consumers writing natural code observe zero runtime effect.

The audit also verified that the gap is **primarily SDK-only** — `oxicode-agent`
already exposes both the `AgentHooks::before_tool_call` / `after_tool_call` hook
slots (called at `agent_loop/tool_exec.rs:340, 700`) and an `AgentEvent` stream
emitted by every `Agent::run*` method (per-turn `Usage` / `TurnStart` /
`TurnEnd`, per-tool `ToolExecutionStart` / `ToolExecutionEnd`). The missing
piece is that no SDK code bridges these primitives to the SDK-side observability
types.

## Decision

Hybrid wiring pattern, split by what each observer needs:

| Subsystem | Pattern | Lower-crate entry used | SDK construction |
|---|---|---|---|
| `AuditLog` | **hook-slot** — `before_tool_call` + `after_tool_call` closures | `AgentHooks::before_tool_call`, `after_tool_call` (config.rs:89-98) | `AgentBuilder::build` writes closures that call `audit_log.log(AuditEntry::tool_execution(...))` |
| `Authorizer` / `AccessGate` | **hook-slot** (with short-circuit) | Same `before_tool_call` slot + `BeforeToolCallResult { block: true, reason }` (config.rs:23-28) | `AgentBuilder::build` writes a `before_tool_call` closure that calls `authorizer.check(...)` and returns the deny result when blocked |
| `CostTracker` | **event-tap** — task subscribed to `AgentEvent`s | `AgentEvent::Usage { input_tokens, output_tokens }` (events.rs:321-326, emitted at `agent_loop/streaming.rs:353`) | SDK spawns one consumer task per `Agent` that dispatches `cost_tracker.record(agent_id, &model, TokenUsage { input_tokens, output_tokens })` on `Usage` |
| `Tracer` | **event-tap** (turn boundaries) — span open on `TurnStart`, drop on `TurnEnd` | `AgentEvent::TurnStart` (events.rs:154-157) + `TurnEnd` (events.rs:160-167) | Same consumer task as `CostTracker`: `tracer.start("turn")` returns `SpanGuard`; `SpanGuard::drop()` closes on the corresponding `TurnEnd` |

All four bridges live in `oxicode-sdk`. Only one minimal change is required in
`oxicode-agent`: an `Agent::set_observability_dispatch(impl Fn(AgentEvent) + Send +
Sync + 'static)` setter that, if set, is called on every event by every
existing `Agent::run*` method, in addition to the existing channel/callback.

**Why split hook-slot vs event-tap**:

- Authorizer needs **denial short-circuit**. The hook-slot returns
  `BeforeToolCallResult { block: true }` and the agent loop converts that into
  a tool-error before dispatch (tool_exec.rs:699-705). An event-tap would
  arrive after dispatch — too late to deny.
- AuditLog entries are tool-bound; the hook-slot gives them precise timing
  without consumer-side event parsing.
- Tracer spans are turn-bound; the event-tap on `TurnStart` / `TurnEnd` is the
  natural fit since `AgentHooks` has no turn-boundary hook.
- CostTracker aggregates across a turn and naturally subscribes to
  `AgentEvent::Usage`, which is emitted exactly once per turn.

## Phase 1 — `oxicode-agent` change

**File**: `oxicode-agent/src/agent.rs`

**Add** to `Agent`:

```rust
impl Agent {
    /// Register a side-dispatch closure called for every AgentEvent emitted
    /// by `run`, `run_with_channel`, `run_streaming`, `run_tokio_stream`.
    /// Multiple calls stack: every registered closure is invoked on every
    /// event. Closures run synchronously on the agent-loop emit thread.
    ///
    /// Used by `oxicode-sdk` to bridge observability types (Tracer,
    /// CostTracker, ...) into the agent loop without leaking SDK types
    /// into `oxicode-agent`.
    pub fn add_observability_dispatch(
        &self,
        f: impl Fn(AgentEvent) + Send + Sync + 'static,
    ) {
        let mut slot = self.observability_dispatch.lock();
        slot.push(Arc::new(f));
    }
}
```

**Add field** to `AgentInner`:

```rust
struct AgentInner {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
    observability_dispatch: parking_lot::Mutex<Vec<Arc<dyn Fn(AgentEvent) + Send + Sync>>>,
}
```

Construction in `build_inner` initializes the new field to an empty `Mutex`.

**Modify** the three emit-fun closures in `run_with_channel_inner` (line
614), `run_tokio_stream` (line 918), and the sync `run_streaming` callback
shim to dispatch to ALL registered side closures after the primary forwarding:

```rust
// Pseudocode for the emit-fn body in each Agent run entry point:
move |event: AgentEvent| {
    // 1. existing primary forwarding (channel, callback, etc.)
    primary_forward(&event);
    // 2. cancellation propagation (existing)
    ...
    // 3. NEW: dispatch to SDK-side observers
    for f in self.observability_dispatch.lock().iter() {
        f(event.clone());
    }
}
```

This is **5-10 lines of new code** in `oxicode-agent` plus the method/field — no
behavioural change for existing consumers (the dispatch list is empty by
default).

## Phase 2 — `oxicode-sdk` implementation

**File**: `oxicode-sdk/src/agent_builder.rs`

In `AgentBuilder::build()`, after constructing the `Agent` (line 439) but
before attaching middleware (line 467), assemble an observability
configuration into a new helper function `attach_observability`. This
function does both halves:

**A. Hook-slot bridge** for `Authorizer`/`AccessGate` + `AuditLog`. Composes
into `AgentHooks` (not the middleware pipeline, since `AgentHooks` is what
`Agent::set_hooks` consumes). When `audit_log` is set, write a
`before_tool_call` and `after_tool_call` closure pair that record audit
entries. When `authorizer` is set, also write a `before_tool_call` that
returns `BeforeToolCallResult { block: true, reason }` on denial.

**B. Side-dispatch bridge** for `Tracer` + `CostTracker`. After
`Agent::new_with_resolver(...)`, call `agent.add_observability_dispatch(...)`
with a closure that:
- On `AgentEvent::TurnStart { turn_number }`: span = tracer.start("turn-N",
  SpanKind::Agent); store in a `Mutex<Option<SpanGuard>>`.
- On `AgentEvent::TurnEnd { ... }`: drop the stored span.
- On `AgentEvent::Usage { input_tokens, output_tokens }`:
  cost_tracker.record(agent_id, &model, TokenUsage { input_tokens, output_tokens }).
- Other variants: ignored.

The `agent_id` is the Agent's resolved name (via `agent.get_config().name`).
The `model` is resolved from the Agent's resolver via the same
`agent.resolver()` interface used internally.

```rust
// Pseudocode for the dispatch closure:
let agent_id = agent.get_config().name.clone();
let resolver = agent.resolver().clone();
let model_id = agent.get_config().model_id.clone();
move |event: AgentEvent| {
    match event {
        AgentEvent::TurnStart { turn_number } => {
            let span = tracer.start(&format!("turn-{}", turn_number),
                                    crate::observability::SpanKind::Agent);
            *turn_span.lock() = Some(span);
        }
        AgentEvent::TurnEnd { .. } => {
            // SpanGuard::drop() closes the span.
            *turn_span.lock() = None;
        }
        AgentEvent::Usage { input_tokens, output_tokens } => {
            let model = resolver.resolve_model(&model_id);
            if let Some(m) = model {
                cost_tracker.record(&agent_id, &m, TokenUsage {
                    input_tokens, output_tokens,
                });
            }
        }
        _ => {}
    }
}
```

`AgentBuilder` stores the closure in `Arc<Mutex<Option<SpanGuard>>>` so
`SpanGuard::drop()` runs when the field is replaced — this is the same RAII
pattern `SpanGuard` itself uses internally.

## Phase 3 — mirror in `SupervisorBuilder::build`

**File**: `oxicode-sdk/src/builder.rs:753-766`

`SupervisorBuilder::build` currently accepts `audit / authorizer / tracer /
cost_tracker` setters and drops all four. Today the supervisor only manages
agent lifecycle (spawn/supervise/restart); the agents it spawns are created
WITHOUT the SDK's `AgentBuilder` (so observability can't reach them through
the supervisor path today).

**Decision**: do NOT attempt to retro-fit supervisor-managed agents with
observability in this PR. The supervisor is a process-management abstraction;
its agents are spawned through Oxicode's internal machinery (`agent_pool`), not
through `AgentBuilder`. Forcing observability through supervisor requires
either (a) reworking the supervisor to use `AgentBuilder` for spawned
agents, or (b) providing a separate "observer for the supervisor channel"
API.

Instead, this PR does:
1. **Document the limitation** with a doc-comment on the four supervisor
   setters. They remain on the API (non-breaking) but warn that the
   supervisor-managed agents don't currently receive them.
2. **Deprecation path** [INFERENCE] — this opens follow-up work in a
   separate RFC.

Same handling for the hook-slot bridge: it ONLY affects agents constructed
via `AgentBuilder`. Supervisor-managed agents stay unchanged.

## Phase 4 — test coverage

Add a `#[cfg(test)] mod tests` block in `oxicode-sdk/src/agent_builder.rs` with
one test per bridged observer, using `MockProvider` (already exists):
1. `audit_log_records_tool_execution` — Agent runs, calls a single tool,
   assert `audit_log.query(Default::default())` has one ToolExecution entry.
2. `authorizer_blocks_before_tool_call` — authorizer grants empty caps;
   agent run with a tool call; assert result is error + audit recorded.
3. `cost_tracker_records_usage` — mock provider returns a known usage
   count; assert `cost_tracker.snapshot(agent_id)` reflects it.
4. `tracer_opens_and_closes_turn_span` — agent runs one turn; assert the
   tracer's broadcast receiver saw `SpanStart` + `SpanEnd`.

## Trade-offs and alternatives considered

**Pattern A — pure event-tap for everything**. Cleaner code in the SDK, but
loses the deny-at-tool-call semantics that `before_tool_call` already
provides for Authorizer. The deny semantics are core to the value
proposition of `AccessGate` (CSpace + RBAC + ExecPolicy), so we keep the
hook-slot for Authorizer.

**Pattern B — pure hook-slot per concern**. Closes the audit's framing but
Tracer spans need a turn-boundary slot that doesn't exist in `AgentHooks`.
Adding a new hook slot to `oxicode-agent` is a wider API change than
warranted.

**OxicodeBuilder-managed agents**. A `Oxicode::run_agent()` convenience could
auto-attach observability. Out of scope — consumers who want observability
today call `OxicodeBuilder::agent(...).build()` and the new wiring in this PR
applies.

## Files

| File | Change |
|---|---|
| `oxicode-agent/src/agent.rs` | +`observability_dispatch: Mutex<Vec<Arc<...>>>` on `AgentInner`; +`Agent::add_observability_dispatch`; 3 emit-fun sites call the list. ~20-30 LOC. |
| `oxicode-sdk/src/agent_builder.rs` | `build()` composes audit + authorizer into `AgentHooks` (alongside existing middleware); calls `agent.add_observability_dispatch(...)` for tracer + cost_tracker. +tests. ~80-120 LOC. |
| `oxicode-sdk/src/builder.rs` | Doc-comment on `SupervisorBuilder` setters; no functional change. ~10 LOC. |
| `oxicode-cli/src/app/agent_session_runtime.rs:326,425` (follow-up PR) | Migrate direct `oxicode_agent::Agent::new(...)` calls to `oxicode.agent(config).build()`. Out of scope for observability wiring per se — separate finding. |

## Verification plan

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings`
4. `cargo nextest run --workspace`
5. The four new tests in `oxicode-sdk/src/agent_builder.rs` pass.
6. `oxicode-cli` still compiles and tests pass (the migration in
   `agent_session_runtime.rs` is a SEPARATE PR per audit Gap-5
   recommendation).

## Out of scope

- Supervisor-managed agent observability (Phase 3 limitation noted).
- `GroupStrategy::Orchestrated` stub (audit Gap-3).
- `RoutingControl` ↔ `oxicode_ai::router` bridge (audit Gap-2).
- `oxicode-cli` `Agent::new(...)` migration (audit Gap-5).
- Authorizer/AccessGate richer policy gate (multi-layer) is on the audit's
  recommendations table; this PR wires the existing types, it doesn't extend
  the security model.
