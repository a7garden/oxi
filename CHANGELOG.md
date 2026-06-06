# Changelog

All notable changes to the oxi project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.29.1] - 2026-06-06

### Added — oxi-agent

- **`ScreenshotMeta` struct**: Screenshot metadata (bytes, width, duration_ms) attached to `ToolCallContext::PageVisit`.
- **`PageVisit.navigation_error`**: Navigation error message from `BrowseProgress::NavigationFailed`.
- **`PageVisit.screenshot`**: Screenshot metadata from `BrowseProgress::ScreenshotCaptured`.
- **Enrichment match arms**: `make_browse_enrichment_cb` now handles `NavigationFailed` and `ScreenshotCaptured` events (previously only `DocumentReady` was processed).
- **Unit tests**: `browse_enrichment_callback_fills_navigation_error`, `browse_enrichment_callback_fills_screenshot`, `browse_enrichment_callback_navigation_failed_ignores_non_page_visit`.

### Fixed — oxi-cli

- **Clippy `large_enum_variant`**: `SessionEvent::Agent` variant boxed to reduce enum size from 264 bytes.

## [0.29.0] - 2026-06-06

### Added — oxi-agent

- **`ToolCallContext` enum**: Semantic context for tool calls (`WebSearch`, `PageVisit`, `DataExtraction`, `SessionAction`, `ScriptStep`). The agent loop infers context from tool name + args via `infer_context()`; tools remain unaware of semantics.
- **`BrowseProgress` enum**: Structured progress events from browser tab lifecycle (`NavigationStarted`, `WaitingForSelector`, `DocumentReady`, `ScreenshotCaptured`, `NavigationFailed`). Converted from `oxibrowser_core::BrowserEvent` in the backend drain task.
- **`VisitReason` enum**: `DirectNavigation`, `SearchResult { position }`, `LinkFollow` — distinguishes *why* a page was visited.
- **`BrowseCallbacks` mixin** (`callback_mixin.rs`): Eliminates duplicated pending-callback boilerplate across 4 browse tools. Provides `store_progress()`, `store_browse()`, `register_on_registry()`, `register_on_tab()`.
- **`TabCallbacks` composite** in `TabCallbackRegistry`: Single `HashMap<Uuid, TabCallbacks>` replaces the dual-map pattern. One `clear()` removes both string and browse callbacks atomically — no key-set divergence possible.
- **`make_browse_enrichment_cb()`**: Shared closure factory that enriches `ToolCallContext::PageVisit` and `DataExtraction` with `DocumentReady` data (title, status, bytes, duration).
- **`enrich_context_from_metadata()`**: Post-execute enrichment that fills `DataExtraction.result_count` from `AgentToolResult.metadata`.
- **Parallel tool execution parity**: `execute_prepared_tool_call_static` (parallel path) now has full context_cell, tab_id_slot, progress callback, and browse callback wiring — identical observability to the sequential path.
- **`browse_session "goto" → PageVisit`**: Semantic upgrade — `goto` action now produces `PageVisit { reason: DirectNavigation }` instead of generic `SessionAction`.
- **`browse_script → ScriptStep`**: `infer_context` parses step count from YAML or JSON args, producing `ScriptStep { current: 0, total: N, step: "starting" }`.
- **`browse_extract result_count`**: Extraction results include `result_count` in metadata; context enrichment populates `DataExtraction.result_count` after execute.
- **Integration tests**: `engine_forwards_browse_progress_to_callback`, `engine_routes_browse_progress_by_tab_id` — end-to-end browse progress verification with real browser.
- **Unit tests**: `browse_progress_serde_roundtrip`, `browse_enrichment_callback_*`, `infer_context_browse_script_*` — 18 new tests total.
- **`AgentTool::on_browse_progress`**: Default trait method for structured browse progress callbacks.
- **`BrowserTab::set_browse_progress_callback`**: Default trait method; only backends with browse callback support override.

### Changed — oxi-agent

