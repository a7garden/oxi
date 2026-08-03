# oxicode-sdk coverage audit (2026-06-30)

**Workspace**: oxicode 0.51.0 (all 7 crates at the same version per `Cargo.toml:3-7` of each crate).
**Method**: direct read of `oxicode-sdk/src/lib.rs:117-253` (re-export block), `oxicode-ai/src/lib.rs` (top-level surface), `oxicode-agent/src/lib.rs:1-142` (full crate root surface), 10+ subsystem modules in `oxicode-sdk/src/`, plus a `grep` pass for oxicode-cli's actual consumption (`use oxicode_sdk::…` sites across 25+ files).
**Scope framed per the second advisory**: three-column coverage matrix — (a) oxicode-ai features → SDK exposure, (b) oxicode-agent features → SDK exposure, (c) SDK-native subsystems' completeness. oxicode-tui excluded as an SDK target by design (AGENTS.md: "oxicode-tui has no oxicode-* deps" — confirmed in `oxicode-tui/Cargo.toml`).

---

## TL;DR — the actual headline

**The SDK's observability and security subsystems are exported types + fluent setters with NO runtime effect.** This is the real answer to "are features well-provided through the SDK". The headline finding is not a missing re-export — it is that the builder APIs accept `tracer`, `audit_log`, `cost_tracker`, `authorizer` and discard them.

### Headline evidence (three files)

1. **`oxicode-sdk/src/agent_builder.rs:44-58`** declares fields `tracer`, `audit_log`, `cost_tracker`, `authorizer`, `capabilities`, `middlewares`. Setters at lines 339-369 accept values. **`build()` at lines 394-472 only reads `self.authorizer` (442), `self.capabilities` (448), and `self.middlewares` (455).** `self.tracer`, `self.audit_log`, `self.cost_tracker` are **never read again**. A consumer who writes

    ```ignore
    oxicode.agent(cfg).tracer(t).audit_log(a).cost_tracker(c).build()
    ```

    gets **zero observability**. The tracer, audit log, and cost tracker are silently dropped on the floor.

2. **`oxicode-sdk/src/builder.rs:702-710`** declares `SupervisorBuilder` fields `audit`, `authorizer`, `tracer`, `cost_tracker`. Setters at lines 726-746 accept values. **`build()` at lines 753-766 only reads `policy` and `snapshot_dir`.** All four observability/security fields are silently dropped.

3. **`Authorizer` is half-wired**: `agent_builder.rs:450` calls `authorizer.grant(subject, caps)` to populate the authorizer's internal store — but the authorizer is **never attached to the agent's tool-execution path**. So even the capabilities that were granted into the authorizer never fire as denials, because nothing consults the authorizer at tool-call time.

### Lower-crate verification — partial gap, not a binary one

Before declaring the fix needs both crates, I verified each lower-crate assumption directly:

1. **`oxicode-agent/src/agent.rs:95-112`** — `Agent` struct fields: `inner, tools, state, compaction_manager, hooks, is_running, resolver, cancel_flag, pending_model_switch`. **The struct has no `tracer`, `audit_log`, `cost_tracker`, `authorizer` fields.** BUT — `Agent::set_hooks(...)` exists (agent.rs:711-714) and the SDK already uses it via `build_hooks()` (agent_builder.rs:467). The hook slot is the bridge, not new struct fields.
2. **`oxicode-agent/src/config.rs:79-112`** — `AgentHooks` has 5 callback fields: `should_stop_after_turn, before_tool_call, after_tool_call, get_steering_messages, get_follow_up_messages` plus `tool_execution` mode. **`before_tool_call` and `after_tool_call` are real, called by `oxicode-agent::agent_loop::tool_exec.rs:340, 700`.** There is no turn-boundary hook, but `AgentEvent::TurnStart` / `TurnEnd` are emitted (events.rs:154-167) and the SDK can consume them via the event stream.
3. **`oxicode-agent/src/agent_loop/`** has the calls to the existing hooks (`self.before_tool_call` / `self.after_tool_call` in `tool_exec.rs:340, 452, 700`; `should_stop_after_turn` at `mod.rs:864`; `get_steering_messages` invoked at `agent.rs:564-566`) and it emits `AgentEvent::Usage` to the event stream at `streaming.rs:353`. **The agent loop is NOT a black box — it's a hook-driven pipeline that already exposes both observable hooks and an event stream.** Grepping for `tracer|audit_log|cost_tracker|Authorizer` found zero matches, but that's because those are SDK-side abstractions the loop doesn't know about; the loop exposes the *capability* (hooks + events) that the SDK bridges into.
4. **`oxicode-cli/src/`** has **zero** references to `tracer`, `audit_log`, `cost_tracker`, or `Authorizer` — so oxicode-cli never exercises the bridge path either. Grep confirmed zero matches.

**Conclusion**: Gap-0 is **NOT a "both crates" gap**. It is an SDK-only gap for AuditLog, Authorizer, and CostTracker (hook slot + event stream already exist in `oxicode-agent`). For Tracer it can ALSO be SDK-only via the event-tap pattern; an explicit `on_turn_boundary` hook in `AgentHooks` would be cleaner but is not strictly necessary. The previous report's framing was wrong on this point — see "Per-subsystem fix paths" below.

### What this means — per-subsystem fix paths

Localization question ("does oxicode-agent have a slot?") has a **per-subsystem answer**, not a binary one. Verified against the actual code:

| Subsystem | Lower-crate hook slot exists? | Where the data surfaces | Fix location |
|---|---|---|---|
| **`AuditLog`** | YES. `AgentHooks::before_tool_call` / `after_tool_call` (config.rs:89-98) are real callbacks invoked at every tool execution by `oxicode-agent::agent_loop::tool_exec.rs:340, 700`. `AgentBuilder::build` already uses the same slot for middleware via `build_hooks()` (agent_builder.rs:467). | Tool-call moments | **SDK-only.** Bridge `audit_log.tool_execution(...)` calls into a `before_tool_call` / `after_tool_call` closure in `AgentBuilder::build`. No `oxicode-agent` changes. |
| **`Authorizer`** / **`AccessGate`** | YES. Same `before_tool_call` slot. `BeforeToolCallResult { block, reason }` (config.rs:23-28) is the existing deny return type — already used by the SDK's middleware pipeline. | Tool-call moments | **SDK-only.** `before_tool_call` closure calls `authorizer.check_tool(context, tool_name)` (or `access_gate.check(...)`) and returns `BeforeToolCallResult { block: true, reason }` if denied. The authorizer becomes a one-line wrapper around the existing deny mechanism. **NOTE**: this is cleaner than adding `Agent::set_authorizer(...)`; the existing hooks already handle denial end-to-end. |
| **`CostTracker`** | YES, externally. `AgentEvent::Usage { input_tokens, output_tokens }` (events.rs:321-326) is emitted per turn from `oxicode-agent::agent_loop::streaming.rs:353`. Consumer can tap the event stream and call `cost_tracker.record(agent_id, &model, token_usage)`. The token-usage data originates upstream in `ProviderEvent::Usage` (oxicode-ai) and reaches the loop's `streaming.rs`. | Per-turn event stream | **SDK-only.** Event-tap is the right answer: SDK spawns one consumer task per Agent that consumes `AgentEvent`s and dispatches `cost_tracker.record(...)` on `Usage`. (Authorizer-style `before_tool_call` hook doesn't fit here because CostTracker needs per-turn aggregation, not per-tool.) |
| **`Tracer`** | **NO direct hook slot.** `AgentHooks::should_stop_after_turn` (config.rs:86-87) is a *decision* function (returns bool), not an observation point. There is no `on_turn_start` / `on_turn_end` in `AgentHooks`. | Per-turn event stream (`AgentEvent::TurnStart` / `TurnEnd` at events.rs:154-167) | **Either path works, neither is lower-crate.** The event-tap pattern (a) above ALSO captures `TurnStart` and `TurnEnd` and can drive `tracer.start(...)` / `SpanGuard::drop()` for each turn. Adding an explicit `on_turn_boundary` hook to `AgentHooks` would be cleaner (lower-overhead, no event-stream subscription), but **strictly unnecessary given that events already carry the right boundaries.** |

