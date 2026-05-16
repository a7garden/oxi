# oxi

Terminal-based AI coding assistant built in Rust. Multi-crate workspace providing multi-provider LLM access, an agent tool-calling loop, a terminal UI, a persistent session store, and an SDK for building multi-agent systems.

## Quick Facts

| Item | Value |
|------|-------|
| Language | Rust 2021 edition |
| Workspace crates | `oxi-ai`, `oxi-agent`, `oxi-store`, `oxi-tui`, `oxi-sdk`, `oxi-cli` |
| Version | 0.15.x |
| License | MIT |
| CI | `cargo fmt`, `cargo clippy -D warnings`, `cargo test`, `cargo audit` |

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

Provider-agnostic streaming interface. The core trait is `Provider`:

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

**8 providers**: OpenAI, Anthropic, Azure, Google, Vertex AI, Bedrock, Mistral, OpenAI Responses.
Each lives in `oxi-ai/src/providers/<name>.rs`.
`model_db.rs` contains pricing/context/feature data for 50+ models.
`compaction.rs` summarizes old messages when context grows too large.

Key types: `Model`, `Context`, `Message`, `ContentBlock`, `Tool`, `ProviderEvent`, `ProviderError`.

### oxi-agent — Agent Runtime

Manages the LLM tool-calling loop. The core trait is `AgentTool`:

```rust
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn essential(&self) -> bool { false }
    async fn execute(&self, tool_call_id: &str, args: &Value) -> ToolResult;
}
```

**15 built-in tools** in `oxi-agent/src/tools/`: `bash`, `read`, `write`, `edit`, `ls`, `find`, `grep`, `github`, `github_search`, `subagent`, `questionnaire`, `context7` (2 sub-tools: resolve-library-id, query-docs), `web_search`, `get_search_results`.

`agent_loop/` contains the retry/streaming/queue logic.
`mcp/` implements a Model Context Protocol client.
`proxy.rs` handles permission gating.

Key types: `Agent`, `AgentEvent`, `AgentState`, `AgentConfig`, `ToolRegistry`.

### oxi-store — Persistent State

Append-only JSONL session storage with tree branching (fork).
Layered settings: defaults → global (`~/.oxi/settings.toml`) → project (`.oxi/settings.toml`) → env vars → CLI args.
`auth_storage.rs` stores API keys and OAuth tokens securely.
`model_resolver.rs` resolves model IDs to concrete `Model` structs.

Key types: `SessionEntry`, `AgentMessage`, `Settings`, `SessionManager`.

### oxi-tui — Terminal UI

Built on `ratatui` + `crossterm`. Theme system with hot-reload from TOML/JSON files.
Markdown rendering via `pulldown-cmark`. Fuzzy search for file/command completion.

Key types: `Theme`, `ThemeManager`, `ChatWidget`, `ToolRenderer`.

### oxi-sdk — Multi-Agent SDK

Builder pattern for constructing agents. `AgentGroup` supports parallel, sequential, and fan-out strategies.
`MessageBus` provides pub/sub inter-agent communication.
`KernelToolProvider` bridges SDK agents to the host tool registry.

Key types: `AgentBuilder`, `AgentGroup`, `MessageBus`, `ClosureTool`.

### oxi-cli — CLI Binary

Entry point: `oxi-cli/src/main.rs` (uses `clap`).
Runs in two modes: **TUI mode** (interactive terminal) and **RPC mode** (JSON-over-stdin/stdout for IDE integration).
`extensions/` supports two extension mechanisms: native shared libraries (`.dylib`/`.so`/`.dll`) and WASM sandboxed plugins (via Extism).
`skills/` loads markdown skill files from `~/.oxi/skills/<name>/SKILL.md`.
`storage/packages.rs` is the built-in package manager.

## Conventions

### Code Style

- `cargo fmt` before every commit — no exceptions.
- `cargo clippy --workspace -- -D warnings` must pass clean.
- Module structure: `mod.rs` re-exports public API, implementation in sibling files.
- Prefer `anyhow::Result` for application code, custom error enums (`thiserror`) for library crates.
- Use `parking_lot::RwLock` instead of `std::sync::RwLock`.
- Atomic file writes: use `atomic_write()` helper (write to temp, then rename).
- Async: `tokio` runtime with `#[tokio::main]`. Use `async_trait` for trait objects.

### Testing

- Unit tests in `#[cfg(test)] mod tests` within each module.
- Integration tests in `<crate>/tests/*.rs`.
- Mock providers (`MockProvider`) for agent/loop testing.
- `cargo test --workspace` must pass before merge.
- Benchmarks live in `<crate>/benches/` (Criterion).

### Adding a New LLM Provider

1. Create `oxi-ai/src/providers/<name>.rs`.
2. Implement the `Provider` trait.
3. Register in `oxi-ai/src/providers/register_builtins.rs`.
4. Add model data to `model_db.rs` if needed.

### Adding a New Tool

1. Create `oxi-agent/src/tools/<name>.rs`.
2. Implement the `AgentTool` trait.
3. Register in `oxi-agent/src/tools.rs`.
4. Add tests in `oxi-agent/src/tests.rs` or `oxi-agent/tests/`.

### Adding a New Extension Type

1. Define the interface in `oxi-cli/src/extensions/types.rs`.
2. Implement loading in `oxi-cli/src/extensions/loading.rs` (native) or `wasm.rs` (WASM).
3. Register hooks in `oxi-cli/src/extensions/registry.rs`.

## Common Commands

```bash
cargo build                          # Debug build
cargo build --release                # Release binary
cargo test --workspace               # Run all tests
cargo test -p oxi-agent              # Test single crate
cargo clippy --workspace -- -D warnings   # Lint
cargo fmt --all -- --check           # Format check
cargo bench -p oxi-ai                # Run benchmarks
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
- `model_db.rs` is auto-generated from provider APIs. Manual edits will be overwritten.
- SSE parsing handles partial UTF-8 lines. Do not assume line boundaries are clean.