- **`TabCallbackRegistry` restructured**: Dual `callbacks` + `browse_callbacks` maps → single `entries: HashMap<Uuid, TabCallbacks>` with composite `TabCallbacks { progress, browse }`. `clear()` is now atomic for both callback types.
- **`BrowserTab::clear_browse_progress_callback` removed**: `TabCallbacks` clearing handles both; no separate method needed.
- **4 browse tools refactored**: `pending_callback` + `pending_browse_callback` fields replaced with single `callbacks: BrowseCallbacks` field. ~80 lines of duplicated boilerplate eliminated.
- **`BrowseScriptTool` YAML parser rewritten**: `parse_steps` now handles the `{ steps: [...] }` map format correctly, with per-step variant dispatch and shorthand support (`- goto: "url"` for single-field struct variants, `screenshot: {}` for unit variants). Fixes 10 previously-failing tests.
- **`browse_progress_from_event`**: `NavigationFailed` match arm gated behind `oxibrowser-core ≥ 0.14` (crates.io 0.13 compatibility).

### Removed — oxi-agent (Breaking Changes)

- **`ToolProgress` enum**: Unused structured progress type (replaced by `BrowseProgress`).
- **`FileOp` enum**: Unused file operation types (part of `ToolProgress`).
- **`StructuredProgressCallback` type**: Unused callback type (replaced by `BrowseProgressCallback`).
- **`AgentTool::on_structured_progress`**: Unused trait method (replaced by `on_browse_progress`).

### Changed — oxi-sdk

- Re-exports `BrowseProgress`, `BrowseProgressCallback`, `ToolCallContext`, `VisitReason`.

### Changed — oxi-cli

- `ToolExecutionStart` and `ToolExecutionUpdate` pattern matches updated with `..` for backward compatibility.

### Changed — workspace

- Bumped all crate versions to 0.29.0.
- Inter-crate dependency versions aligned to 0.29.0.

- Per-`tab_id` `TabCallbackRegistry` replaces the single-slot `ProgressForwarder`.
  Concurrent `BrowseTool` calls (each with their own tab) are now routed correctly.
  Each `BrowseTool::execute` registers its callback on the specific tab; the
  engine's background event-drain task routes events by `tab_id`.
- `AgentTool::set_tab_id_slot` and `AgentTool::current_tab_id` default methods
  on the tool trait, enabling the agent loop to read the active tab ID.
- `BrowserTab::tab_id`, `BrowserTab::as_any`, `BrowserTab::clear_progress_callback`
  default methods on the browser tab trait.
- `BrowseTool::pending_callback` pattern: `on_progress` stores the callback;
  `execute` registers it on the actual tab (tab_id not known until tab creation).
- Integration test `engine_routes_events_by_tab_id_concurrent`: opens two tabs,
  registers per-tab callbacks, and verifies event isolation.

### Changed — oxi-agent

- `oxibrowser-core` dependency bumped from 0.12 to **0.13**.
- `BrowseTool::execution_mode` remains `SequentialOnly` (per-tab routing makes
  parallel safe, but no concrete multi-tab use case yet).

### Fixed — oxi-agent

- `AgentEvent::ToolExecutionUpdate.tab_id` is now populated (no longer always `None`).
  The agent loop passes a shared `tab_id_slot` to the tool; `BrowseTool` writes
  the tab ID when it opens a tab, and the progress callback reads it.
- `TabGuard::close` now calls `clear_progress_callback()` to unregister the
  per-tab callback, preventing stale callbacks from accumulating in the registry.

### Fixed — workspace

