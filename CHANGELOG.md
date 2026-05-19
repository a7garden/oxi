# Changelog

All notable changes to the oxi project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed — workspace

- Added proper attribution to original [pi](https://github.com/earendil-works/pi) project (MIT License, Copyright © 2025 Mario Zechner)
- Updated LICENSE.md with dual copyright notice (pi + oxi contributors)
- Added NOTICE.md with detailed attribution of derived architecture
- Updated README.md, AGENTS.md, CONTRIBUTING.md to reflect port provenance

### Changed — workspace

- Root repository cleaned up: removed 75+ analysis/report markdown files and orphaned source files
- All Korean comments and doc strings translated to English across 15 source files
- `.gitignore` expanded with editor, OS, and profiling exclusions
- `rust-toolchain.toml` added to pin toolchain version
- `deny.toml` added for `cargo deny` dependency auditing
- `.editorconfig` added for cross-editor consistency
- `.cargo/config.toml` added for build configuration
- CI pipeline enhanced with `cargo doc`, `cargo test --doc`, and `cargo deny` jobs
- `docs.rs` metadata added to all library crate Cargo.toml files

### Added — workspace

- `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1)
- `CONTRIBUTING.md` expanded with full development guide, architecture, and conventions
- GitHub issue templates (bug report, feature request, config)
- GitHub pull request template
- `oxi-ai/examples/basic_streaming.rs`
- `oxi-agent/examples/custom_tool.rs`
- `oxi-tui/examples/theme_demo.rs`
- `oxi-store/examples/settings_demo.rs`
- `oxi-sdk/examples/builder_demo.rs`

### Fixed — oxi-agent

- `truncate.rs` test updated to use emoji-based multi-byte characters

### Fixed — oxi-tui

- `fuzzy.rs` Unicode match test updated for ASCII pattern
- `chat.rs` CJK wrapping tests updated with English text
- `input.rs` CJK input tests updated with ASCII equivalents
- `text.rs` CJK truncation tests updated with ASCII equivalents

## [0.20.0] - 2026-05-19

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
