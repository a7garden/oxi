# oxi

Rust port of [pi](https://github.com/earendil-works/pi) — terminal-based AI coding assistant. Multi-crate workspace providing multi-provider LLM access, an agent tool-calling loop, a terminal UI, a port-based adapter system, and an SDK for building multi-agent systems.

## Quick Facts

| Item | Value |
|------|-------|
| Language | Rust 2024 edition |
|Workspace crates|10 crates — see "Workspace Layout" below (do NOT hardcode the count; the set evolves)|
| Version | see `Cargo.toml` / `git tag` — single source of truth (do NOT hardcode the number here; it drifts) |
| License | MIT |
| CI | `cargo fmt`, `cargo clippy -D warnings`, `cargo nextest run`, `cargo audit`, `cargo deny check` |
| Workflows | `ci.yml` (8 jobs: fmt/clippy/clippy-native-browser/smoke-test/audit/deny/msrv/doc), `test.yml` (macOS-only matrix + doc), `pr-gate.yml`, `publish.yml` (crates.io, unified), `build-binaries.yml`, `sbom.yml`, `labels.yml` |

> The legacy `oxi-store` crate (settings, sessions, auth) was absorbed
> into `oxi-cli/src/store/` as a self-contained sub-module. The legacy
> `oxi-fs` crate (file-based port adapters) was absorbed into
> `oxi-sdk/src/ports/fs/`. See the refactor history in CHANGELOG.md.

## Workspace Layout

```
oxi/
├── oxi-ai/            Unified LLM API — streaming, multi-provider abstraction (foundation)
├── oxi-agent/         Agent runtime — tool-calling loop, MCP client, built-in tools
├── oxi-sdk/           Multi-agent SDK + port contract: 15 port traits + reference impls
├── oxi-cli/           CLI binary — composition root (TUI + RPC + print modes)
├── oxi-hashline/      Line-anchored patch format for AI-assisted code editing
├── oxi-lsp/           LSP bridge
├── oxi-mnemopi/       Local SQLite vector memory engine (ported from omp Mnemopi)
├── oxi-snapcompact/   Context compaction via PNG rasterization (fontdue)
├── oxi-tui/           Terminal UI — widgets, theme, glyph, render (ratatui + DiffBackend)
```

> The grok-inspired `oxi-tui` v2 crate was retired (P2.1, 2026-07-29).
> The legacy crate was renamed to `oxi-tui` as the sole TUI crate.
> Production chat rendering now uses the main-screen `TapeEngine`
> (`oxi-tui/src/tape/`); ratatui remains only for transient overlays and
> off-screen line formatting.

### Dependency Flow

Leaf crates (zero internal `oxi-*` deps): `oxi-ai`, `oxi-hashline`, `oxi-lsp`,
`oxi-mnemopi`, `oxi-snapcompact`, `oxi-tui`.

```
oxi-ai  (foundation)              oxi-hashline (independent)
  ↓                                 ↓
oxi-agent  ←  oxi-ai, oxi-hashline
  ↓
oxi-sdk  ←  oxi-ai, oxi-agent, oxi-snapcompact
  ↓
oxi-cli  ←  oxi-ai, oxi-agent, oxi-sdk, oxi-lsp, oxi-mnemopi, oxi-tui
```

`oxi-ai` is the foundation layer with zero internal dependencies.
`oxi-cli` is the integration layer that depends on all other crates.
Never create circular dependencies between crates.

## Port System (oxi-sdk)

`oxi-sdk` defines **15 port traits** as the contract between the SDK
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
| `InternalUrlRouter` | Resolve internal URIs (`skill://`, `issue://`, …) | ✅ wired | 🔜 TBD |
| `ProtocolHandler` | Handle internal-protocol requests | ✅ wired (7 impls: issue, pr, memory, skill, rule, agent, local in `services.rs`) | 🔜 TBD |
| `RuleRegistry` | Project steering rules (TTSR) | ✅ wired | 🔜 TBD |
| `EmbeddingProvider` | Vector embeddings for memory | 🔜 TBD | 🔜 TBD |

(Plus `ModelCatalog` in `ports/catalog.rs` for catalog/model-data access.) See `oxi-sdk/src/ports/mod.rs` for the canonical trait list.

Reference implementations live in `oxi-sdk/src/ports/fs/` (file-based)
and `oxi-sdk/src/ports/inmem/` (in-memory). See `docs/PORT_GUIDE.md`
for the full contract, the noop-fallback semantics, and patterns for
writing new impls.

## Architecture Overview

### oxi-ai — Unified LLM API

Provider-agnostic streaming interface. Core trait in `providers/trait_def.rs`:

```rust
pub trait Provider: Send + Sync + 'static {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>>;
}
```

> **Identity ≠ transport.** The trait has **no `name()`** — provider identity
> (the canonical catalog id) lives in the registry key and `Model.provider`,
> never on the transport (Step 2 / P0.3 three-way split).

**8 built-in providers** in `src/providers/`: openai, openai-responses, anthropic, google, vertex, azure, bedrock, ollama.
`model_db.rs` + `catalog/` index pricing/context/feature data for 5000+ models
across 70+ providers, with models.dev as the source of truth (see
`data/catalog/README.md`):
- **SNAP (Layer 1)** — embedded models.dev snapshot `_snapshot.json.gz`
  (`include_bytes!`-ed; works fully offline on first run).
- **LIVE (Layer 2.5)** — runtime cache `~/.oxi/cache/models-dev.json` (ETag-aware
  conditional GET, ~1h mtime window). `catalog/models_dev.rs`.
- **Layer 2** — user overrides (`~/.oxi/catalog/overrides.toml`).
- **LOCAL (Layer 3)** — runtime `/v1/models` discovery for local servers
  (ollama/lmstudio/vllm/sglang).
  Gates: `OXI_MODELS_DEV`, `OXI_MODELS_DEV_URL`, `OXI_MODELS_DEV_DISABLE_FETCH`,
  `OXI_MODELS_DEV_MTIME_WINDOW`, `OXI_MODELS_DEV_FORCE_REFRESH`,
  `OXI_MODELS_DEV_CACHE_PATH`, `OXI_CATALOG_SNAPSHOT`.
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

**~21 tools** across `src/tools/` (registered by `ToolRegistry::with_builtins_cwd`, plus `ask` wired by the `oxi-cli` composition root): read, write, edit, bash, grep, find, ls, todo, ask, web_search, get_search_results, github, subagent, memory_recall, memory_reflect, memory_retain, memory_edit, mcp, context7 (2 sub-tools), generate_image, commit.
**7 essential tools** (cannot be disabled): read, write, edit, bash, grep, find, ls.
`agent_loop/` contains streaming, tool execution, retry logic, and queue management.
`mcp/` implements Model Context Protocol client.
`agent.rs` has `ProviderResolver` trait for resolving provider/model by name.

Key types: `Agent`, `AgentEvent`, `AgentState`, `AgentConfig`, `ToolRegistry`.

### oxi-tui — Terminal UI

`oxi-tui` provides ratatui-based TUI widgets, theme system, glyph system,
and the OMP-aligned tape engine. **No oxi-* dependencies** — pure widget
library. Production oxi-cli renders chat transcripts on the terminal main
screen through `tape::TapeEngine`; ratatui is retained for transient
overlay sessions and off-screen line formatting. See
`docs/superpowers/specs/2026-07-29-p2-tui-tape-model-design.md`.

- Theme system with hot-reload from TOML/JSON files.
- **Glyph set system** (`symbols.rs`): every UI symbol (status markers, list
  cursors, box drawing, spinners, icons) comes from a pluggable `GlyphSet`
  preset — `Unicode` (default), `Ascii`, or `Nerd`. The active `Symbols`
  table rides on `Theme`/`ThemeStyles`, so `styles.symbols.<field>` is the
  single source for any glyph. **Never hardcode a symbol in a widget** — read
  it from the symbol table so the `glyph_set` setting re-skins the whole UI.
  Adding a glyph: add a field to `Symbols`, populate all three preset
  constructors (`unicode`/`ascii`/`nerd`), migrate the one call site.
- Markdown rendering via `pulldown-cmark`. Fuzzy search for file/command completion.
- `widgets/chat/` is the main conversation widget.
- The widget layer defines its own domain types (`ChatMessage`,
  `MessageRole`, `ContentBlock`) so it can be reused by any product
  that wants the chat UX. Products implement the conversion
  (one `From` impl per direction) in their own composition root.

Key types: `Theme`, `ThemeManager`, `ChatWidget`, `ToolRenderer`, `GlyphSet`, `Symbols`.

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

Composition root: `oxi-cli/src/main.rs` is the binary entry point. The
top-level `main()` is a thin dispatcher (12 lines) that routes
to either `handle_subcommand(command)` for non-interactive
subcommands or `oxi::bootstrap::run_with_args(args)` for the
default TUI/print/RPC modes. **However**, `handle_subcommand`
itself is a large match arm (~90 lines) that delegates each
subcommand to an inline `handle_*` function declared in the same
file (F-5 audit 2026-06-21). Those `handle_*` functions together
add ~1,400 LOC to `main.rs` — contradicting the older "12-line
dispatcher" claim that this paragraph originally made. The
follow-up refactor (move each `handle_*` to `oxi-cli/src/cli/commands/*.rs`,
see audit F-5 in `oxi-code-audit-report.html`) is non-trivial because
clap's `Subcommand`-derived enums can hit generic-bound surprises when
referenced from a sibling module — a structural split should be done in
a dedicated PR with explicit testing of every subcommand.

All wiring (settings merge, custom-provider registration, router
registration, built-in tool registration, WASM extension loading) lives
in `oxi-cli/src/bootstrap.rs`. The run-mode dispatcher
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
- `cargo clippy --workspace --all-targets -- -D warnings` must pass clean
  (the ci.yml `clippy` job and the pre-commit hook both enforce this).
  Test/bench/example code relaxes exactly two test-idiom lints —
  `clippy::unwrap_used` and `clippy::field_reassign_with_default` — via
  `#![cfg_attr(test, allow(...))]` at each library crate root; shipped
  (non-test) code still `warn`s on `unwrap_used`. Every other lint
  (correctness, suspicious, style, complexity) is enforced even in tests.
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
4. The model catalog is powered by models.dev (see `oxi-ai/data/catalog/README.md`).
   If the provider exists on models.dev, its models appear automatically once the
   embedded snapshot is refreshed (regenerate `data/catalog/_snapshot.json.gz`).
   Add oxi-specific provider metadata (extra HTTP headers, etc.) to
   `data/catalog/product-meta.toml`.

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
cargo clippy --workspace --all-targets -- -D warnings   # Lint
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
| `publish.yml` | `v*` tag push + manual | Single workflow: tag-on-main verification, packaging dry-run, then `cargo publish` in topological order. All on `ubuntu-latest` (GitHub-hosted). No binaries, no GitHub Release, no self-hosted runners. |
| `pr-gate.yml` | PR opened/synchronized/reopened | Conventional-Commit title, PR size ≤ 4000 lines, no merge commits, issue linkage. |
| `build-binaries.yml` | weekly cron + manual | Continuous binary build (no release artifact) for sanity. |
| `sbom.yml` | push to main + release | Generates CycloneDX 1.5 SBOM and submits it to GitHub's dependency-graph API. |
| `labels.yml` | weekly + labels.yml change | Syncs `.github/labels.yml` to the repo's label set via `EndBug/label-sync`. |

### Required GitHub Secrets

| Secret | Used by | Required? | How to create |
|--------|---------|:---:|---------------|
| `CARGO_TOKEN` | `publish.yml` | ✅ **Yes** (to publish) | <https://crates.io/settings/tokens>, scope: publish |

**Scope decisions (2026-06-20):**

- **Distribution channel:** crates.io only. No Homebrew tap, no Scoop bucket.
  No binary builds, no GitHub Release artifacts.
- **Runner:** `ubuntu-latest` (GitHub-hosted) only. Self-hosted runners are
  no longer required — `release.yml` was replaced by a unified `publish.yml`
  that runs entirely on GitHub infrastructure.
- **Supply chain:** crates.io package signing. No SHA256SUMS, no GPG, no SBOM
  in the publish pipeline (the `sbom.yml` workflow still generates CycloneDX
  SBOMs on push to main and release).

With **only** `CARGO_TOKEN` configured, the full pipeline runs:
CI gates (`ci.yml`) + tests (`test.yml`) + PR gate + crates.io publish
(`publish.yml`) + SBOM + label sync.

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
- **Issue-system ownership identity (Phase 0 / defect #13).** The local `issue`
  tool's `start`/`close` ownership checks use `ToolContext.session_id` as the
  caller identity, and `is_session_alive` checks a per-session `flock` under
  `.oxi/issues/.alive/<session_id>`. For this to actually protect anything,
  the identity must (a) be **non-empty** and (b) **match a flock the process
  holds**. `oxi-cli` enforces both by construction:
  - `bootstrap.rs::build_app` picks the identity — `liveness::TUI_OWNERSHIP_ID`
    ("tui") in TUI mode, `proc-<pid>-<uuid>` otherwise.
  - `App::from_oxi(..., ownership_session_id)` acquires the flock for `App`'s
    lifetime AND sets `AgentConfig.session_id = Some(ownership_session_id)`,
    which `agent.rs` threads into `AgentLoopConfig.session_id` →
    `ToolContext.session_id`.
  - `liveness::TUI_OWNERSHIP_ID` is the single source of truth; the TUI panel's
    `IssuesPanelOverlay::session_id()` references it so the agent tool, the
    panel, and the `/issue` slash command all see one consistent flock holder.
  - **Do NOT** re-introduce a hardcoded `session_id: None` in `agent.rs`'s
    `AgentLoopConfig` construction (that was the #13 bug — it made every agent
    `start` write an empty-string owner that was instantly reclaimable).
    Regression coverage: `session_id_wiring_tests` (oxi-agent) +
    `start_with_distinct_live_owners_collides` (oxi-cli).
- **Issue-system CAS: strict store, recovery in the tool (Phase 2 / #2).** The
  store's `update` is deliberately strict — it returns raw `IssueError::Conflict`
  and **never retries on its own**. Only the `issue` agent tool wraps mutations in
  `cas_retry` (4 attempts: first try uses the agent's `content_hash` as a fast
  path; on conflict it re-reads a fresh hash and retries). Consequence: **a
  stale `content_hash` from an earlier `read` is advisory, not fatal** — the tool
  auto-reconciles. If you add a new mutating store op the agent can call, route
  it through `cas_retry` (see `issue_tool.rs`). Direct `oxi issue` CLI calls do
  **not** retry (CLI is a single-shot caller; re-read manually on conflict).
  `IssuePatch` (absent=keep, `Some([])`=clear labels, `Some`=replace) is the
  precise mutation surface — prefer `apply_patch`/`reopen`/`close` over
  hand-rolled `update` closures, and note `apply_patch` **enforces ownership**
  (different non-empty assignee → `NotAssigned`), matching the legacy policy.
  `update` also does **no-op detection** (skips write/timestamp/invalidate when
  nothing meaningful changed), so don't rely on a no-op `update` bumping
  `updated_at` or the dir mtime.
- The catalog is models.dev-sourced, not hand-written. The embedded snapshot
  `data/catalog/_snapshot.json.gz` is the source of truth (regenerate per
  `oxi-ai/data/catalog/README.md`); per-provider TOML no longer exists.
  oxi-specific provider metadata lives in `data/catalog/product-meta.toml`.
  Set `OXI_MODELS_DEV=off` to test the embedded snapshot alone.
- SSE parsing handles partial UTF-8 lines. Do not assume line boundaries are clean.
- `Agent::is_running` field prevents concurrent agent runs — check this before spawning parallel tasks.
- Port trait methods are **async**. `MutexGuard`s held across `.await` will not compile (`!Send`). Use `tokio::sync::Mutex` or scope the lock.
- The legacy `oxi-store` crate no longer exists. If a new file needs session/settings/auth, put it in `oxi-cli/src/store/` (or in a sibling product's store).
- The `oxi-cli` crate is a **monorepo monolith** by design (~66K lines as of 2026-07-25; was ~17K when this rationale was written — the crate has grown 4× since). Do not split it into more crates unless the 4 separation conditions (independent reuse, independent versioning, build isolation, team boundary) genuinely hold — re-evaluate at ~80K LOC.
- **TUI language policy is TUI-only — by design, not oversight.** `Settings::output_languages` (see `oxi-cli/src/store/settings.rs`) is consumed **exclusively** by `app::agent_session_runtime::build_system_prompt` (the TUI session build path). The `lib.rs` App build path used by `oxi --print` and RPC mode does **not** inject the policy — it has its own simpler `build_system_prompt` that omits it. So a policy written to `settings.toml` is **silently ignored** in non-TUI modes.
  - **Why this asymmetry is intentional:** `oxi --print` and RPC mode are programmatic/scriptable interfaces where language determinism is the caller's responsibility (they control the prompt and can pre-translate or route through any language). The TUI is the conversational surface where this policy earns its place.
  - **Do NOT "fix" the asymmetry** by injecting the directive into `oxi-cli/src/lib.rs::build_system_prompt` without an explicit product decision. If a future caller needs the policy in print/RPC, add an explicit opt-in (CLI flag or extra config field) rather than making it implicit. The single injection point to add later is `oxi-cli/src/lib.rs::build_system_prompt`.
  - **Default is OFF (opt-in).** As of v6, `language_policy_enabled: bool` defaults to `false` even with non-empty `output_languages`. Users must toggle it ON in the `/settings` overlay for the policy to take effect. See `docs/designs/2026-06-17-tui-language-policy.md` for the rationale.
  - **Note:** `/settings` opens the **editable** settings overlay (not a read-only view). Its description and handler now live together on the `SettingsCommand` struct in `tui/slash/builtin/tools_commands.rs` — the slash-command registry removed the old `BUILTIN_SLASH_COMMANDS`-table-vs-`tui/slash.rs`-handler split, so there is nothing to "keep in sync" anymore.
- **TUI language policy is a strong default, NOT a hard guarantee.** The system prompt carries a "MUST" directive and the compaction summarizer sees a "Focus areas:" instruction, but both are prompt-level signals. The model can still occasionally violate the policy when context grows long, when tool output is echoed verbatim, or when the summarizer reframes it as "focus areas" (weaker than the main prompt's MUST). See the docstrings on `Settings::output_languages`, `BuildSystemPromptOptions::language_directive`, `AgentSession::rebuild_system_prompt`, and `build_compaction_instruction` for the full caveat list. To get 100% enforcement you need additional layers (tool output wrapping, response post-processing) — out of scope for the current MVP.
- **Channels are prompt-level directives, not classifiers.** `KNOWN_CHANNELS` (`response`, `code_comment`, `documentation`, `commit_message`) is a label table for the directive string, not a runtime tagger. The model is asked to apply these rules per-output, but it has no mechanism to classify its own output by channel. Boundary cases (commit message quoted inside a response, code-comment-as-explanation, etc.) may be misapplied. See `docs/designs/2026-06-17-tui-language-policy.md` §3.4 for the canonical cases.
- **Theme system: background slots must be consumed by render code, not just defined.** `ColorScheme` (oxi-tui/src/theme.rs) has 28 color slots — 21 original + 7 Phase-1 background slots (`response_bg`, `thinking_bg`, `surface_bg`, `panel_bg`, `diff_add_bg`, `diff_remove_bg`, `diff_hunk_bg`). The 7 new slots AND the 3 previously-dead slots (`user_bg`, `code_bg`, `selection_bg`) are now **wired into render code** as of the theme redesign (2026-06-24). If you add a new `ColorScheme` field, you MUST also:
  - Add it to `ThemeStyles` + `to_styles()` (pack as `Style::default().bg(color)` or `.fg(color)`).
  - Add it to `ThemeFileColors` + `into_theme()` resolve.
  - Populate it in all 6 `ColorScheme::*()` constructors (`dark`, `light`, `nord`, `catppuccin`, `github_dark`, `monokai`).
  - **Consume it in the render code** — a defined-but-unconsumed slot is a dead field that makes the theme feel unchangeable. Use `buf.set_style(rect, self.styles.<field>)` for area fills or `Style::patch()` for per-Span composition.
  - The brightness hierarchy is `background ≤ response_bg < thinking_bg < surface_bg < user_bg < panel_bg` — new slots must respect this ordering. See `docs/THEME_GUIDE.md` for the derivation rules.
  - **`DashboardWidget` takes `&Theme`** (not `Theme::dark()`). The MCP dashboard overlay constructs it fresh in `render()` with the live theme. Do not re-introduce a hardcoded `Theme::dark()` in any widget — pass the theme through.
  - **`OxiStyleSheet` is theme-aware** — constructed via `OxiStyleSheet::from_styles(&ThemeStyles)`. Do not revert to the old zero-sized unit struct with hardcoded RGB values.

## oxi-tui v2 — RETIRED; tape cutover — LIVE (2026-07-29)

The grok-inspired `oxi-tui` v2 crate (terminal-first pipeline with
`RetainedTree`, `draw_frame_closure`, cell-level `DiffBackend`) has been
retired and deleted. The production TUI now renders chat transcripts on the
main screen through `tape::TapeEngine` with memoized transcript components.
Alternate screen is entered only for transient overlays. See
`docs/superpowers/specs/2026-07-29-p2-tui-tape-model-design.md` and
`docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`.
