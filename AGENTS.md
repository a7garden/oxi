# oxi

Rust port of [pi](https://github.com/earendil-works/pi) — terminal-based AI coding assistant. Multi-crate workspace providing multi-provider LLM access, an agent tool-calling loop, a terminal UI, a persistent session store, and an SDK for building multi-agent systems.

## Quick Facts

| Item | Value |
|------|-------|
| Language | Rust 2021 edition |
| Workspace crates | `oxi-ai`, `oxi-agent`, `oxi-store`, `oxi-tui`, `oxi-sdk`, `oxi-cli` |
| Version | 0.26.2 (latest release: 0.25.7) |
| License | MIT |
| CI | `cargo fmt`, `cargo clippy -D warnings`, `cargo nextest run`, `cargo audit`, `cargo deny check` |

## Workspace Layout

```
oxi/
├── oxi-ai/       Unified LLM API — streaming, multi-provider abstraction
├── oxi-agent/    Agent runtime — tool-calling loop, MCP client, built-in tools
├── oxi-store/    Persistent state — sessions, settings, auth, model registry
├── oxi-tui/      Terminal UI widgets — chat, themes, markdown rendering (ratatui)
├── oxi-sdk/      Multi-agent SDK — agent groups, message bus, builder pattern
└── oxi-cli/      CLI binary — ties everything together (TUI + RPC modes)
```

### Dependency Flow

```
oxi-ai  ←  oxi-agent  ←  oxi-sdk  ←  oxi-cli
oxi-ai  ←  oxi-store             ←  oxi-cli
oxi-tui  (independent)           ←  oxi-cli
```

`oxi-ai` is the foundation layer with zero internal dependencies.
`oxi-cli` is the integration layer that depends on all other crates.
Never create circular dependencies between crates.

## Architecture Overview

### oxi-ai — Unified LLM API

Provider-agnostic streaming interface. Core trait in `providers/trait_def.rs`:

```rust
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;
    fn name(&self) -> &str;
}
```

**8 built-in providers** in `src/providers/`: openai, openai-responses, anthropic, google, vertex, mistral, azure, bedrock.
`model_db.rs` contains pricing/context/feature data for 934 models across 29 providers.
`compaction.rs` summarizes old messages when context grows too large.
`ProviderRegistry` in `mod.rs` supports both custom providers (via `register()`) and built-in fallback (via `register_builtins.rs`).

Key types: `Model`, `Context`, `Message`, `ContentBlock`, `Tool`, `ProviderEvent`, `ProviderError`, `ProviderRegistry`.

### oxi-agent — Agent Runtime

Manages the LLM tool-calling loop. Core trait in `src/tools.rs`:

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
}
```

**17 tools** in `src/tools/`: bash, read, write, edit, edit_diff, ls, find, grep, github, github_search, subagent, questionnaire, context7 (2 sub-tools), web_search, get_search_results, generate_image, search_cache.
**7 essential tools** (cannot be disabled): read, write, edit, bash, grep, find, ls.
`agent_loop/` contains streaming, tool execution, retry logic, and queue management.
`mcp/` implements Model Context Protocol client.
`agent.rs` has `ProviderResolver` trait for resolving provider/model by name.

Key types: `Agent`, `AgentEvent`, `AgentState`, `AgentConfig`, `ToolRegistry`.

### oxi-store — Persistent State

Append-only JSONL session storage with tree branching (fork).
Layered settings: defaults → global (`~/.oxi/settings.toml`) → project (`.oxi/settings.toml`) → env vars → CLI args.
`auth_storage.rs` stores API keys and OAuth tokens.
`model_registry.rs`/`model_resolver.rs` handle model metadata and ID resolution.

Key types: `SessionEntry`, `AgentMessage`, `Settings`, `SessionManager`.

### oxi-tui — Terminal UI

Built on `ratatui` + `crossterm`. Theme system with hot-reload from TOML/JSON files.
Markdown rendering via `pulldown-cmark`. Fuzzy search for file/command completion.
`widgets/chat/` is the main conversation widget (3166 lines across 8 files).

Key types: `Theme`, `ThemeManager`, `ChatWidget`, `ToolRenderer`.

### oxi-sdk — Multi-Agent SDK

Builder pattern for constructing agents. `AgentGroup` supports parallel, sequential, and fan-out strategies.
`MessageBus` provides pub/sub inter-agent communication.
`KernelToolProvider` bridges SDK agents to the host tool registry.

Key types: `AgentBuilder`, `AgentGroup`, `MessageBus`, `ClosureTool`, `KernelBridge`.

### oxi-cli — CLI Binary

Entry point: `oxi-cli/src/main.rs` (uses `clap`).
Runs in two modes: **TUI mode** (interactive terminal) and **RPC mode** (JSON-over-stdin/stdout for IDE integration).
`extensions/` supports native shared libraries (`.dylib`/`.so`/`.dll`) and WASM via Extism.
`skills/` loads markdown skill files from `~/.oxi/skills/<name>/SKILL.md`.
`storage/packages.rs` is the built-in package manager.

Extension system (`src/extensions/types.rs`):
- `ExtensionManifest`: metadata with permissions (FileRead, FileWrite, Bash, Network)
- `ExtensionState`: Pending, Active, Disabled, Failed, Unloaded
- `InputEventResult`: Continue, Transform { text }, Handled

## Conventions

### Code Style

- `cargo fmt` before every commit — no exceptions.
- `cargo clippy --workspace -- -D warnings` must pass clean.
- Module structure: `mod.rs` re-exports public API, implementation in sibling files.
- Prefer `anyhow::Result` for application code, custom error enums (`thiserror`) for library crates.
  - **Library crates** (oxi-ai, oxi-agent, oxi-store, oxi-sdk): define typed error enums with `thiserror::Error` for public API functions. Internal helpers may use `anyhow`.
  - **Application crate** (oxi-cli): use `anyhow::Result` everywhere.
  - **Leaf crate** (oxi-tui): `anyhow` is acceptable — no public error types needed.
  - Never create a shared workspace error crate. Each library owns its own error type.
- Use `parking_lot::RwLock` instead of `std::sync::RwLock`.
- Atomic file writes: use `atomic_write()` helper (write to temp, then rename).
- Async: `tokio` runtime with `#[tokio::main]`. Use `async_trait` for trait objects.

