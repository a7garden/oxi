# Changelog

All notable changes to the oxi project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