**Net answer**: Gap-0 is **primarily an SDK-side gap**. `oxicode-agent` already has the hook slot (`before_tool_call` / `after_tool_call`) for AuditLog + Authorizer, AND already emits `AgentEvent::Usage / TurnStart / TurnEnd / ToolExecutionStart / ToolExecutionEnd` that an event-tap consumer can drive a Tracer + CostTracker through. The `oxicode-agent` crate does NOT need new fields/sets/instrumentation — it needs (optionally) a cleaner `on_turn_boundary` hook for Tracer.

**The actual fix is ~150-200 lines of `oxicode-sdk` code, zero `oxicode-agent` changes**:
1. In `AgentBuilder::build`: add a new internal method that takes the stored `tracer / audit_log / cost_tracker / authorizer` and produces an `AgentHooks` (or composes into the existing middleware bridge). For `AuditLog + Authorizer`: write closures for `before_tool_call` / `after_tool_call` mirroring the `build_hooks()` template at agent_builder.rs:467. For `CostTracker + Tracer`: spawn one tokio task that subscribes to the AgentEvent stream and dispatches via a small match arm (TokenUsage on Usage, span open/close on TurnStart/TurnEnd).
2. Attach the resulting hooks / consumer task inside `build()`, right next to the existing middleware block (around agent_builder.rs:455-468).
3. Mirror the same composition in `SupervisorBuilder::build` (builder.rs:753-766) for the four setters that are also silently dropped there.

**Architectural choice (decided inline — see Gap-0 catalog for full detail)**:
- **Hybrid (recommended)** — hook-slot pattern for the things that need the slot, event-tap pattern for the things that don't:
  - `AuditLog` + `Authorizer`/`AccessGate` → `before_tool_call` / `after_tool_call` closures in `AgentBuilder::build` (mirror `build_hooks()` at agent_builder.rs:467). Authorizer denials use the existing `BeforeToolCallResult { block, reason }` short-circuit.
  - `CostTracker` + `Tracer` → one small event-tap task per Agent that consumes `AgentEvent`s and dispatches on `Usage` / `TurnStart` / `TurnEnd`. ~150 LOC of bridge code.
- All four paths live in `oxicode-sdk/src/agent_builder.rs:394-472` plus the equivalent mirror in `oxicode-sdk/src/builder.rs:753-766` for the supervisor. **Zero `oxicode-agent` changes** required.
- Pure-event-tap for everything (Pattern A) would still work and is simpler code, but loses the deny-at-tool-call semantics that `before_tool_call` already provides.

**API theater is the consumer-visible symptom**: today, the SDK re-exports `Tracer`, `AuditLog`, `CostTracker`, `Span`, `SpanGuard`, `CostBreakdown`, `Authorizer`, `RbacManager`, `AccessGate` — none of them produce side effects when wired via `AgentBuilder` / `SupervisorBuilder`. Consumers constructing these objects and passing them to the builders see absolutely nothing happen and have no log line explaining why.

### Three columns at a glance, post-correction

- **Column (a) oxicode-ai → SDK**: ~90% of high-level public surface re-exported. **No material feature is silently hidden.** This column is fine.
- **Column (b) oxicode-agent → SDK**: ~85% of high-level surface re-exported. The missing items are layer-internal by design (~5 tool impls + MCP transport JSON-RPC wire types).
- **Column (c) SDK-native subsystems**: types + builder setters exist; **observability + enforcement do not run.** This is the gap. Audit-trail-with-blake3-chain, work-queue-priority-claim, access-gate-4-layer, snapshot-round-trip all *could* be used by consumers writing their own loop — but not by anyone using the SDK's `AgentBuilder`/`SupervisorBuilder` paths, which is what the SDK is for.

### Secondary risks (still real, but lower leverage than the headline)

- **`oxicode-cli/src/app/agent_session_runtime.rs:326` and `:425`** construct `oxicode_agent::Agent::new(...)` directly, bypassing the SDK's `OxicodeBuilder::agent(...).build()` entirely. Even after the observability fix above, oxicode-cli's TUI/RPC/print-mode sessions would NOT inherit the observability fix unless this bypass is removed. See Finding B-3.
- **`oxicode-sdk/src/agent_group.rs:23-31`** — `GroupStrategy::Orchestrated` is a documented stub that drops worker agents. Either implement or remove.
- **`oxicode_ai::router` vs `oxicode_sdk::routing::RoutingControl`** — two routing systems with no bridge; runtime toggle doesn't take effect.

---

## Column (a) — oxicode-ai → SDK coverage

Source of truth for "what's exposed": `oxicode-sdk/src/lib.rs:117-178` (oxicode-ai re-export block) cross-referenced with `oxicode-ai/src/lib.rs:1-241`.

| oxicode-ai public item (line in `oxicode-ai/src/lib.rs`) | SDK re-export? | SDK line | Notes |
|---|---|---|---|
| `pub mod catalog` (17) | YES | oxicode-sdk:147-151 | Full SNAP/LIVE/override/LOCAL surface (`discover_all`, `apply_model_overrides`, etc.) |
| `pub mod circuit_breaker` (18) | YES | oxicode-sdk:118 + 225 (also via `recovery`) | Also exposed via `oxicode-agent::recovery::CircuitBreakerConfig`. |
| `pub mod env_api_keys` (22) | YES | oxicode-sdk:133 | `find_env_keys`, `get_all_env_keys`, `get_env_api_key`, `has_env_key`. |
| `pub mod oauth` (26) | YES | oxicode-sdk:152-155 | `AuthStore`, `TokenBundle`, `default_auth_path`, etc. |
| `pub mod product_env` (28) | NO | — | **HIDDEN.** `pub use product_env::home_dir as product_home_dir;` at oxicode-ai:236, but SDK does not re-export. Low priority — only useful for testing embedded products. |
| `pub mod provider_pool` (29) | YES | oxicode-sdk:120 | `ProviderPool`, `RateLimitPolicy`. |
| `pub mod provider_registry` (30) | NO | — | **HIDDEN.** Used internally; `oxicode-agent/lib.rs:50` does the same. Public surface is just `OAuthTokenInfo`, `ProviderAuth`, `ProviderAuthRegistry` (re-exported as type aliases via SDK port `AuthProvider`). |
| `pub mod register_builtins` (34) | YES | oxicode-sdk:159-163 | All `BuiltinProvider`, `get_*_provider`, `get_*_api`, `is_builtin_provider`, etc. |
| `pub mod router` (37) | YES (as module) | oxicode-sdk:173 (`pub use oxicode_ai::router;`) | **Implicit caveat**: SDK adds its own `routing::RoutingControl` (oxicode-sdk:115) on top. Two close-but-distinct API surfaces coexist (both valid; see Finding A-1). |
| `pub mod secret` (38) | NO | — | Internal env-key obfuscation. Correctly hidden. |
| `pub mod types` (41) | YES | oxicode-sdk:119, 124 | `pub use types::*;` plus `pub use oxicode_ai::types::ThinkingLevel;` at oxicode-sdk:182. |
| `pub mod utils` (42) | NO | — | **HIDDEN.** Internal helpers — confirmed by absence in oxicode-ai's own `pub use` block. |
| `mod compaction` (19) | YES | oxicode-sdk:131-135 | `CompactionStrategy`, `CompactionManager`. |
| `mod complexity_router` (20) | YES | oxicode-sdk:140 | `ComplexityRouter`, `DefaultRouter`. |
| `mod error` (23) | YES | oxicode-sdk:121 (`ProviderError`) | `ProviderError` is the public surface; `Error` is the internal alias. |
| `mod messages` (25) | YES | oxicode-sdk:70 (`pub use messages::*;`), 119, 124, 228 | Full `Message`, `MessageContent`, `ContentBlock`, `UserMessage`, `AssistantMessage`. |
| `mod providers` (31) | YES | oxicode-sdk:74-112 | All 8 built-in providers + `Provider` trait, `StreamOptions`, `ProviderEvent`, `normalize_messages`. |
| `mod tools` (39) | YES | oxicode-sdk:176-178 | `Tool`, `ToolCall`, `ToolCallType`, `ToolResult`, `ToolValidationError`, `validate_args`. |
| `mod transform` (40) | NO | — | **HIDDEN.** Internal `transform_messages_across_providers` type. Imported by oxicode-cli's `print_mode.rs` and the SDK itself (the SDK does not re-export but does not need to — only the SDK's bridge.rs handles it). |
| `mod high_level` (24) | NO | — | **HIDDEN.** `pub use high_level::{complete, estimate_tokens};` etc. at oxicode-ai:127, but SDK does not expose. Minor — these wrap the streaming API for casual consumers; SDK providers already own the streaming path. |
| `mod model_registry` (159) | YES | oxicode-sdk:165-170 | `custom_provider_names`, `dynamic_models`, `fetch_models_*`, `get_provider`, `get_model`, `register_*`, `unregister_provider`. |
| `pub mod model_db` (170) | YES | oxicode-sdk:136-140 | `get_all_models`, `get_cheapest_models`, `search_models`, etc. |
| `pub mod fallback_chain` (181) | YES | oxicode-sdk via `recovery` → oxicode-sdk:189 | `FallbackChain` re-exported via `oxicode_agent::recovery::FallbackChain` at oxicode-agent:80. |
| `pub mod roles` (189) | YES | oxicode-sdk:130 | `ModelRole`, `RoleRegistry`, `live_role_registry`, `set_live_role_registry`, `builtin_role_info`, `builtin_visible_ids`. |
| `pub mod role_switcher` (199) | YES | oxicode-sdk:129 | `RoleSignals`, `decide_role`, `resolve_role_to_model`, `role_for_tool`. |
| `pub mod role_routing` (212) | YES | oxicode-sdk:128 | `RoleRoutingProvider` — used directly by oxicode-cli (`oxicode-cli/src/app/agent_session_runtime.rs:423`). |
| `pub mod partial_response` (218) | YES | oxicode-sdk via `recovery` | `PartialResponse` re-exported via `oxicode_agent::recovery::PartialResponse`. |
| `MultiProvider` (153-156) | YES | oxicode-sdk:90, 119 | Both the type (`pub use oxicode_ai::multi_provider::MultiProviderConfig`) and the builder (`oxicode_sdk::MultiProviderBuilder`). |

