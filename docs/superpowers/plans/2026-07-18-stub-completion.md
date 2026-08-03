# oxicode 스텁 정합성화 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** oxicode 워크스페이스의 11개 미완성 스텁을 실제 동작으로 완성하거나 제거하여 "거짓 성공 경로 0개"를 달성한다.

**Architecture:** 공통 런타임 capability 패턴 — `AgentConfig`에 `url_resolver`, `lsp_provider`, `tool_call_loop_guard`를 추가하고, `AppServices`가 URL/LSP/memory lifecycle을 소유하며, TUI/print/RPC가 동일 `AgentSession`을 공유한다.

**Tech Stack:** Rust 2024, tokio, ratatui, async-lsp, fontdue, rusqlite, parking_lot

**Spec:** `docs/designs/2026-07-18-stub-completion.md`

## Global Constraints

- Rust 2024 edition, MSRV 1.96
- `cargo fmt` before every commit
- `cargo clippy --workspace --all-targets -- -D warnings` must pass
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` must pass
- `cargo nextest run --workspace` must pass
- `parking_lot::RwLock` over `std::sync::RwLock`
- Atomic file writes: temp + rename pattern
- No new oxicode-* circular dependencies
- OMP/grok upstream evidence must be cited in code comments where a pattern is ported

---

## PR 1: AgentConfig capability 필드 + URL resolver adapter + tool-call loop guard 연결

**Files:**
- Modify: `oxicode-agent/src/config.rs` — add `url_resolver`, `lsp_provider`, `tool_call_loop_guard` fields
- Modify: `oxicode-agent/src/agent.rs:552-579` — thread new fields into `AgentLoopConfig`
- Modify: `oxicode-agent/src/agent_loop/config.rs` — add fields to `AgentLoopConfig`
- Modify: `oxicode-agent/src/agent_loop/mod.rs:97-107` — remove `#[allow(dead_code)]`, wire guard into turn loop
- Modify: `oxicode-agent/src/agent_loop/mod.rs:255-262` — pass new capabilities to `ToolContext`
- Create: `oxicode-sdk/src/url_resolver.rs` — `SdkUrlResolver` wrapping `InternalUrlRouter`
- Modify: `oxicode-sdk/src/lib.rs` — re-export `SdkUrlResolver`

**Interfaces:**
- Produces: `AgentConfig.url_resolver: Option<Arc<dyn UrlResolver>>`
- Produces: `AgentConfig.lsp_provider: Option<Arc<dyn LspProvider>>`
- Produces: `SdkUrlResolver::new(router: Arc<dyn InternalUrlRouter>) -> Self`
- Produces: `SdkUrlResolver` implements `oxicode_agent::tools::UrlResolver`

- [ ] **Step 1: Write failing test for tool_call_loop_guard wiring**

Create `oxicode-agent/src/agent_loop/loop_guard_tests.rs`:
```rust
#[tokio::test]
async fn loop_guard_steers_on_repeated_identical_tool_call() {
    // Agent that calls the same tool 6 times (threshold=5)
    // After 5th call, steering message should appear in next provider context
    // After 6th identical call, run should terminate with ToolCallLoop error
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p oxicode-agent loop_guard_steers`
Expected: FAIL — guard never fires (dead code)

- [ ] **Step 3: Wire tool_call_loop_guard into turn loop**

In `agent_loop/mod.rs`, after each assistant turn completes:
```rust
// Feed completed turn's tool calls to the guard
let turn_tools: Vec<ToolCallRef> = /* extract from assistant message */;
if let Some(reason) = self.tool_call_loop_guard.lock().record_turn(&turn_tools) {
    // OMP TERMINAL_TOOL_RESULT_ABORT_REASON pattern:
    // inject steering message, abort inner loop
    steering_queue.push(reason);
    break; // terminate inner loop, outer run continues
}
```

- [ ] **Step 4: Add url_resolver and lsp_provider to AgentConfig**

