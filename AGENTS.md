# oxi

Rust port of [pi](https://github.com/earendil-works/pi) — terminal-based AI coding assistant. Multi-crate workspace providing multi-provider LLM access, an agent tool-calling loop, a terminal UI, a port-based adapter system, and an SDK for building multi-agent systems.

## Quick Facts

| Item | Value |
|------|-------|
| Language | Rust 2024 edition |
| Workspace crates | `oxi-ai`, `oxi-agent`, `oxi-tui`, `oxi-sdk`, `oxi-cli` (5 crates) |
| Version | see `Cargo.toml` / `git tag` — single source of truth (do NOT hardcode the number here; it drifts) |
| License | MIT |
| CI | `cargo fmt`, `cargo clippy -D warnings`, `cargo nextest run`, `cargo audit`, `cargo deny check` |
| Workflows | `ci.yml` (8 jobs: fmt/clippy/clippy-native-browser/smoke-test/audit/deny/msrv/doc), `test.yml` (macOS-only matrix + doc), `pr-gate.yml`, `release.yml` (aarch64-apple-darwin + SHA256SUMS + SBOM), `build-binaries.yml`, `publish.yml` (crates.io), `sbom.yml`, `labels.yml` |

> The legacy `oxi-store` crate (settings, sessions, auth) was absorbed
> into `oxi-cli/src/store/` as a self-contained sub-module. The legacy
> `oxi-fs` crate (file-based port adapters) was absorbed into
> `oxi-sdk/src/ports/fs/`. See the refactor history in CHANGELOG.md.

## Workspace Layout

```
oxi/
├── oxi-ai/       Unified LLM API — streaming, multi-provider abstraction
├── oxi-agent/    Agent runtime — tool-calling loop, MCP client, built-in tools
├── oxi-tui/      Terminal UI widgets — chat, themes, markdown rendering (ratatui)
├── oxi-sdk/      Multi-agent SDK + port contract: 11 port traits + reference impls
└── oxi-cli/      CLI binary — composition root (TUI + RPC + print modes)
```

### Dependency Flow

```
oxi-ai  ←  oxi-agent  ←  oxi-sdk  ←  oxi-cli
oxi-tui  (independent, no oxi-* deps)  ←  oxi-cli
```

`oxi-ai` is the foundation layer with zero internal dependencies.
`oxi-cli` is the integration layer that depends on all other crates.
Never create circular dependencies between crates.

## Port System (oxi-sdk)

`oxi-sdk` defines **11 port traits** as the contract between the SDK
and product-specific infrastructure. Each port has a noop default;
products register their own implementations via `OxiBuilder::with_port_*`
or `with_ports(PortRegistry)`.

| Port | Purpose | oxi-cli uses | oxios (sister repo) uses |
|---|---|:-:|:-:|
| `StateStore` | Durable key-value / append-only | ✅ `FileStateStore` | 🔜 TBD |
| `ConfigStore` | Layered configuration | ✅ `FileConfigStore` | 🔜 TBD |
| `AuthProvider` | API keys + OAuth | ✅ `FileAuthProvider` | 🔜 TBD |
| `EventBus` | pub/sub kernel events | ✅ `InProcessEventBus` | 🔜 TBD |
| `SkillLoader` | SKILL.md discovery | ✅ `FileSkillLoader` | 🔜 TBD |
| `PersonaProvider` | System-prompt fragments | ✅ `FilePersonaProvider` | 🔜 TBD |
| `AccessGate` | Pre-execution policy | ✅ `SimpleAccessGate` | 🔜 TBD |
| `CapabilityResolver` | Subject → tool visibility | ✅ `TomlCapabilityResolver` | 🔜 TBD |
| `MemoryStore` | Episodic / semantic | ✅ `InMemoryMemoryStore` | 🔜 TBD |
| `CronScheduler` | Time-based triggers | ✅ `InMemoryCronScheduler` | 🔜 TBD |
| `ResourceMonitor` | Usage limits | ✅ `CountingResourceMonitor` | 🔜 TBD |

Reference implementations live in `oxi-sdk/src/ports/fs/` (file-based)
and `oxi-sdk/src/ports/inmem/` (in-memory). See `docs/PORT_GUIDE.md`
for the full contract, the noop-fallback semantics, and patterns for
writing new impls.

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
`model_db.rs` indexes pricing/context/feature data for 1099 models across 30+
providers, sourced from a 3-tier catalog (`data/catalog/*.toml`):
- **Layer 1** — built-in TOML (`include_str!`-ed at build time)
- **Layer 2** — user overrides (`~/.oxi/catalog/overrides.toml`)
- **Layer 2.5** — **models.dev live enrichment** (`catalog/models_dev.rs`):
  fetches `https://models.dev/api.json` (5-min cache) to fill `0.0` price gaps
  and refresh context/max_tokens/reasoning. MIT upstream (also used by opencode).
  Gates: `OXI_MODELS_DEV`, `OXI_MODELS_DEV_URL`, `OXI_MODELS_DEV_DISABLE_FETCH`,
  `OXI_MODELS_DEV_TTL`, `OXI_MODELS_DEV_CACHE_PATH`.
- **Layer 3** — runtime `/v1/models` discovery for local servers (ollama/lmstudio/vllm/sglang)
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

### oxi-tui — Terminal UI

Built on `ratatui` + `crossterm`. **No oxi-* dependencies** — pure widget library.

- Theme system with hot-reload from TOML/JSON files.
- Markdown rendering via `pulldown-cmark`. Fuzzy search for file/command completion.
- `widgets/chat/` is the main conversation widget.
- The widget layer defines its own domain types (`ChatMessage`,
  `MessageRole`, `ContentBlock`) so it can be reused by any product
  that wants the chat UX. Products implement the conversion
  (one `From` impl per direction) in their own composition root.

Key types: `Theme`, `ThemeManager`, `ChatWidget`, `ToolRenderer`.

### oxi-sdk — Multi-Agent SDK + Port Contract

`OxiBuilder` is the entry point:

```rust
let oxi = OxiBuilder::new()
    .with_builtins()
    .with_state(Arc::new(my_state_store))
    .with_auth(Arc::new(my_auth))
    .build();
let agent = oxi.agent(AgentConfig { /* ... */ }).build()?;
```

`AgentGroup` supports parallel, sequential, and fan-out strategies.
`MessageBus` provides pub/sub inter-agent communication.
`KernelToolProvider` bridges SDK agents to the host tool registry.

The SDK is **re-exported through the oxi-sdk crate** — products do
not depend on `oxi-ai` or `oxi-agent` directly. See `docs/PORT_GUIDE.md`
for the full port contract and "single dependency" pattern
(`oxios → oxi-sdk`, no `oxi-ai` direct dep).

Key types: `Oxi`, `OxiBuilder`, `AgentBuilder`, `AgentGroup`, `MessageBus`, `PortRegistry`.

### oxi-cli — CLI Binary

Single binary with three run modes: **TUI** (interactive), **print**
(plain or JSON), **RPC** (JSON-over-stdin/stdout for IDE integration).

Composition root: `oxi-cli/src/main.rs` is a 12-line dispatcher that
calls `oxi::bootstrap::run_with_args(args)`. All wiring (settings
merge, custom-provider registration, router registration, built-in
tool registration, WASM extension loading) lives in
`oxi-cli/src/bootstrap.rs`. The run-mode dispatcher
(`dispatch_run_mode`) routes to TUI / print / RPC based on flags.

The `App` struct is built via `App::from_oxi(oxi, settings)` from
the wired `Oxi` engine — no manual `Agent::new(provider, config, ...)`
calls anymore. Subcommand handlers (config, session, export, share,
setup, models) stay in `main.rs` because they are dispatch targets,
not bootstrap.

Self-contained submodules in `oxi-cli/src/`:

- `store/` — domain types and file-based adapters (was `oxi-store`):
  `session.rs` (JSONL session persistence), `settings.rs` (layered
  config), `auth_storage.rs` (API keys + OAuth), `router_config.rs`
  (auto-routing rules), `session_cwd.rs` (cwd binding).
- `bootstrap.rs` — composition root.
- `tui/` — interactive mode entry points and app glue
  (`tui/app.rs`, `tui/handlers.rs`, `tui/slash.rs`,
  `tui/overlay/*`).
- `app/agent_session*.rs` — single-shot session wrapper around
  `Agent`.
- `setup_wizard.rs` — interactive `oxi setup`.
- `rpc_mode/` — JSON-RPC mode.
- `extensions/` — WASM / native extension loading.
- `storage/packages.rs` — built-in package manager (ClawHub-style).

Extension system (`src/extensions/types.rs`):

- `ExtensionManifest`: metadata with permissions (FileRead, FileWrite, Bash, Network)
- `ExtensionState`: Pending, Active, Disabled, Failed, Unloaded
- `InputEventResult`: Continue, Transform { text }, Handled

## Conventions

### Code Style

- `cargo fmt` before every commit — no exceptions.
- `cargo clippy --workspace -- -D warnings` must pass clean.
  `clippy --all-targets` is **not** enforced yet (test/bench code has
  pre-existing `unwrap()` and pedantic lints) — see "Pre-existing TODO"
  below.
- **`native-browser` feature must always compile.** The `ci.yml`
  `clippy-native-browser` job runs `cargo clippy -p oxi-sdk --features
  native-browser -- -D warnings` on every PR. This feature compiles
  `oxibrowser_backend.rs` — if you change `BrowserTab`/`BrowserEngine`
  traits or their impls, this job catches edition-2024 lifetime bugs
  that `cargo clippy --workspace` (default features) cannot see. To
  verify locally:
  ```bash
  cargo clippy -p oxi-sdk --features native-browser -- -D warnings
  cargo build -p oxi-agent --features native-browser
  ```
- **Pre-commit hooks** — `.pre-commit-config.yaml` mirrors the ci.yml
  gate. Install once: `pre-commit install`. On every `git commit`,
  trailing whitespace, EOF, YAML/TOML lint, merge-conflict, large
  files, private-key scan, and `cargo fmt --check` /
  `cargo clippy --all-targets` run automatically. PRs that fail
  locally fail remotely too.
- Module structure: `mod.rs` re-exports public API, implementation in sibling files.
- Prefer `anyhow::Result` for application code, custom error enums (`thiserror`) for library crates.
  - **Library crates** (oxi-ai, oxi-agent, oxi-sdk): define typed error enums with `thiserror::Error` for public API functions. Internal helpers may use `anyhow`.
  - **Application crate** (oxi-cli): use `anyhow::Result` everywhere.
  - **Leaf crate** (oxi-tui): `anyhow` is acceptable — no public error types needed.
  - Never create a shared workspace error crate. Each library owns its own error type.
- Use `parking_lot::RwLock` instead of `std::sync::RwLock`. (But
  `parking_lot::MutexGuard` is `!Send` — drop the guard before any
  `.await` or use `tokio::sync::Mutex` instead.)
- Atomic file writes: use the `temp + rename` pattern
  (e.g. `oxi-sdk/src/ports/fs/session.rs::FileStateStore`).
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
4. Add model data to `oxi-ai/data/catalog/models/<provider>.toml` (no Rust
   changes needed — `build.rs` picks it up). If the provider exists on
   models.dev, the Layer 2.5 enrichment will fill pricing/limits
   automatically at runtime.

### Adding a New Tool

1. Create `oxi-agent/src/tools/<name>.rs`.
2. Implement the `AgentTool` trait.
3. Add module declaration in `oxi-agent/src/tools.rs`.
4. Register in `ToolRegistry::with_builtins_cwd()`.
5. Mark `essential()` as `true` if it cannot be disabled.

### Adding a New Port Implementation

1. Implement the port trait from `oxi_sdk::ports::*`.
2. The SDK does not include concrete adapters (beyond `fs/` and
   `inmem/`) — products write their own.
3. Register via `OxiBuilder::with_port_*(Arc::new(my_impl))` in your
   composition root.

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
| **models.dev cache** | `~/.oxi/cache/models-dev.json` (5-min TTL, atomic writes) |
| MCP config | `~/.config/oxi/mcp.json` or `.oxi/mcp.json` |
| Logs | `~/.oxi/logs/` |
| Nextest config | `.config/nextest.toml` |
| Pre-commit config | `.pre-commit-config.yaml` |
| Issue labels | `.github/labels.yml` (synced by `labels.yml` workflow) |
| Funding | `.github/FUNDING.yml` |

## CI/CD & Release Pipeline

oxi ships a multi-stage pipeline. The full source is under
`.github/workflows/`:

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | PR + push to main/develop | Fast feedback: `fmt`, `clippy`, `clippy-native-browser`, `smoke-test`, `audit`, `deny`, `msrv`, `doc`. ~2-3 min for the fast jobs. |
| `test.yml` | PR + push to main + release | Full nextest matrix on **macos-latest** (Apple Silicon), plus doc tests. Replaces the older "smoke on PR, full on main" split. |
| `pr-gate.yml` | PR opened/synchronized/reopened | Conventional-Commit title, PR size ≤ 4000 lines, no merge commits, issue linkage. |
| `release.yml` | `v*` tag push | Build `aarch64-apple-darwin`, `tag-check` (rejects stale tags), `SHA256SUMS` + CycloneDX SBOM, GitHub Release. |
| `build-binaries.yml` | weekly cron + manual | Continuous binary build (no release artifact) for sanity. |
| `publish.yml` | `release: published` + manual | `cargo publish` to crates.io in topological order with a dry-run pre-flight. Requires `CARGO_TOKEN`. |
| `sbom.yml` | push to main + release | Generates CycloneDX 1.5 SBOM and submits it to GitHub's dependency-graph API. |
| `labels.yml` | weekly + labels.yml change | Syncs `.github/labels.yml` to the repo's label set via `github/issue-labeler`. |

### Required GitHub Secrets

| Secret | Used by | Required? | How to create |
|--------|---------|:---:|---------------|
| `CARGO_TOKEN` | `publish.yml` | ✅ **Yes** (to publish) | <https://crates.io/settings/tokens>, scope: publish |

**Scope decisions (2026-06-07):**

- **Distribution channel:** crates.io only. No Homebrew tap, no Scoop bucket.
- **Build target:** `aarch64-apple-darwin` (macOS Apple Silicon) only.
  The maintainer does not have access to Linux or Windows build
  environments, so cross-OS verification is not part of this pipeline.
  To re-enable other targets, add an entry to the `matrix` in
  `release.yml`/`build-binaries.yml` and a matching runner in
  `test.yml`.
- **Supply chain:** SHA256SUMS generated on every release (unsigned).
  No GPG signing, no Codecov coverage reporting.

With **only** `CARGO_TOKEN` configured, the full pipeline runs:
CI gates (`ci.yml`) + tests (`test.yml`) + PR gate + release build
(`release.yml`) + crates.io publish (`publish.yml`) + SBOM + label sync.

## Design Principles

1. **Streaming-first** — every provider streams tokens. No blocking request/response.
2. **Provider-agnostic** — all LLM providers share the same `Provider` trait and message types.
3. **Append-only sessions** — conversation history is never mutated, only appended. Branching creates new entries.
4. **Progressive enhancement** — core works with zero config. Settings, extensions, skills, MCP are all opt-in layers.
5. **Sandboxed extensions** — WASM extensions get zero host access by default. Permissions must be explicitly requested.
6. **Atomic I/O** — file writes go through temp+rename to prevent corruption on crash.
7. **Port-based adapters** — infrastructure (state, auth, events, memory, skills, …) is **opt-in**. Products register only the ports they care about; the SDK provides noop fallbacks for the rest.
8. **SDK is the contract, not the implementation** — `oxi-sdk` defines port traits and ships small reference impls. Products (oxi-cli, oxios) write their own domain-specific impls.
9. **Composition root in one place** — each binary has a single module that wires its `Oxi` engine. Wiring code does not leak into business logic.

## Pitfalls

- `oxi-ai` has no dependency on other oxi crates. Do not import `oxi_agent` or `oxi_store` from it.
- Session entries form a tree via `parent_id`, not a flat list. Always traverse with this in mind.
- Provider message formats differ significantly (Anthropic vs OpenAI). Use `transform.rs` for conversion.
- The tool-calling loop in `agent_loop/` has retry logic — tool implementations must be idempotent.
- The catalog lives in `data/catalog/*.toml`, not hand-written Rust. Adding a
  TOML file requires no Rust code (build script auto-enumerates). Many oxi-
  original entries ship `cost_input`/`cost_output` = `0.0`; these are
  transparently enriched at runtime by the models.dev Layer 2.5
  (`catalog/models_dev.rs`). Set `OXI_MODELS_DEV=off` to test Layer 1 alone.
- SSE parsing handles partial UTF-8 lines. Do not assume line boundaries are clean.
- `Agent::is_running` field prevents concurrent agent runs — check this before spawning parallel tasks.
- Port trait methods are **async**. `MutexGuard`s held across `.await` will not compile (`!Send`). Use `tokio::sync::Mutex` or scope the lock.
- The legacy `oxi-store` crate no longer exists. If a new file needs session/settings/auth, put it in `oxi-cli/src/store/` (or in a sibling product's store).
- The `oxi-cli` crate is a **monorepo monolith** by design (~17K lines). Do not split it into more crates — the 4 separation conditions (independent reuse, independent versioning, build isolation, team boundary) do not hold.
- **TUI language policy is TUI-only — by design, not oversight.** `Settings::output_languages` (see `oxi-cli/src/store/settings.rs`) is consumed **exclusively** by `app::agent_session_runtime::build_system_prompt` (the TUI session build path). The `lib.rs` App build path used by `oxi --print` and RPC mode does **not** inject the policy — it has its own simpler `build_system_prompt` that omits it. So a policy written to `settings.toml` is **silently ignored** in non-TUI modes.
  - **Why this asymmetry is intentional:** `oxi --print` and RPC mode are programmatic/scriptable interfaces where language determinism is the caller's responsibility (they control the prompt and can pre-translate or route through any language). The TUI is the conversational surface where this policy earns its place.
  - **Do NOT "fix" the asymmetry** by injecting the directive into `oxi-cli/src/lib.rs::build_system_prompt` without an explicit product decision. If a future caller needs the policy in print/RPC, add an explicit opt-in (CLI flag or extra config field) rather than making it implicit. The single injection point to add later is `oxi-cli/src/lib.rs::build_system_prompt`.
  - **Default is OFF (opt-in).** As of v6, `language_policy_enabled: bool` defaults to `false` even with non-empty `output_languages`. Users must toggle it ON in the `/settings` overlay for the policy to take effect. See `docs/designs/2026-06-17-tui-language-policy.md` for the rationale.
  - **Note:** `/settings` opens the **editable** settings overlay (not a read-only view). Its description and handler now live together on the `SettingsCommand` struct in `tui/slash/builtin/tools_commands.rs` — the slash-command registry removed the old `BUILTIN_SLASH_COMMANDS`-table-vs-`tui/slash.rs`-handler split, so there is nothing to "keep in sync" anymore.
- **TUI language policy is a strong default, NOT a hard guarantee.** The system prompt carries a "MUST" directive and the compaction summarizer sees a "Focus areas:" instruction, but both are prompt-level signals. The model can still occasionally violate the policy when context grows long, when tool output is echoed verbatim, or when the summarizer reframes it as "focus areas" (weaker than the main prompt's MUST). See the docstrings on `Settings::output_languages`, `BuildSystemPromptOptions::language_directive`, `AgentSession::rebuild_system_prompt`, and `build_compaction_instruction` for the full caveat list. To get 100% enforcement you need additional layers (tool output wrapping, response post-processing) — out of scope for the current MVP.
- **Channels are prompt-level directives, not classifiers.** `KNOWN_CHANNELS` (`response`, `code_comment`, `documentation`, `commit_message`) is a label table for the directive string, not a runtime tagger. The model is asked to apply these rules per-output, but it has no mechanism to classify its own output by channel. Boundary cases (commit message quoted inside a response, code-comment-as-explanation, etc.) may be misapplied. See `docs/designs/2026-06-17-tui-language-policy.md` §3.4 for the canonical cases.