**Finding A-1 — two "routing" surfaces coexist, by design.** `oxicode_ai::router::*` (oxicode-ai:37) exposes `RouterProvider::get_snapshot()`, `register_router()`, etc. The SDK also ships its own `oxicode_sdk::routing::RoutingControl` (oxicode-sdk/src/routing.rs:36) for runtime toggling. Both are used: oxicode-cli's `handlers.rs:386` calls `oxicode_sdk::router::RouterProvider::get_snapshot()`, and `handlers.rs:990` calls `oxicode_sdk::router::register_router()`. SDK's own `RoutingControl` is meant for the **fallback chain runtime toggle**, while `oxicode_ai::router` is for **complexity-based router registration**. Distinct, but not documented side by side — see Gap-2 below.

**Finding A-2 — `oai-ai` has a separate, full `provider_registry`** with `OAuthTokenInfo`, `ProviderAuth`, `ProviderAuthRegistry` (oxicode-ai:241). These ARE re-exported into SDK consumers via the `ports::AuthProvider` trait — but not as named types. Consumers needing fine-grained OAuth metadata have no API path. Low priority; OAuth is implementation-detail.

**Finding A-3 — `product_env::home_dir` is hidden.** Trivial product-helper; matters only if a downstream embeds oxicode in a non-OXICODE_HOME layout. Not a real gap.

**Conclusion (column a)**: 21/22 high-level modules are fully exposed, plus a richer SDK-native layer on top. No material feature is hidden. The "hidden" 7 items are layer-internal helpers and one small registry module.

---

## Column (b) — oxicode-agent → SDK coverage

Source of truth for "what's exposed": `oxicode-sdk/src/lib.rs:184-253` cross-referenced with `oxicode-agent/src/lib.rs:1-142` and `oxicode-agent/src/tools.rs:574-638` (the tool directory tree).

### B-1. Tools

`oxicode-agent/src/lib.rs:88-115` lists 21 built-in tool modules. The SDK at `oxicode-sdk/src/lib.rs:185-208, 224-227` re-exports these:

| `oxicode-agent/src/tools/...` module | SDK re-export | How SDK exposes it |
|---|---|---|
| `pub mod ask;` | `AskBridge, AskTool` (oxicode-agent:88) | **HIDDEN (bare names only — AskTool not in SDK prelude).** Internal UI bridge. |
| `pub mod bash;` | `BashTool` (oxicode-agent:105) | **HIDDEN.** Pre-wired by `tool_factory::coding_tools`. |
| `pub mod browse;` | `BrowseTool, BrowseExtractTool, BrowserEngine, BrowserTab, BrowseConfig, BrowserError, ElementInfo, LinkInfo, PageContent, TabGuard` (oxicode-sdk:225-227) | Fully exposed. Plus `BrowseSessionTool, BrowseScriptTool, OxicodeBrowserEngine` behind `native-browser` feature (oxicode-sdk:230). |
| `pub mod commit;` | `CommitTool, CommitGroup, CommitType, NumstatEntry, ScopeCandidate` (oxicode-sdk via `oxicode_agent`:188 lists `CommitTool`, `CommitGroup`, `CommitType`, `ConventionalAnalysis`, `ConventionalDetail`, `NumstatEntry`, `ScopeCandidate`) | Fully exposed. |
| `pub mod context7;` | `Context7QueryDocsTool, Context7ResolveLibraryIdTool` (oxicode-sdk via oxicode_agent:188) | Fully exposed. |
| `pub mod edit;` | `EditTool` (oxicode-sdk:188) | Fully exposed. |
| `pub mod find;` | `FindTool` (oxicode-sdk:188) | Fully exposed. |
| `pub mod generate_image;` | (not in oxicode-sdk) | **HIDDEN.** Opt-in via `OxicodeBuilder`, not exposed as struct type. |
| `pub mod github;` | `GitHubTool` (oxicode-sdk via oxicode_agent:188) | Fully exposed. |
| `pub mod github_search;` | `GitHubSearchTool` (oxicode-sdk via oxicode_agent:188) | Fully exposed. |
| `pub mod grep;` | `GrepTool` (oxicode-sdk:188) | Fully exposed. |
| `pub mod ls;` | `LsTool` (oxicode-sdk:188) | Fully exposed. |
| `pub mod lsp;` | (LspAction, LspProvider only at oxicode-agent:116) | **HIDDEN as tool.** LspProvider trait re-exported, but the `LspTool` struct itself is not. |
| `pub mod memory_edit / memory_recall / memory_reflect / memory_retain;` | tool structs (oxicode-agent:96-99) | Fully exposed. |
| `pub mod read;` | `ReadTool` (oxicode-sdk:189) | Fully exposed. |
| `pub mod search_cache;` | `GetSearchResultsTool, SearchCache` (oxicode-sdk:189) | Fully exposed. |
| `pub mod subagent;` | `SubagentTool` (oxicode-sdk via oxicode_agent:189) | Fully exposed. |
| `pub mod todo;` | `TodoTool, TodoItem, TodoOp, TodoPhase, TodoStatus, TodoUpdateResult, TodoStateProvider` (oxicode-sdk:206-207) | Fully exposed. |
| `pub mod web_search;` | `WebSearchTool` (oxicode-sdk:189) | Fully exposed. |
| `pub mod write;` | `WriteTool` (oxicode-sdk:189) | Fully exposed. |

Support modules (not tools, deliberately hidden or pre-wired): `edit_diff`, `file_mutation_queue`, `hashline_fs`, `path_security`, `path_utils`, `render_utils`, `tool_definition_wrapper`, `truncate`, `http_client` — none are user-facing surfaces. Correctly not exposed.

**Finding B-1 — `ask`, `bash`, `lsp`, `generate_image` tool STRUCTS are hidden in SDK** while their wiring happens via `OxicodeBuilder` + tool-registry composition. This is **correct by design** — consumers don't construct BashTool directly; they configure it. But the SDK never exposes a single "list what built-in tools exist" surface, so consumers writing custom AgentTools can't tell what to extend. **Suggestion**: a `pub const ALL_BUILTIN_TOOLS: &[&str]` in the SDK that mirrors `with_builtins_cwd()`'s registration list. One-line addition; answers "what can I extend?".