```rust
// oxicode-agent/src/config.rs
#[serde(skip, default)]
pub url_resolver: Option<std::sync::Arc<dyn crate::tools::UrlResolver>>;
#[serde(skip, default)]
pub lsp_provider: Option<std::sync::Arc<dyn crate::tools::LspProvider>>;
```

Thread through `agent.rs` → `AgentLoopConfig` → `ToolContext`.

- [ ] **Step 5: Create SdkUrlResolver adapter**

```rust
// oxicode-sdk/src/url_resolver.rs
pub struct SdkUrlResolver {
    router: Arc<dyn crate::ports::InternalUrlRouter>,
}
impl SdkUrlResolver {
    pub fn new(router: Arc<dyn crate::ports::InternalUrlRouter>) -> Self { Self { router } }
}
impl oxicode_agent::tools::UrlResolver for SdkUrlResolver {
    fn can_resolve(&self, input: &str) -> bool { /* delegate to router */ }
    async fn resolve(&self, uri: &str) -> Result<ResolvedContent, String> { /* delegate */ }
}
```

- [ ] **Step 6: Run all tests**

Run: `cargo nextest run -p oxicode-agent -p oxicode-sdk`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add oxicode-agent/src/ oxicode-sdk/src/
git commit -m "feat: wire tool_call_loop_guard, add url_resolver/lsp_provider to AgentConfig"
```

---

## PR 2: 7개 URL handler 등록 + read/grep/find 통합

**Files:**
- Modify: `oxicode-cli/src/services.rs:134-138` — register all 7 handlers
- Modify: `oxicode-cli/src/internal_urls/issue_handler.rs` — already implemented, just register
- Modify: `oxicode-cli/src/internal_urls/pr_handler.rs` — already implemented, just register
- Create: `oxicode-cli/src/internal_urls/skill_handler.rs` — SkillLoader-backed
- Create: `oxicode-cli/src/internal_urls/rule_handler.rs` — RuleRegistry-backed
- Create: `oxicode-cli/src/internal_urls/agent_handler.rs` — AgentArtifactStore
- Create: `oxicode-cli/src/internal_urls/local_handler.rs` — session-scoped artifacts
- Modify: `oxicode-cli/src/lib.rs` — wire `SdkUrlResolver` into `AgentConfig`
- Modify: `oxicode-sdk/src/ports/mod.rs:931` — extend `ProtocolHandler` trait (write, complete, signal)
- Modify: `oxicode-agent/src/tools/read.rs:316` — update schema to list exactly 7 schemes

**Interfaces:**
- Consumes: `SdkUrlResolver` from PR 1
- Produces: 7 registered URL handlers in `build_url_router()`

- [ ] **Step 1: Extend ProtocolHandler trait**

Add `can_write()`, `write()`, `complete()` default methods. Add `is_directory` to `ResolvedUrl`. Add `signal`, `path_only`, `skip_directory_listing` to `ResolveContext`.

- [ ] **Step 2: Write failing test for issue:// resolution**

```rust
#[tokio::test]
async fn read_resolves_issue_url() {
    // Mock AgentSession with url_resolver wired
    // Call read tool with "issue://42"
    // Assert content is GitHub issue markdown
}
```

- [ ] **Step 3: Register issue/pr handlers**

In `build_url_router()`:
```rust
router.register(Arc::new(IssueProtocolHandler));
router.register(Arc::new(PrProtocolHandler));
router.register(Arc::new(MemoryProtocolHandler::new(memory_root)));
```

- [ ] **Step 4: Implement skill://, rule://, agent://, local:// handlers**

Each follows the OMP pattern (`skill-protocol.ts`, `rule-protocol.ts`, `agent-protocol.ts`, `local-protocol.ts`).

- [ ] **Step 5: Wire SdkUrlResolver into AgentConfig in App::from_oxicode**

```rust
let url_resolver = Arc::new(SdkUrlResolver::new(oxicode.ports().url_router.clone()));
config.url_resolver = Some(url_resolver);
```

- [ ] **Step 6: Run tests + commit**

---

## PR 3: oxicode-lsp crate + CLI adapter + LspTool 등록

**Files:**
- Create: `oxicode-lsp/Cargo.toml`, `oxicode-lsp/src/lib.rs` — thin protocol adapter
- Create: `oxicode-lsp/src/client.rs` — process spawn, JSON-RPC, diagnostics Notify
- Create: `oxicode-cli/src/lsp/manager.rs` — multi-server lifecycle, config, crash recovery
- Create: `oxicode-cli/src/lsp/provider.rs` — `CliLspProvider` implementing `LspProvider`
- Modify: `oxicode-cli/src/bootstrap.rs` — spawn LSP manager, inject into AgentConfig
- Modify: `oxicode-agent/src/tools.rs:895-945` — register LspTool only when provider is Some
- Modify: `Cargo.toml` — add `oxicode-lsp` workspace member

**Interfaces:**
- Consumes: `AgentConfig.lsp_provider` from PR 1
- Produces: `oxicode-lsp::LspClient`, `oxicode-cli::lsp::LspManager`, `oxicode-cli::lsp::CliLspProvider`

- [ ] **Step 1: Create oxicode-lsp crate skeleton**

`Cargo.toml` deps: `async-lsp`, `lsp-types`, `tokio`, `serde_json`, `thiserror`.

- [ ] **Step 2: Implement LspClient (based on grok pattern)**

`start()`, `initialize_with_timeout()`, `shutdown()`, `diagnostics_ready: Arc<Notify>`, `lifecycle_id`.

- [ ] **Step 3: Write failing test with mock Python LSP server**

Port grok's `tests.rs` pattern: write a small Python script that responds to initialize/didOpen/publishDiagnostics/definition.

- [ ] **Step 4: Implement LspManager in oxicode-cli**

Multi-server, config layering (user > project > plugin), `filter_project_lsp_when_untrusted`, `restart_monitor` with lifetime budget.

- [ ] **Step 5: Implement CliLspProvider**

Wraps `LspManager`, implements `LspProvider` trait: `ensure_started_background`, `ensure_ready`, `drain_diagnostics`, `read_diagnostics`, `notify_file_changed`, `execute_action`.

- [ ] **Step 6: Wire into bootstrap + AgentConfig**

- [ ] **Step 7: Register LspTool conditionally**

In `ToolRegistry::with_builtins_cwd`, only register `LspTool` if `lsp_provider.is_some()`.

- [ ] **Step 8: Run tests + commit**

---

## PR 4: RPC actor + dispatch + shared AgentSession

**Files:**
- Modify: `oxicode-cli/src/bootstrap.rs:213-249` — add `"rpc"` dispatch branch
- Create: `oxicode-cli/src/rpc_mode/actor.rs` — single-session actor
- Rewrite: `oxicode-cli/src/rpc_mode/handlers.rs` — delegate to actor instead of mock
- Modify: `oxicode-cli/src/rpc_mode/protocol.rs` — add `status` field to responses
- Modify: `oxicode-cli/src/cli.rs` — validate `--mode` values at parse time

**Interfaces:**
- Consumes: shared `AgentSession` from `App`
- Produces: `RpcActor`, `SessionCommand` enum

- [ ] **Step 1: Add "rpc" dispatch branch**

```rust
if args.mode.as_deref() == Some("rpc") {
    crate::rpc_mode::run_rpc_mode(app).await?;
    return Ok(0);
}
```

- [ ] **Step 2: Write failing RPC smoke test**

Spawn `oxicode --mode rpc`, send `{"type":"prompt","message":"hello"}`, assert streamed events + final response.

- [ ] **Step 3: Implement RpcActor**

`mpsc<SessionCommand>`, single stdout writer task, `Idle|Running|ShuttingDown` state machine.

- [ ] **Step 4: Map each command to AgentSession API**

`Prompt` → `session.prompt()`, `Steer` → queue, `Abort` → cancel, `SetModel` → `session.switch_model()`, etc.

- [ ] **Step 5: Remove mock handlers**

Delete the giant `execute_command` match with hardcoded responses.

- [ ] **Step 6: Run tests + commit**

---

## PR 5: Memory pipeline Stage 1/2 + /memory 명령

**Files:**
- Rewrite: `oxicode-cli/src/services.rs:376-412` — `MemoryPipeline` with real workers
- Modify: `oxicode-cli/src/store/memory_workers.rs:334-384` — implement actual LLM calls
- Modify: `oxicode-cli/src/bootstrap.rs:197-209` — wire `MemoryPipeline` into `AppServices`
- Rewrite: `oxicode-cli/src/tui/slash/builtin/memory.rs:90-133` — real command implementations
- Modify: `oxicode-cli/src/store/settings.rs` — add memory pipeline config fields

**Interfaces:**
- Consumes: `Arc<Oxicode>` for model resolution
- Produces: `MemoryPipeline::start()`, `MemoryPipeline::shutdown()`, `MemoryPipeline::command()`

- [ ] **Step 1: Write failing test for Stage 1 extraction**

Deterministic fake provider → session JSONL → Stage 1 prompt → structured output → DB row.

- [ ] **Step 2: Implement MemoryPipeline with JoinSet + CancellationToken**

- [ ] **Step 3: Wire Stage 1 worker to Oxicode resolver**

`Oxicode::resolve_model(create_provider)` for extraction model, execute prompt, parse `Stage1OutputSchema`.

- [ ] **Step 4: Implement Stage 2 consolidation**

Corpus assembly, consolidation model, atomic artifact write (temp+rename).

- [ ] **Step 5: Implement /memory view|stats|diagnose|clear|enqueue|rebuild**

Each routes through `MemoryPipeline::command()` channel.

- [ ] **Step 6: Run tests + commit**

---

## PR 6: Snapcompact renderer 흡수 + SnapcompactCompactor + inline imaging

**Files:**
- Rewrite: `oxicode-snapcompact/src/lib.rs` — absorb pi-natives renderer
- Create: `oxicode-snapcompact/src/fonts/` — bundled BDF/hex/TTF fonts
- Create: `oxicode-snapcompact/src/render.rs` — bitmap rasterization + PNG encoding
- Create: `oxicode-snapcompact/src/normalize.rs` — ANSI/emoji/NFKD/dim stopwords
- Create: `oxicode-snapcompact/src/archive.rs` — foveation, planArchive, historyBlocks
- Modify: `oxicode-snapcompact/Cargo.toml` — add fontdue, png deps
- Create: `oxicode-snapcompact/NOTICE.md` — Silver font CC BY 4.0 attribution
- Modify: `oxicode-ai/src/compaction.rs` — add grok trait seams, `SnapcompactCompactor`, `CompactionStrategy::Snapcompact`
- Modify: `oxicode-ai/Cargo.toml` — add `oxicode-snapcompact` dep
- Delete: `oxicode-ai/src/compaction.rs:432-456` — remove `ContextTransformer`/`NoopContextTransformer`

**Interfaces:**
- Produces: `oxicode_snapcompact::render_snapcompact_png(text, options) -> Vec<u8>`
- Produces: `oxicode_snapcompact::compact(preparation, options) -> CompactResult` (real PNGs)
- Produces: `oxicode_ai::SnapcompactCompactor` implementing `Compactor`
- Produces: `oxicode_ai::CompactionItem`/`CompactionSampler`/`ItemTokenCounter` trait seams

- [ ] **Step 1: Copy pi-natives renderer into oxicode-snapcompact**

Remove `#[napi]` attributes, change `Latin1String` → `Vec<u8>`, keep all bitmap/PNG/font logic.