- Resolved CI gate violations (12 errors total under `cargo clippy --workspace -- -D warnings` and `RUSTFLAGS="-D warnings" cargo build --workspace`):
  - **oxi-sdk** (3): removed unused `std::sync::Arc` import in `ports/fs/access.rs`; replaced `let _ = tokio::spawn(...)` with `drop(tokio::spawn(...))` in `ports/mod.rs`; collapsed nested `if` in `ports/fs/capability.rs` wildcard prefix resolution.
  - **oxi-cli** (9): removed unused `clap::Parser` / `std::sync::Arc` imports in `bootstrap.rs` and `setup_wizard.rs`; removed unused `oxi::extensions::ExtensionRegistry` / `std::path::PathBuf` imports in `main.rs`; silenced `unexpected_cfgs` on the `keyring` placeholder cfg in `store/auth_storage.rs::persist`; deleted dead `run_single_prompt` helper from `bootstrap.rs` (replaced by `crate::main_dispatch::run_single_prompt`); dropped needless `&` on `args` borrow in `register_builtin_tools` call; suppressed unused `Result` from `App::switch_model` call in `lib.rs`; added missing `///` doc comment on `init_logging`; split doc-comment/regular-comment collision before `build_system_prompt` in `lib.rs`.
  - **oxi-agent** (1): `cargo fmt` trailing blank line in `tools/browse/engine.rs` (auto-fixed by `cargo fmt --all`).

### Changed — workspace

- Bumped all crate versions to 0.27.1 (oxi-ai, oxi-cli, oxi-sdk, oxi-tui). oxi-agent was already at 0.27.1. Inter-crate dependency versions aligned to 0.27.1.

### Fixed — oxi-agent

- `BrowseTool::execution_mode` now returns `SequentialOnly` to prevent the OxiBrowserEngine progress forwarder race. (Future work: per-tool_call_id forwarder.)

### Changed — infrastructure

- **CI**: Added `smoke-test` job to `.github/workflows/ci.yml` so PRs run a lightweight test subset
- **CI**: Replaced `cargo install` with `taiki-e/install-action` for `cargo-audit` and `cargo-deny` (saves ~3 min/job)
- **CI**: Added macOS to `test.yml` matrix for cross-platform test coverage
- **CI**: Added `RUSTDOCFLAGS=-D warnings` to `test.yml` so doc-tests fail on warnings
- **Release**: Switched x86_64 macOS runner from `macos-13` (deprecated) to `macos-14` (cross-compiled)
- **Release**: Added tag-on-main verification step to prevent releases from stale branches
- **PR Gate**: Conventional commit title is now enforced (error, not warning); PR size hard cap at 4000 lines
- **PR Gate**: Added merge-commit detection and issue-linkage encouragement
- **Dependabot**: Added `github-actions` ecosystem alongside cargo
- **Cargo**: Removed conflicting `[profile.release]` from `.cargo/config.toml` (workspace `Cargo.toml` is now the single source of truth)
- **Cargo audit/deny**: Synced ignore lists across `.cargo/audit.toml` and `deny.toml`; added upgrade tracker comment for extism ≥ 1.22 (wasmtime ≥ 43)
- **Docs**: Added `CODEOWNERS` for per-area review assignment

[0.29.0]: https://github.com/a7garden/oxi/compare/v0.28.0...v0.29.0
[Unreleased]: https://github.com/a7garden/oxi/compare/v0.29.0...HEAD

## [0.24.0] - 2026-05-30

### Changed — workspace

- Bumped all crate versions to 0.24.0
- Fixed 18 doc warnings across all crates (unresolved links, bare URLs, HTML tags)
- Added `.cargo/audit.toml` with documented vulnerability ignore rationale (wasmtime 41.x via extism)
- Updated README version badge to 0.24.0
- Updated AGENTS.md version to 0.24.0

## [0.25.7] - 2026-05-31

### Changed — oxi-cli

- **Provider select overlay improvements**: Updated handler logic, factory enhancements, and slash command integration
- Bumped all crate versions to 0.25.7

## [0.25.4] - 2026-05-31

### Added — oxi-sdk

- `oxi-sdk/examples/builder_demo.rs` — end-to-end SDK usage example

### Changed — workspace