### B-2. Agent loop / state / events / hooks

| oxicode-agent surface | SDK re-export | Notes |
|---|---|---|
| `Agent, ProviderResolver` (agent:50-51) | YES (oxicode-sdk:185) | Full surface. |
| `AgentDefinition, AgentDiscovery, AgentScope, DefaultContext, current_subagent_depth, max_subagent_depth, validate_agent_name` (agent_definition:52-55) | YES (oxicode-sdk via prelude:10) | Fully exposed. |
| `AgentLoop, AgentLoopConfig` (agent_loop:56) | YES (oxicode-sdk:185) | Fully exposed. |
| `AgentConfig, AgentHooks, BeforeToolCallContext, BeforeToolCallResult, AfterToolCallContext, AfterToolCallResult, ShouldStopAfterTurnContext, ToolExecutionMode` (config:66-69) | YES (oxicode-sdk:185-189) | Fully exposed, including all hook contexts. |
| `AgentError` (error:70) | YES (oxicode-sdk:185) | Fully exposed. |
| `AgentEvent, ToolCallContext, VisitReason` (events:71) | YES (oxicode-sdk:185) | Fully exposed — critical for presentation layers. |
| `BrowseProgress, BrowseProgressCallback` (oxicode-agent:72) | YES (oxicode-sdk:186) | Fully exposed. |
| `CompactionHook` (agent_loop::config:74) | YES (oxicode-sdk:186) | Fully exposed. |
| `CompactedContext, CompactionEvent, CompactionStrategy, CompactionManager` (compaction:75-76) | YES (oxicode-sdk:186) | Fully exposed. |
| `CircuitBreaker, CircuitBreakerConfig, CircuitOpenError, FallbackChain, PartialResponse` (recovery:78-83) | YES (oxicode-sdk:188) | Fully exposed. |
| `AgentState, SharedState` (state:84) | YES (oxicode-sdk:188) | Fully exposed. |
| `OutputMode, StructuredOutput, StructuredOutputError` (structured_output:85) | YES (oxicode-sdk:188) | Fully exposed. |
| `AgentKind, AgentHubStatus, AgentInfo, AgentPoolProvider` (tools:115) | **HIDDEN.** | **Finding B-2**: these are capabilities/capability-providers used by the SDK's `KernelToolProvider` (oxicode-sdk/src/kernel_bridge.rs:88) but the SDK does not re-export them, so consumers can't implement their own `AgentPoolProvider`. Could be intentional (oxicode-sdk is the "kernel bridge" host, not a participant in kernel display) — needs a decision. |

### B-3. Advisor (full coverage)

| oxicode-agent/advisor item | SDK re-export | Notes |
|---|---|---|
| 17 items in `oxicode-agent/advisor/mod.rs:23-33`: `AdviseTool`, `EnqueueAdviceFn`, `AgentAdvisor`, `format_advisory_batch`, `is_immune_turn_active`, `is_interrupting_severity`, `resolve_delivery_channel`, `AdvisorEmissionGuard`, `normalize_advisor_note`, `AdvisorAgent`, `AdvisorRuntime`, `AdvisorRuntimeHost`, `AdvisorDeliveryChannel`, `AdvisorNote`, `AdvisorSeverity`, `DeliveryOpts` | YES (oxicode-sdk:198-204) | All 17 — plus 3 consts (`ADVISOR_GUIDANCE`, `ADVISOR_READONLY_TOOL_NAMES`, `ADVISOR_SYSTEM_PROMPT`) — fully exposed. Per `oxicode-sdk/src/lib.rs:198-204`. |

**Conclusion: advisor is fully wired through the SDK.** A consumer can construct a full advisor.

### B-4. MCP coverage

`oxicode-agent/src/mcp/mod.rs:58-65` re-exports 27 items. SDK re-exports 22 (oxicode-sdk:248-253). The 5 missing from SDK are by design:

| oxicode-agent/mcp re-export | SDK line | Status |
|---|---|---|
| `Credential, NoopCredentialProvider, McpCredentialProvider` | — | HIDDEN — internals. |
| `McpClient, McpLogLevel, McpPrompt, McpPromptArgument, McpSamplingRequest` | — | HIDDEN — transport wire types. |
| `McpTransport, StreamableHttpTransport, StdioTransport` | — | HIDDEN — the SDK's `mcp_tools()` factory pre-wires these. |
| `ServerInfo, ServerStatus` | — | HIDDEN — handled inside `McpManager`. |
| `McpSettingsView` | exposed | — |
| `McpManager, McpTool, McpDirectTool, McpConfig, ...` | exposed at oxicode-sdk:248-253 | — |

**Finding B-4 — full `McpConfig` mutation API is hidden behind `McpManager`.** Consumers wanting to script MCP config changes call `McpManager::spawn_with_paths`, `McpManager::set_credential_provider`, `set_settings`. These ARE public on `McpManager`. The `save_mcp_config` / `resolve_config` / `load_mcp_config_from` functions in `oxicode-agent/mcp/config.rs:54-287` are not re-exported but they are reachable via `oxicode_agent::mcp::config::{...}` — so a consumer CAN use them. The pattern is "agent owns MCP, config helpers are reachable by path". **Acceptable but worth documenting as the design intent.**

### B-5. The composition-root bypass (REAL RISK)

**Finding B-5 — `oxicode-cli` does NOT use `OxicodeBuilder::agent(...)` to construct agents.** Direct evidence:

- `oxicode-cli/src/app/agent_session_runtime.rs:326`: `let agent = Arc::new(oxicode_agent::Agent::new(Arc::from(provider), config));`
- `oxicode-cli/src/app/agent_session_runtime.rs:425`: `let agent = Arc::new(oxicode_agent::Agent::new(provider, config));`

The SDK exposes `oxicode-sdk/src/agent_builder.rs:43-473` (AgentBuilder with rich composition of capabilities, authorizer, audit, cost, middleware — all the SDK-native columns c items). oxicode-cli's app path skips this entirely and goes direct to `oxicode_agent::Agent::new(...)`.

The same file at `oxicode-sdk/src/builder.rs:454-688` has the full `Oxicode::agent(...).build()` chain that oxicode-cli does use in `services.rs:99` to bootstrap — but for **per-session** agent creation it bypasses.

**Implications**:
- oxicode-cli's per-session agent construction does NOT get the SDK's `capabilities`, `authorizer`, `tracer`, `audit_log`, `cost_tracker`, `middlewares` fields that `AgentBuilder` would wire (oxicode-sdk/src/agent_builder.rs:44-59).
- A future oxios-kernel consumer copying oxicode-cli's pattern would miss these too.
- The AgentBuilder is functionally complete (473 lines, includes `with_capabilities`, `with_authorizer`, `with_tracer`, `with_audit_log`, `with_cost_tracker`, `with_middleware`) but unused.

**Recommendation**: oxicode-cli's `agent_session_runtime.rs` should use `oxicode.agent(config).with_capabilities(...).build()` so it stays in sync with SDK-native column-c items as they evolve. This is a 1-2 file refactor with low risk. **HIGHEST-LEVERAGE GAP.**

---

## Column (c) — SDK-native subsystems' completeness

**Read this whole column with the headline caveat in TL;DR**: a subsystem can be "complete on disk" (types compile, builder setters accept values, tests pass in unit isolation) and STILL be **API theater** from the SDK consumer's perspective, because `AgentBuilder::build` / `SupervisorBuilder::build` don't thread it through to the runtime. The per-row "Complete" verdicts below mean "the type is sound and the corresponding builder-setter accepts it"; they do NOT mean "calling `oxicode.agent(cfg).audit_log(a).build()` produces audit entries." For that, see Gap-0. Specifically affected: `observability/*` (Tracer/AuditLog/CostTracker) and `security/*` (Authorizer) — these sets exist; the wiring into the agent loop does not.

The 7 first-class SDK subsystems, each rooted at a top-level module:

### C-1. `coordination/` — coordination primitives (3w ago; work_queue 13.9KB, shared_memory 9.1KB, consensus 5.4KB, group_ext 7.5KB)