- [ ] **Step 2: Copy bundled fonts + NOTICE.md**

- [ ] **Step 3: Write deterministic hash test**

Same input + shape → byte-identical PNG.

- [ ] **Step 4: Port serializeConversation from OMP**

Role prefixes, useless call merge, dim ON/OFF.

- [ ] **Step 5: Port planArchive foveation + resolveShapeForText**

- [ ] **Step 6: Port normalizeWithStats + dimStopwords + wrap**

- [ ] **Step 7: Add grok CompactionItem trait seams to oxicode-ai**

- [ ] **Step 8: Implement SnapcompactCompactor**

- [ ] **Step 9: Remove NoopContextTransformer**

- [ ] **Step 10: Run tests + commit**

---

## PR 7: Observability + routing + stream_responses 제거 + Orchestrated 제거

**Files:**
- Modify: `oxicode-sdk/src/observability/trace.rs` — `SpanGuard` owns `Arc<Tracer>`
- Modify: `oxicode-sdk/src/agent_builder.rs:320-328` — tracer actually records spans
- Modify: `oxicode-sdk/src/builder.rs:750-848` — remove no-op setters, add `agent_decorator`
- Create: `oxicode-sdk/src/lifecycle/decorator.rs` — `AgentDecorator` trait + `ObservabilityDecorator`
- Modify: `oxicode-sdk/src/routing.rs` — wire `RoutingControl` to agent resolution
- Modify: `oxicode-sdk/src/lifecycle/supervisor.rs:361-384` — routing handle uses shared config
- Modify: `oxicode-sdk/src/agent_group.rs:23-31` — remove `Orchestrated` variant
- Delete: `oxicode-cli/src/store/settings.rs:209-210` — remove `stream_responses`
- Modify: `oxicode-cli/src/main.rs` — remove stream_responses config handling
- Modify: `oxicode-cli/src/tui/overlay/settings.rs:725-733` — remove "(not wired)" display