- Added proper attribution to original [pi](https://github.com/earendil-works/pi) project (MIT License, Copyright © 2025 Mario Zechner)
- Updated LICENSE.md with dual copyright notice (pi + oxi contributors)
- Added NOTICE.md with detailed attribution of derived architecture
- Updated README.md, AGENTS.md, CONTRIBUTING.md to reflect port provenance
- Root repository cleaned up: removed 75+ analysis/report markdown files and orphaned source files
- All Korean comments and doc strings translated to English across 15 source files
- `.gitignore` expanded with editor, OS, and profiling exclusions
- `rust-toolchain.toml` added to pin toolchain version
- `deny.toml` added for `cargo deny` dependency auditing
- `.editorconfig` added for cross-editor consistency
- `.cargo/config.toml` added for build configuration
- CI pipeline enhanced with `cargo doc`, `cargo test --doc`, and `cargo deny` jobs
- `docs.rs` metadata added to all library crate Cargo.toml files
- Bumped all crate versions to 0.25.4

### Fixed — oxi-agent

- `truncate.rs` test updated to use emoji-based multi-byte characters

### Fixed — oxi-tui

- `fuzzy.rs` Unicode match test updated for ASCII pattern
- `chat.rs` CJK wrapping tests updated with English text
- `input.rs` CJK input tests updated with ASCII equivalents
- `text.rs` CJK truncation tests updated with ASCII equivalents

## [0.24.0] - 2026-05-19

### Added — oxi-sdk

- Re-export `SearchCache`, `CompactionEvent`, `UserMessage` and all built-in tools (`EditTool`, `ReadTool`, `WriteTool`, `GrepTool`, `FindTool`, `LsTool`, `WebSearchTool`, `GetSearchResultsTool`) for single-dependency access via `oxi-sdk`

## [0.15.1] - 2026-05-16

### Fixed — oxi-agent

- **tool_exec.rs**: Add `+ Send` bound to `FinalizedToolCallEntry::Future` and `pending_futures` type alias, making `AgentLoop::run()` / `run_messages()` / `continue_loop()` futures `Send`-compatible for `tokio::spawn`

### Changed — oxi-sdk, oxi-cli

- Bump `oxi-agent` dependency to 0.15.1

## [0.15.0] - 2026-05-16

(No changelog entry recorded)

## [0.14.0] - 2026-05-16

### Added — oxi-sdk (oxios Agent OS Engine)

- **KernelToolProvider trait** (`oxi-sdk/src/kernel_bridge.rs`): Bridge interface for oxios kernel tools (exec, memory, browser, persona) to be plugged into the SDK agent builder
- **AgentGroup** (`oxi-sdk/src/agent_group.rs`): Multi-agent orchestration with Pipeline/Parallel/Orchestrated strategies
- **MessageBus** (`oxi-sdk/src/message_bus.rs`): Broadcast-based inter-agent communication for oxios environments
- **AgentMetrics** (`oxi-sdk/src/metrics.rs`): Atomic counters for tracking runs, tokens, durations with snapshot export

### Added — oxi-agent

- **Agent::export_state() / import_state()**: Session persistence via JSON serialization of AgentState
- **Agent::continue_with()**: Session continuation within same agent instance
- **Agent::run_tokio_stream()**: Tokio-native event streaming with tokio::sync::mpsc channels (WebSocket/SSE gateway friendly)
- **StructuredOutput** (`oxi-agent/src/structured_output.rs`): JSON extraction and schema validation from agent responses
- **AgentState Serialize/Deserialize**: Full state serialization including messages, tokens, iteration progress
- **AgentConfig::output_mode**: Optional structured output mode configuration

### Added — oxi-ai

- **ProviderPool** (`oxi-ai/src/provider_pool.rs`): Rate limiting and concurrency control with semaphore + sliding window RPM for multi-agent shared API key scenarios

### Added — oxi-sdk / oxi-agent

- **AgentBuilder::kernel_tools()**: Register kernel tools via KernelToolProvider during agent construction

### Fixed — oxi-agent

- **edit_diff.rs**: Detect and reject ambiguous matches (old_text appearing >1 time) with clear error message
- **edit.rs**: Add serde aliases for `old_text`/`new_text` to fix multi-edit JSON parsing
- **grep.rs**: Detect and skip broken symlinks before `read_dir` to prevent crashes

### Fixed — tests

- **edge_cases.rs**: Fix `test_read_large_file` offset (101 for 1-indexed), `test_grep_with_broken_symlink` error handling
- **tools.rs**: Fix `test_bash_working_dir` (handle workspace restriction errors), `test_find_path_not_found` (accept 'Cannot read' error)
- **provider_mock.rs**: Fix `test_empty_stream` expectation (1 Start event, not 0)

### Changed — oxi-agent

- **SharedState now Clone + Arc-based**: `SharedState` wraps `Arc<RwLock<AgentState>>` enabling state sharing across async boundaries
- **AgentInner now Clone**: Inner config/provider cloneable for tokio streaming paths

## [0.13.0] - 2026-05-15

### Added — oxi-cli / oxi-agent

- **Thinking level display in footer**: Model shown with thinking level indicator (e.g., `(minimax) MiniMax-M2.7 • high`)
- **Shift+Tab to cycle thinking level**: Press Shift+Tab to cycle through thinking levels: off → minimal → low → medium → high → xhigh → off
- **Thinking level in TUI footer**: Footer now shows thinking level as secondary info (muted color) next to model name

### Changed — oxi-store

- **ThinkingLevel enum aligned with pi-agent**: Changed from `none, minimal, standard, thorough` to `off, minimal, low, medium, high, xhigh` to match pi-agent naming conventions
- **Default thinking level is now `medium`**: Consistent with pi-agent behavior

### Changed — oxi-cli / oxi-ai

- **Thinking level system prompts updated**: All thinking levels (off, minimal, low, medium, high, xhigh) now have appropriate system prompts with distinct characteristics

### Fixed — oxi-store

- **Fixed failing tests**: Updated environment variable tests to reflect that `apply_env()` and `from_env()` are now no-op (env overrides disabled)
- **Fixed PoisonError in parallel tests**: Removed unnecessary ENV_LOCK usage from tests that don't modify env vars

## [0.8.0] - 2026-05-06

### Added — oxi-agent

- **2-level agentic loop** matching pi-mono architecture: outer loop (follow-up messages), inner loop (tool calls + steering)
- **turn_start / turn_end events** emitted each iteration for lifecycle tracking
- **Steering messages**: inject user messages mid-run via `session.steer()`, polled after each turn
- **Follow-up messages**: queue messages during agent execution, processed when agent would stop via `session.follow_up()`
- **beforeToolCall / afterToolCall hooks** for tool execution pipeline customization
- **shouldStopAfterTurn hook** for graceful early termination
- **ToolExecutionMode** (Sequential / Parallel) config on AgentHooks
- **Terminate flag propagation**: batch terminates only when every tool result sets `terminate: true`
- **Streaming message lifecycle events**: `MessageStart` → `MessageUpdate` (per delta) → `MessageEnd`
- **ThinkingDelta forwarding** to TUI for real-time reasoning display
- **AgentHooks** struct with all hook types (get_steering_messages, get_follow_up_messages, etc.)
- **ToolBatchResult** for batch tool execution results
- **Compaction per iteration**: context window check at each iteration, not just once

### Added — oxi-cli

- **Tool snippets in system prompt**: Available tools now show descriptions instead of "(none)"
- **AgentSession queue → Agent hooks connection**: steering/follow-up queues wired to agent loop
- **Input unlock during agent busy**: typing, paste, and Enter allowed while agent is streaming
- **Enter while busy → queue as steering message** instead of being ignored

### Fixed

- **TurnEnd event**: real assistant message instead of placeholder UserMessage
- **Fallback model logic restored** on stream error
- **turn_number**: incremented before use (was starting at 0)
- **web_search.rs** compilation error simplified
- **Removed dead code**: old `execute_tool()` method, unused imports, Korean comments → English
- **ToolExecutionMode default**: Sequential (parallel was fallback to sequential anyway)

### Changed

- System prompt tool descriptions now populated from `tool_snippets` HashMap
- Agent loop restructured from single loop to pi-mono 2-level loop architecture

## [0.5.0] - 2026-05-05

### Fixed — oxi-ai

- **TextDelta double-push bug** in `high_level.rs` `complete()` function. Text was being pushed to `text_buffer` twice at block boundaries, causing double-counting. Fixed by reordering logic to execute `text_buffer.push_str(&delta)` exactly once.
- **ToolCallStart synthetic ID generation** now uses the actual `tool_call_id` from provider events instead of always generating synthetic IDs.

- **SSE parsing edge cases** comprehensively tested for both OpenAI and Anthropic providers. Added 39 unit tests covering single/multiple events, finish reasons, tool call deltas, usage accumulation, thinking blocks, carriage return line endings, and malformed input handling.
- **Serialization roundtrip tests** added to `types.rs`, `messages.rs`, and `error.rs`. All core types now have comprehensive test coverage for JSON/MessagePack roundtrips.
- Fixed pre-existing `concat!` macro syntax errors in `providers/anthropic.rs` and `providers/openai.rs`.


### Changed — oxi-ai

- `ProviderEvent::ToolCallStart` now carries `tool_call_id: Option<String>` for real tool call IDs from providers.

- `ContentBlockStart` (Anthropic) now includes `id` field.
- `ContentBlockRef` (Bedrock) now includes `id` field.

### Added — oxi-agent

- **Parallel tool execution**: `execute_tool_calls_parallel` now uses `futures::future::join_all` for concurrent execution while preserving result order.
- **Circuit breaker integration**: `CircuitBreaker` from `recovery.rs` is now wired into `AgentLoop`. Configurable threshold and open duration with automatic recovery.
- **18 integration tests** covering multi-turn tool use loop, compaction flow, cross-provider model switching, error recovery scenarios, steering messages, and follow-up queue processing.

### Added — oxi-cli

- **48 AgentSession tests** covering model cycling, thinking level changes, steering/follow-up queues, compaction trigger logic, session persistence, and event subscriptions.

## [0.1.0-alpha] - 2025-05-03

Initial alpha release of the oxi workspace.

### Added — oxi-ai

- Unified LLM API with provider-agnostic `Context` and `Message` types
- Streaming response handling via async `ProviderEvent` streams
- Multi-provider support (OpenAI, Anthropic, Google, Ollama, OpenRouter)
- Tool/function calling with typed definitions and responses
- Token estimation with hybrid algorithm (character + token heuristic)
- Conversation context management and message compaction
- Cross-provider message transformation
- JSON Schema validation for structured outputs

### Added — oxi-agent

- Agent runtime with streaming event loop
- `AgentTool` trait for defining LLM-callable tools
- `ToolRegistry` for tool management and dispatch
- Built-in tools: read, write, edit, bash, web search, questionnaire, review loop
- Context compaction for long conversations
- Tool streaming and progress updates
- Agent event types (thinking, text, tool calls, completion)

### Added — oxi-tui

- Component-based terminal UI framework
- Differential rendering (line-level dirty tracking)
- Theme system with TOML/JSON hot-reload
- Built-in components: Text, Input, Editor, Markdown, Completion
- Overlay system for modals and popovers
- Image rendering with Kitty and iTerm2 protocol support
- Chat view with streaming display
- Unified keyboard, mouse, and resize event handling

### Added — oxi (CLI)

- Interactive REPL for chatting with LLMs
- Session system with persistence and branching
- CLI argument parsing via clap
- Skill/template system for reusable prompt patterns
- Extension loading system for dynamic plugins
- Error handling and recovery
- TUI integration for interactive mode

### Added — Skills

- Brainstorming skill for collaborative ideation
- Deep-research skill for investigation and design
- Scout skill for fast codebase reconnaissance
- Super-review skill for deep system analysis
- Design-farmer skill for design system construction
- Playwright CLI skill for browser automation
- Worktree skill for git worktree management
- Obsidian skill for vault operations

### Infrastructure

- Workspace with 4 crates: oxi, oxi-ai, oxi-agent, oxi-tui
- Comprehensive test suites for all built-in tools
- Project README files for each crate
- MIT license