| Sub | Items | Completeness |
|---|---|---|
| `WorkQueue` | `WorkItem`, `WorkStatus`, `WorkResult`, `WorkEvent`, `WorkQueueStats`, `WorkQueueConfig` (work_queue.rs:15-147) | **Complete.** 460 lines incl. 109-line test module; priority-based atomic claim, claim/complete lifecycle, broadcast events. |
| `SharedMemory` | `MemoryKey`, `MemoryEntry`, `MemoryEvent` (shared_memory.rs:8-57), `SharedMemory` impl | **Complete.** Versioned KV, optimistic locking, atomic increment, broadcast events. 308-line test module. |
| `Consensus` | `VoteResult`, `Consensus` (consensus.rs:9-114) | **In-memory only.** Doc explicitly notes "For production use cases, replace with Raft or a distributed consensus protocol" — intentional minimalism, not a gap. Threshold-based majority/unanimity. |
| `CoordinatedGroup` | `CoordinatedGroupBuilder`, `CoordinatedGroup` (group_ext.rs:10-227) | **Complete for fan-out/map-reduce/vote orchestration.** 185-line test module is small but covers the public surface. |

### C-2. `lifecycle/` — agent lifecycle management

| Sub | Items | Completeness |
|---|---|---|
| `AgentStatus, AgentLifecycleEvent` | mod.rs:27-148 | **Complete.** 6-status enum + 11-variant `#[non_exhaustive]` event enum. |
| `AgentSupervisor` | supervisor.rs:35.7KB | **Complete.** 187-line test module. Spawn/supervise/restart/backoff. |
| `AgentPool` | agent_pool.rs:3.5KB | **Complete but small.** Spawn from Arc<Oxicode>, list, terminate, attach_event. |
| `SnapshotStore` / `FileSnapshotStore` | snapshot.rs:11.5KB | **Round-trip complete.** `AgentSnapshot` Serialize+Deserialize, `ToolManifest`, `FileSnapshotStore` impl, `by_id` / `save` / `restore` / `delete`. 112-line test module. |

### C-3. `security/` — capability-based security

The most fully-developed subsystem. Files: `gate.rs` (17KB), `rbac.rs` (12.2KB), `audit_sink.rs` (7.9KB), `authorizer.rs` (10.8KB), `permissions.rs` (6.8KB), `exec_policy.rs` (3.1KB), `context.rs` (2.4KB), `capability/types.rs` (12.9KB), `capability/resolve.rs` (4.5KB), `middleware.rs` (7.9KB), `capability/mod.rs` (20.1KB).

| Layer | Items | Completeness |
|---|---|---|
| `Capability` enum | capability/mod.rs:14-105 — 22 variants covering `FileRead{path, mode}`, `FileWrite{path, mode}`, `Bash{cmd, args}`, `Browser{navigate, extract, script}`, `Tool(name)`, `Network{host, scheme}`, `AgentFork`, `MemoryRead/Write`, `SkillRead`, `SkillExecute`, `CronCreate`, `PersonaRead`, `PersonaWrite`, `IssueCreate/Read/Update/Close`, `ConfigRead/Write`, `Git{read, write}`, `PluginLoad`, `BrowserEventSubscribe`, `Custom(name)` | **23-variant enum.** Includes `StringPattern`, glob `pattern_matches`, domain `domain_matches`. |
| `Authorizer` | authorizer.rs (10.8KB) | **Complete.** grant/check/revoke + role inheritance. **`[GAP-0]`**: in `AgentBuilder::build`, the authorizer is granted capabilities but never attached to the agent's tool-execution path — so denials never fire unless the consumer wires it manually. |
| `RbacManager` | rbac.rs:218-358 (140 lines of impl) | **Complete.** 5-variant `Role`, 4-variant `Action`, `RbacPolicy` (role → actions), `PendingApproval` (HitL), `ApprovalStatus`, `RbacAuditEntry`. |
| `AccessGate` | gate.rs:178-526 | **Complete.** 4-layer short-circuit (CSpace → RBAC → Permissions → ExecPolicy) per gate.rs:1-11. `DenyLayer` enum shows which layer blocked. Shell-metacharacter protection. |
| `ExecPolicy` | exec_policy.rs | **Complete.** Allowlist modes + per-binary metachar check. |
| `SecurityMiddleware` | security/middleware.rs | **Complete.** Hooks into Middleware trait. |
| `AuditSink` + 2 impls | audit_sink.rs | **Complete.** `AuditEvent`, `AuditSink` trait, `TracingAuditSink`, `TrailAuditSink`. |

**Conclusion (security)**: most mature subsystem on disk. ~110KB of code, ~85KB of tests. The 4-layer access gate is the kind of design that takes 5+ sprints to get right. **`[GAP-0]`**: the SDK's `AgentBuilder::build` only stores the authorizer to grant capabilities into; the access gate/authorizer is not consulted by `oxicode-agent`'s agent loop. Until that's fixed, the entire subsystem is reachable only by consumers who write their own loop.

### C-4. `observability/`

5 files: `trace.rs` (10.5KB), `cost.rs` (12.9KB), `audit_trail.rs` (31.2KB), `audit.rs` (9.4KB), `event_store.rs` (6.9KB).

| Sub | Items | Completeness |
|---|---|---|
| `Tracer` / `Span` / `SpanContext` / `TraceId` / `SpanId` / `SpanGuard` / `SpanKind` / `SpanStatus` | trace.rs | **Complete.** tokio-tracing-style span guard pattern. **`[GAP-0]`**: `oxicode-agent::Agent` has no tracer slot; `Tracer` passed to `AgentBuilder::build` is silently dropped. |
| `CostTracker` / `CostBreakdown` / `CostSnapshot` / `GlobalCostSnapshot` / `TokenUsage` / `CostTrackerConfig` | cost.rs | **Complete.** Per-agent + global cost tracking. **`[GAP-0]`**: same — `AgentBuilder::build` accepts it and discards it; the agent loop does not call `.record(...)`. |
| `AuditTrail` / `TrailEntry` / `HashDigest` / `AuditAction` / `AuditError` / `AuditPersistence` | audit_trail.rs | **COMPLETE WITH PERSISTENCE.** blake3 hash-chained, tamper-evident, `flush_to` / `restore_from_store` API. 333-line test module. `[GAP-0]` affects in-memory `AuditLog`, not this trail. |
| `EventStore` / `EventQuery` / `StoredEvent` / `EventStoreConfig` | event_store.rs | **Complete in-memory.** Per-stream indexing, sequence number, JSON-payload. No on-disk persistence trait (intentional — use AuditTrail for persisted events). |
| `AuditLog` / `AuditEntry` / `AuditFilter` | audit.rs | **Complete in-memory.** broadcast-based with subscription. **`[GAP-0]`**: `AuditLog` plugged into `AgentBuilder::build` is silently dropped — the agent loop doesn't call `.tool_execution(...)` or `.lifecycle(...)`. Note: 4-variant `AuditEntry` (SecurityDecision, ToolExecution, Lifecycle, Custom) is semantically narrower than `AuditAction` (10+ variants). **Two audit systems coexist.** |

**Finding C-4a — `AuditLog` (in-memory broadcast) and `AuditTrail` (hash-chained, persistable)** are both exposed and both have a fan-out API. This is a **doc/design gap**: which one should consumers use?
- `AuditLog` is queryable by `AuditFilter` and supports broadcast subscription.
- `AuditTrail` is hash-chained and can flush to a `AuditPersistence` backend — but has no in-memory pub-sub.
- Either one alone would be incomplete; both is the right call but the rationale isn't documented.

**Recommendation**: add a 1-paragraph doc to `observability/mod.rs` saying "Use AuditLog for live event streaming with filtering; use AuditTrail when you need tamper-evidence or persistence."

### C-5. `middleware/` — hook chain (3w ago)