- [ ] **Step 1: SpanGuard → Arc<Tracer> owned**

- [ ] **Step 2: Wire tracer in AgentBuilder::build**

Run/Turn/Tool span recording via event dispatch.

- [ ] **Step 3: Remove SupervisorBuilder no-op setters, add AgentDecorator**

- [ ] **Step 4: Wire RoutingControl to live agent resolution**

- [ ] **Step 5: Remove GroupStrategy::Orchestrated variant**

- [ ] **Step 6: Remove stream_responses**

- [ ] **Step 7: Run tests + commit**

---

## PR 8: WorkflowEngine + SubagentCoordinator

**Files:**
- Create: `oxicode-sdk/src/workflow_engine.rs` — 6-step executor
- Modify: `oxicode-sdk/src/lib.rs` — re-export `WorkflowEngine`
- Modify: `oxicode-sdk/src/prelude.rs` — re-export
- Create: `oxicode-sdk/src/lifecycle/subagent_coordinator.rs` — pending→active→completed lifecycle
- Modify: `oxicode-sdk/src/lifecycle/mod.rs` — re-export

**Interfaces:**
- Consumes: `WorkflowDefinition` (existing parser), prebuilt agent map
- Produces: `WorkflowEngine::execute(workflow) -> Result<WorkflowResult>`
- Produces: `SubagentCoordinator` with `CancellationToken` propagation

- [ ] **Step 1: Write failing test for WorkflowEngine Run step**

- [ ] **Step 2: Implement WorkflowEngine with 6 steps**

- [ ] **Step 3: Implement SubagentCoordinator (grok pattern)**

Pending→active→completed, CancellationToken, `run_in_background`, `resume_from`, `block_wait_slot` + timeout.

- [ ] **Step 4: Run tests + commit**

---

## Verification (all PRs merged)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings`
- [ ] `cargo nextest run --workspace`
- [ ] `cargo test --workspace --doc`
- [ ] Smoke: `oxicode --mode rpc` → prompt → response
- [ ] Smoke: `read issue://42`
- [ ] Smoke: `/memory clear`
- [ ] Smoke: LSP rename across files
- [ ] Smoke: `/compact snapcompact` → PNG generation