### Testing

- Unit tests in `#[cfg(test)] mod tests` within each module.
- Integration tests in `<crate>/tests/*.rs` (e.g., `oxi-agent/tests/agent_loop_full.rs`).
- Mock providers (`MockProvider`) for agent/loop testing.
- **Test runner: `cargo-nextest`** — parallel execution, per-test timeouts.
- Config: `.config/nextest.toml` (profiles: `default`, `ci`, `release`).
- `cargo nextest run --workspace` must pass before merge.

### Adding a New LLM Provider

1. Create `oxi-ai/src/providers/<name>.rs`.
2. Implement the `Provider` trait.
3. Add `BuiltinProvider` entry in `oxi-ai/src/providers/register_builtins.rs`.
4. Add model data to `oxi-ai/src/model_db.rs` if needed.

### Adding a New Tool

1. Create `oxi-agent/src/tools/<name>.rs`.
2. Implement the `AgentTool` trait.
3. Add module declaration in `oxi-agent/src/tools.rs`.
4. Register in `ToolRegistry::with_builtins_cwd()`.
5. Mark `essential()` as `true` if it cannot be disabled.

### Adding a New Extension Type

1. Define types in `oxi-cli/src/extensions/types.rs`.
2. Implement loading in `oxi-cli/src/extensions/loading.rs` (native) or `wasm.rs` (WASM).
3. Register hooks in `oxi-cli/src/extensions/registry.rs`.

## Common Commands

```bash
# Build & Test
cargo build                          # Debug build
cargo build --release                # Release binary
cargo nextest run --workspace        # Run all tests (parallel)
cargo nextest run -p oxi-agent       # Test single crate
cargo nextest run --profile ci       # CI profile (retries, no fail-fast)
cargo nextest run -j 1               # Sequential (debug race conditions)

# Lint & Format
cargo clippy --workspace -- -D warnings   # Lint
cargo fmt --all -- --check           # Format check

# Docs
cargo doc --workspace --no-deps      # Build docs
cargo test --workspace --doc         # Doc tests
```

## File Locations

| Purpose | Path |
|---------|------|
| Global config | `~/.oxi/settings.toml` |
| Project config | `.oxi/settings.toml` |
| Sessions | `~/.oxi/sessions/` |
| Extensions | `~/.oxi/extensions/` |
| Skills | `~/.oxi/skills/<name>/SKILL.md` |
| MCP config | `~/.config/oxi/mcp.json` or `.oxi/mcp.json` |
| Logs | `~/.oxi/logs/` |
| Nextest config | `.config/nextest.toml` |

## Design Principles

1. **Streaming-first** — every provider streams tokens. No blocking request/response.
2. **Provider-agnostic** — all LLM providers share the same `Provider` trait and message types.
3. **Append-only sessions** — conversation history is never mutated, only appended. Branching creates new entries.
4. **Progressive enhancement** — core works with zero config. Settings, extensions, skills, MCP are all opt-in layers.
5. **Sandboxed extensions** — WASM extensions get zero host access by default. Permissions must be explicitly requested.
6. **Atomic I/O** — file writes go through temp+rename to prevent corruption on crash.

## Pitfalls

- `oxi-ai` has no dependency on other oxi crates. Do not import `oxi_agent` or `oxi_store` from it.
- Session entries form a tree via `parent_id`, not a flat list. Always traverse with this in mind.
- Provider message formats differ significantly (Anthropic vs OpenAI). Use `transform.rs` for conversion.
- The tool-calling loop in `agent_loop/` has retry logic — tool implementations must be idempotent.
- `model_db.rs` is large (934 models). Manual edits may be overwritten by regeneration scripts.
- SSE parsing handles partial UTF-8 lines. Do not assume line boundaries are clean.
- `Agent::is_running` field prevents concurrent agent runs — check this before spawning parallel tasks.