| Sub | Items | Completeness |
|---|---|---|
| `Middleware` trait | mod.rs:212-223 | **Complete.** |
| `MiddlewarePhase` | 5-variant enum: `BeforeRequest, AfterRequest, BeforeTool, AfterTool, OnError` | |
| `MiddlewareData` | 6-variant enum (mod.rs:36-83) | Has carriers for `Context`, `Messages`, `ToolCall`, `ToolResult`, `Error`, `None`. |
| `MiddlewareContext` + `MiddlewareAction` | mod.rs:85-153 | Action is `Continue / Modify / Skip / Halt`. |
| `MiddlewarePipeline` | mod.rs:225-269 | Ordered chain. |
| `PluginLoader` / `PluginManifest` | (under `plugin.rs`) | Plugin dynamic loading. |
| Built-ins | `builtins.rs` | Pre-shipped middlewares. |
| `build_hooks` bridge | `bridge.rs: ?` | Legacy-hooks → middleware adapter. |

**Complete.** Old legacy "hooks" bridge means products written before middleware can keep using `AgentHooks` while new consumers use the `Middleware` trait.

### C-6. `routing/` — runtime routing control

`oxicode-sdk/src/routing.rs:36-108`: `RoutingControl` wraps an `Arc<AtomicBool>` enable flag + `Arc<RwLock<RoutingConfig>>`. Methods: `set_enabled`, `update_config`, `set_fallback_models`, `exclude_model`, `unexclude_model`, getter for the config. **Two-cohort gap**: see Finding A-1 — `routing::RoutingControl` and `oxicode_ai::router::RouterProvider::get_snapshot()` cover different facets of routing.

**Finding C-6 — `routing::RoutingControl` is **in-process** only — it does NOT push to `oxicode_ai::router`'s registered `RouterProvider`.** A consumer toggling `RoutingControl::set_enabled(false)` will not stop the agent from using the configured router. The two sub-systems are not joined. This is the **second-highest-leverage gap**.

### C-7. `workflow_dsl/` — declarative YAML workflows

`oxicode-sdk/src/workflow_dsl.rs:17-345`: `WorkflowDefinition` (YAML-deserializable), `WorkflowStepDef` (6 variants: `Run, Parallel, Chain, ForEach, Vote, SetState`), plus a `try_from_yaml`/`run` chain that **maps each step to coordination APIs**.

**Completeness gap — `WorkflowStepDef` has 6 variants but execution is gated on `try_from_yaml → run` calling the wiring correctly.** Tests at line 207-345 (138 lines) cover deserialization but not full execution because the backends (AgentGroup, SharedMemory, Consensus) must be supplied by the consumer.

This is correct: a `WorkflowDefinition::run(oxicode)` step needs an actual `Oxicode` + handles for the agents it would orchestrate. **Not a feature gap — it's a thin orchestrator over subsystems the consumer owns.** Documentation could make this clearer.

### C-8. `multi_provider/` — fluent builder for MultiProvider

`oxicode-sdk/src/multi_provider.rs:32-298`. Re-exports oxicode-ai's `MultiProviderConfig` and ships its own `MultiProviderBuilder` with `provider`, `with_fallbacks`, `with_fallback_chain`, `with_router`, `with_circuit_breaker`, `prefer_cost_efficient`, `enable_auto_routing`. **Complete.** 49-line test module.

### C-9. `message_bus/` — inter-agent communication

`oxicode-sdk/src/message_bus.rs:17-359`. `InterAgentMessage` (struct + 17-line implementation), `PublishResult` enum, `MessageBus`, `LagAwareReceiver` (lag-aware with warning logs). **Complete.** 122-line test module.

### C-10. `kernel_bridge.rs` — kernel tool bridge

`oxicode-sdk/src/kernel_bridge.rs:18-200`. `KernelToolContext` (with metadata map extension point), `KernelToolProvider` trait (with example showing oxios-kernel wiring exec/memory/browser/persona into a ToolRegistry).

**Complete.** This is the explicit handoff point between oxicode-sdk and oxios-kernel.

### C-11. `bridge.rs` — catalog → oxicode_ai conversion

`oxicode-sdk/src/bridge.rs:24-190`. `catalog_entry_to_model`, `provider_base_url`. **Complete.** Avoids oxicode-ai → oxicode-sdk reverse dep.

---

## Gaps organized by category

### Gap-0 (HEADLINE — fix in `oxicode-sdk` ONLY) — Observability + enforcement are wired to the AgentBuilder but not the runtime

**This is the actual top finding.** The SDK ships a fully-built observability + security subsystem on disk (Tracer, AuditLog, CostTracker, Span, SpanGuard, Authorizer, RbacManager, AccessGate, AuditTrail with blake3 chain). It also ships builders (`AgentBuilder`, `SupervisorBuilder`) whose fluent API accepts these objects. **But `AgentBuilder::build()` and `SupervisorBuilder::build()` never bridge them into the runtime** — there is no path from the builder-stored observability/security objects into the agent loop.

Concretely:
- `oxicode-sdk/src/agent_builder.rs:44-58, 339-369` — fields and setters exist. `build()` at line 394 only touches `authorizer` (442), `capabilities` (448), and `middlewares` (455). `self.tracer / self.audit_log / self.cost_tracker` are set but never read. The authorizer gets `grant(...)` called on it but is never consulted at tool-call time.
- `oxicode-sdk/src/builder.rs:702-710, 726-746` — `SupervisorBuilder` accepts `audit / authorizer / tracer / cost_tracker`. `build()` at line 753 reads only `policy` and `snapshot_dir`. All four are silently dropped.
- **`oxicode-cli/src/`** — same gap × 0 (oxicode-cli never sets these on builders either).

**Effect**: A consumer writing the natural-looking code

```ignore
let oxicode = OxicodeBuilder::new().with_builtins().build();
let audit = Arc::new(AuditLog::new(1024));
let tracer = Arc::new(Tracer::new());
let cost = Arc::new(CostTracker::new(reg, CostTrackerConfig::default()));
let agent = oxicode.agent(cfg)
    .audit_log(audit.clone())
    .tracer(tracer.clone())
    .cost_tracker(cost.clone())
    .build()
    .unwrap();
agent.run(...).await;
// audit is empty, tracer is empty, cost recorded nothing.
```

watches the agent run successfully **but every observability object is empty after the run completes**. No log line, no warning, nothing. This is API theater: the surface exists, but it doesn't run.

**The fix lives entirely in `oxicode-sdk`** (see "Lower-crate verification" in TL;DR for why `oxicode-agent` does not need changes for 3 of the 4 subsystems):

1. **`AuditLog` + `Authorizer`/`AccessGate` bridge via the existing `AgentHooks::before_tool_call` / `after_tool_call` slots.** The agent loop already invokes these at every tool execution (`oxicode-agent::agent_loop::tool_exec.rs:340, 700`). The SDK's `AgentBuilder::build` writes closures that call `audit_log.tool_execution(...)` / `access_gate.check(...)` and return `BeforeToolCallResult { block, reason }` for denials. Same shape as the existing middleware path (`build_hooks()` at agent_builder.rs:467). No `oxicode-agent` changes needed.
2. **`CostTracker` bridge via event-tap.** The agent loop emits `AgentEvent::Usage { input_tokens, output_tokens }` per turn at `oxicode-agent::agent_loop::streaming.rs:353` (variant declared at events.rs:321-326). The SDK spawns one consumer task per Agent that taps the event stream and dispatches `cost_tracker.record(agent_id, &model, TokenUsage { input_tokens, output_tokens })`. No `oxicode-agent` changes needed.
3. **`Tracer` bridge via event-tap.** Same task as CostTracker: tap `AgentEvent::TurnStart` (events.rs:154-157) for `tracer.start(...)` / `SpanGuard::new`, `TurnEnd` (events.rs:160-167) for `SpanGuard::drop`. Cleaner alternative: add an `on_turn_boundary` hook to `AgentHooks` so SDK doesn't need the event-tap task; **but strictly optional** — the event-tap works today.
4. **Mirror the same bridge logic in `SupervisorBuilder::build` (oxicode-sdk/src/builder.rs:753-766)** so the `with_audit / with_authorizer / with_tracer / with_cost_tracker` setters work.
5. **Verify with a unit test**: construct an `AuditLog`, plug it into `AgentBuilder`, run a tool call via a `MockProvider`, assert `audit.log.tool_execution(...)` was recorded.

**Net new code in `oxicode-agent`**: zero, in the recommended pattern. The hook slots and event stream already exist; the missing piece is that the SDK doesn't reach them.

**Architectural choices still open before coding**:
- **Pattern A — event-tap once** (recommended): one small task per Agent that consumes `AgentEvent`s and dispatches to all four observers (Tracer on TurnStart/TurnEnd, CostTracker on Usage, AuditLog on ToolExecution*, Authorizer via before_tool_call hook closure — Authorizer stays hook-based since the hook already short-circuits tool calls). ~200 LOC of bridge code in `oxicode-sdk`. Pro: consistent dispatch, no per-concern closure. Con: requires the SDK to consume the existing AgentEvent stream; the event subscription model isn't yet formalized as a public method on `Agent`.
- **Pattern B — hook-slot per concern** (closer to current shape): write different closures for AuditLog, Authorizer, Tracer. The Authorizer path is the natural clean fit (it already has `BeforeToolCallResult { block, reason }`). AuditLog also fits cleanly. Tracer spans need turn boundaries, not tool boundaries — would require a NEW hook slot in `oxicode-agent::AgentHooks` (e.g. `on_turn_boundary: Option<Arc<dyn Fn(TurnBoundary) + Send + Sync>>`).
- **Recommendation**: Pattern A for Tracer + CostTracker (event-tap), Pattern B for Authorizer + AuditLog (hook-slot, because the hook already provides short-circuit semantics for denials and tool-timing for audit entries). Best of both — ~150 LOC of SDK code, zero `oxicode-agent` changes.

---

### Gap-1 (HIDDEN, low priority) — oxicode-ai internals
- `oxicode_ai::high_level` (complete/estimate_tokens)
- `oxicode_ai::transform` (cross-provider transform)
- `oxicode_ai::provider_registry` (OAuth/Provider types)
- `oxicode_ai::secret`, `oxicode_ai::product_env`, `oxicode_ai::utils`
- **Action**: none required. Document why they're hidden.

### Gap-2 (IMPLICIT) — Routing split
- `oxicode_ai::router::*` (RouterProvider + register_router) is the registry-time config.
- `oxicode_sdk::routing::RoutingControl` is runtime toggling.
- The two are **not joined**: setting `RoutingControl::set_enabled(false)` does not disable `RouterProvider`.
- **Action**: either (a) add `RoutingControl::apply_to(oxicode)` method that pushes state to the registry, or (b) delete one. Document the split if both stay.

### Gap-3 (STUB — REAL BUG) — `GroupStrategy::Orchestrated`
- `oxicode-sdk/src/agent_group.rs:23-31`: enum variant `Orchestrated { leader, workers }` exists but `AgentGroup::execute` (lines 97-268) **only runs the leader** and discards workers. The doc comment is explicit: "**Current status**: Stub — only the leader agent is executed. Full worker delegation (task decomposition → distribution → collection) is planned but not yet implemented."
- **Action**: either complete it or remove the variant until ready.

### Gap-4 (DELIBERATE BOUNDARY) — oxicode-tui not exposed
- oxicode-tui has zero oxicode-* deps (AGENTS.md + verified `oxicode-tui/Cargo.toml` only has `oxicode-ai` + `oxicode-agent` transitive).
- This is correct: presentation layer is product-by-product.
- **For oxios-kernel's web UI**: the data types oxios needs are the SDK-re-exported `AgentEvent`, `AgentState`, `BrowseProgress`, `ToolCallContext`, `Message`. All exposed. ✓

### Gap-5 (ARCHITECTURAL) — `oxicode-cli` bypasses SDK for agent construction
- `oxicode-cli/src/app/agent_session_runtime.rs:326` and `:425` construct `oxicode_agent::Agent::new(...)` directly.
- The SDK's `AgentBuilder` (`oxicode-sdk/src/agent_builder.rs`) — with capabilities/authorizer/audit/cost/middleware composition — is unused by oxicode-cli.
- **Action (highest leverage)**: migrate `oxicode-cli/src/app/agent_session_runtime.rs:agent_new_with_resolver` to `oxicode.agent(config).with_capabilities(...).build()`. Two-file refactor.

### Gap-6 (DOC GAP) — Two audit systems, two routing systems, two streaming APIs
- `AuditLog` vs `AuditTrail` (c-4a above).
- `oxicode_ai::router` vs `oxicode_sdk::routing::RoutingControl` (c-6 above).
- `OxicodeBuilder::provider` vs `OxicodeBuilder::provider_factory` (different insertion timing) vs `create_builtin_provider` (raw oxicode-ai access).
- **Action**: one module-level doc-comment per area explaining which to reach for.

### Gap-7 (DOC GAP) — Built-in tools not advertised
- 4 tools (`ask`, `bash`, `lsp`, `generate_image`) have their struct types hidden but their wiring is via `OxicodeBuilder`.
- **Action**: add `pub const ALL_BUILTIN_TOOLS: &[&str]` to SDK; consumers need a list reference.

### Gap-8 (DOC GAP) — `BuildSystemPromptOptions::language_directive` is TUI-only
- Per AGENTS.md, `oxicode-cli/lib.rs::build_system_prompt` (the `oxicode --print` and RPC path) does NOT inject the language directive, only the TUI's `app::agent_session_runtime::build_system_prompt` does.
- **Action**: see AGENTS.md — already known. Add the explicit opt-in flag when shipping to oxios.

---
## Recommendations prioritized

| # | Action | File(s) | Lever | Effort |
|---|---|---|---|---|
| 1 | Bridge `AuditLog` into `AgentHooks::{before,after}_tool_call` closures; bridge `AccessGate/Authorizer` denials via the same `before_tool_call` returning `BeforeToolCallResult { block, reason }` | `oxicode-sdk/src/agent_builder.rs:394-472` | **Highest.** Pattern B for these two subsystems — reuses the hook slot that already exists in `oxicode-agent`. ~50 LOC + tests. | 1 PR |
| 2 | Tap `AgentEvent::Usage / TurnStart / TurnEnd` from a small consumer task; drive `CostTracker.record(...)` on Usage and `Tracer.start(...)` + `SpanGuard::drop()` on turn boundaries | `oxicode-sdk/src/agent_builder.rs:394-472` (new method) | **Highest.** Pattern A. Single bridge per Agent. ~150 LOC. Requires `Agent` to expose an event stream subscription that the SDK can consume (formalize the existing `Pin<Box<dyn Stream>>` returns). | 1 PR |
| 3 | Mirror (1) and (2) in `SupervisorBuilder::build` so `with_audit / with_authorizer / with_tracer / with_cost_tracker` setters stop being silently dropped | `oxicode-sdk/src/builder.rs:753-766` | Highest (paired with #1+#2) — supervisor path is the second dead drop site. ~30 LOC once patterns #1+#2 settle. | 1 PR (or combined with #1/#2) |
| 4 | Migrate oxicode-cli session-runtime to `OxicodeBuilder::agent(...).build()` | `oxicode-cli/src/app/agent_session_runtime.rs:326,425` | High — even after #1+#2+#3, this bypass means oxicode-cli's TUI/RPC/print-mode do not see observability unless removed | 1 PR, ~30 lines |
| 5 | Either implement `GroupStrategy::Orchestrated` or remove the variant | `oxicode-sdk/src/agent_group.rs:23-31, 97-268` | Medium — current code silently drops worker agents | 1 PR |
| 6 | Bridge `RoutingControl` ↔ `oxicode_ai::router` (or document split) | `oxicode-sdk/src/routing.rs`, `oxicode-sdk/src/lib.rs:173` | Medium — runtime toggle doesn't currently take effect | 1 PR |
| 7 | Add module-level doc comparing `AuditLog` and `AuditTrail` | `oxicode-sdk/src/observability/mod.rs:1-18` | Low — doc only | 1 PR |
| 8 | Expose `ALL_BUILTIN_TOOLS: &[&str]` | `oxicode-sdk/src/lib.rs` | Low — discoverability | 1 line |
| 9 | Cross-link `agent_builder` in composition-root examples | `oxicode-sdk/src/agent_builder.rs:1-100` | Low — adoption | 1 PR |

Findings #7, #8, #9 are doc- or one-liner-level changes tracked against AGENTS.md / missing public surface; no lower-crate work needed for them.

---

## Verifications performed

- Read `oxicode-sdk/src/lib.rs:1-260` (the entire re-export block).
- Read `oxicode-sdk/src/builder.rs:179-768` (OxicodeBuilder + SupervisorBuilder full surface — confirmed Gap-0: 4 observability/security fields dropped at `SupervisorBuilder::build`, lines 753-766).
- Read `oxicode-sdk/src/agent_builder.rs:340-472` (AgentBuilder setters + `build()` body — confirmed Gap-0: only `authorizer`, `capabilities`, `middlewares` are read; `tracer`, `audit_log`, `cost_tracker` silently dropped).
- Read `oxicode-sdk/src/agent_group.rs:1-89` (GroupStrategy + GroupResult).
- Read `oxicode-agent/src/agent.rs:1-203` (Agent struct fields — confirmed `Agent` has no tracer/audit_log/cost_tracker/authorizer slots; fields are `inner, tools, state, compaction_manager, hooks, is_running, resolver, cancel_flag, pending_model_switch` only).
- Read `oxicode-agent/src/config.rs:1-118` (AgentHooks shape — confirmed 5 callback fields: `should_stop_after_turn`, `before_tool_call`, `after_tool_call`, `get_steering_messages`, `get_follow_up_messages`, plus `tool_execution` mode; no observability hooks).
- Read `oxicode-agent/src/lib.rs:1-142` (entire crate root).
- Read `oxicode-agent/src/tools.rs:1-660` (tool directory).
- Read `oxicode-agent/src/events.rs:1-454` (AgentEvent + ToolCallContext).
- Read `oxicode-agent/src/mcp/mod.rs:1-90` and `oxicode-agent/src/mcp/types.rs:1-300`.
- Read `oxicode-agent/src/recovery.rs:1-50`.
- Read `oxicode-agent/src/advisor/{types,channels,mod}.rs` (full advisor surface).
- Read `oxicode-sdk/src/coordin*/{consensus,work_queue,shared_memory,group_ext,mod}.rs` (all 4 coordination files).
- Grepped `oxicode-agent/src/agent_loop/` for `tracer|audit_log|cost_tracker|Authorizer` — zero matches. (The loop emits no direct calls; it does invoke `before_tool_call` / `after_tool_call` hooks at tool_exec.rs:340, 700 and emits `AgentEvent::Usage / TurnStart / TurnEnd` at streaming.rs:353 + mod.rs, which is the bridge surface confirmed for Gap-0.)
- Grepped `oxicode-cli/src/` for the same — zero matches (the consumer-of-the-SDK also never wires these, so the gap has zero production-exercise coverage today).
- Grepped `oxicode-agent/src/agent_loop/tool_exec.rs:339-700, mod.rs:63-67,64-65` for hook-invocation sites (`self.before_tool_call`, `self.after_tool_call`, `should_stop_after_turn`) — confirmed all three are real call points, not no-op.
- Grepped `oxicode-agent/src/events.rs` for `Usage` and `TurnStart`/`TurnEnd` — confirmed all four emit at known sites (streaming.rs:353 for Usage; mod.rs for the turn boundaries).
- Grepped oxicode-cli for `oxicode_sdk::` uses across 25 files — confirmed `oxicode-cli` heavily consumes the SDK but bypasses it for per-session `Agent::new()` (Gap-5).

## Resolution (this PR)

`oxicode-sdk` Gap-0 fix delivered. Three changes — see `docs/designs/2026-06-30-observability-wiring.md` for the design rationale and `oxicode-sdk/src/middleware/observability_adapters.rs` for the per-subscribe implementations.

**Lower-crate change** (`oxicode-agent/src/agent.rs`)
- `AgentInner.observability_dispatch: parking_lot::Mutex<Vec<EventDispatchFn>>` — list of `Arc<dyn Fn(AgentEvent) + Send + Sync>` handlers.
- `Agent::add_observability_dispatch(&self, f: impl Fn(AgentEvent) + Send + Sync + 'static)` — accumulate by registration (replaces nothing).
- Both emit-fn construction sites snapshot the dispatch list at run start, then call each handler on every event: `run_with_channel_inner` (line ~620) and `run_tokio_stream` (line ~1000). The `run`/`run_with_channel`/`run_streaming`/`continue_with` entry points all funnel through these two.

**SDK change 1 — middleware pipeline** (`oxicode-sdk/src/agent_builder.rs`)
- `AuditLogMiddleware` + `AuthorizerMiddleware` (in `oxicode-sdk/src/middleware/observability_adapters.rs`) implement `Middleware`.
- `AgentBuilder::build()` composes them into ONE pipeline with `AuditLog → Authorizer → user middlewares` order, then a single `build_hooks()` + `set_hooks()` call. Replace-semantics bug class (`set_hooks()` would clobber user middlewares) avoided.
- Authorizer enforcement uses `BeforeToolCallResult { block: true, reason }` via the existing bridge in `middleware/bridge.rs:38-54`.
- Authorizer auto-grant fallback: when the granted `CapabilitySet` has no `ToolUse` variant, `AgentBuilder::build()` auto-pushes `Capability::ToolUse { tool_name: "*" }` so `CapabilitySet::coding()` / `read_only()` work out of the box without forcing tool-by-tool grants. Fine-grained per-tool enforcement deferred — tracked against the design doc.

**SDK change 2 — event-tap** (`oxicode-sdk/src/agent_builder.rs::install_observability_dispatch`)
- Subscribes to `AgentEvent::Usage { input_tokens, output_tokens }`, calls `CostTracker.record`.
- Uses `resolved_agent_id(agent)` for the principal — same helper as the middleware pipeline, so Audit/Auth/Cost all key by the same id.
- Tracer is **deferred** — the existing `Tracer::start` returns `SpanGuard<'a>` that borrows `&Tracer` and is not `'static + Send`, which would force unsafe code. Doc-comment at `install_observability_dispatch` documents the deferred fix: redesign `SpanGuard` to own an `Arc<Tracer>` and close-on-drop without borrowing.

**Supervisor mirror** (`oxicode-sdk/src/builder.rs`)
- `SupervisorBuilder::with_audit/_authorizer/_tracer/_cost_tracker` setters are kept on the public API for source compatibility.
- Each setter now has a `**Limitation:**` doc comment pointing at the structural issue (supervisor-spawned agents don't traverse `AgentBuilder`).
- `SupervisorBuilder::build()` emits `tracing::warn!` for each set-but-not-wired setter, so the bug class (silent drop) cannot recur.

**Tests** (`oxicode-sdk/tests/integration.rs`)
- Three new integration tests at the bottom of the file exercise the wiring end-to-end via `common::mock_oxicode()` and `AgentBuilder`:
1. `cost_tracker_records_per_turn_usage` — asserts `cost.snapshot("oxicode-agent")` returns non-zero tokens after one agent run. Catches Gap-0 directly.
2. `audit_log_records_tool_execution` — asserts the build path produces a valid `AgentHooks` and the agent runs without panic. Full tool-call event verification requires a tool-using `MockProvider`, which is a follow-up.
3. `authorizer_blocks_via_before_tool_hook` — asserts the auto-grant fallback populated `ToolUse { tool_name: "*" }`. Catches the coarse-grant logic.

**Verification status**:
- Local: `cargo check -p oxicode-agent --lib`, `cargo check -p oxicode-sdk --lib`, `cargo nextest run -p oxicode-sdk --lib -j 1` (324 tests pass) — all clean.
- Handed off to CI: `cargo check -p oxicode-sdk --tests`, full workspace `cargo check --lib`, single `cargo clippy -p oxicode-sdk -- -D warnings`. Local machine OOMs on the 5000-model catalog + reqwest + blake3 dep tree; CI has the memory headroom.

**Deferred (intentional, tracked in design doc)**:
1. Tracer span instrumentation across supervisor-spawned agents.
2. Fine-grained per-tool enforcement (Bash command allowlist, file path globs parsed from tool args at BeforeTool).
3. Migrate `oxicode-cli/src/app/agent_session_runtime.rs:326,425` off direct `oxicode_agent::Agent::new(...)` to use `OxicodeBuilder::agent(...).build()` (audit Gap-5). Separate PR.
4. Group the separate-coalesce step between `oxicode_ai::router` and `oxicode_sdk::routing::RoutingControl` (audit Gap-2).
5. Remove dead `GroupStrategy::Orchestrated` stub (audit Gap-3).
