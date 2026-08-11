# Browsing Identity — Design Spec

> Direction **B**: oxibrowser is a standalone general-purpose headless browser.
> Browsing is a **product capability**, not an SDK contract. The SDK stops
> shipping browser tooling; products that want browsing consume `oxibrowser`
> (and, in our case, the agent-layer browse tools) directly.

## Context & Decision

A competitive survey (browser-use, Stagehand, Playwright MCP, Skyvern,
AgentQL, Claude CU, OpenAI CUA — full report in session) established that:

- Every leading AI browsing agent stacks **3 layers**: (1) low-level
  deterministic driver (Playwright-style), (2) high-level AI primitives
  (`act`/`extract`/`observe`), (3) agent loop.
- oxicode already has layers (1) and (3) at best-in-class quality
  (`browse_session`: 30 actions, persistent tab; pure-Rust `oxibrowser`,
  no Chrome/Playwright dependency — unique among all surveyed).
- Layer (2) (`act`/`extract`) is the real competitive gap — deferred to P1.

The decision in this spec is **not** about building layer (2). It is an
**ownership + identity** decision: where browsing lives in the architecture,
and whether it is on by default.

### Ownership decision (the core of this spec)

`oxicode-sdk`'s stated philosophy (AGENTS.md) is *"SDK is the contract, not
the implementation — defines port traits; products write their own
domain-specific impls."* Browser tooling currently violates this: the SDK
re-exports concrete tool impls (`BrowseTool`, `BrowseSessionTool`), a concrete
backend (`OxicodeBrowserEngine`), and assembly factories
(`browsing_tools_with_session`, `native_browser_tools`, `full_tools`,
`browsing_tools`). That is "implementation," not "contract."

**Decision:** remove browser tooling from the SDK. `oxibrowser` is already an
independent general-purpose crate on crates.io (v0.20). Consumers (oxios, etc.)
that need browsing depend on `oxibrowser` directly and build — or reuse —
their own thin agent-tool wrappers. oxicode's own wrappers stay where the
tools already live: `oxicode-agent`.

This restores the contract-not-implementation principle, keeps the SDK light,
and gives each product policy autonomy over browsing (action surface, domain
allow-lists, screenshot defaults).

## P0 Scope (this spec)

Four changes, ordered so each is independently committable:

### 1. Move browse factories SDK → oxicode-agent

The factories are pure assembly over types that already live in
`oxicode-agent` (`BrowseTool`, `BrowseExtractTool`, `BrowseScriptTool`,
`BrowseSessionTool`, `BrowserEngine`, `ToolRegistry`). They are not contract.

**Move** (from `oxicode-sdk/src/tool_factory.rs` → a new
`oxicode-agent/src/tools/browse/factory.rs`, re-exported from
`oxicode-agent/src/tools/browse/mod.rs`):
- `browsing_tools(engine)` → `Arc<ToolRegistry>`
- `browsing_tools_with_config(engine, config)`
- `browsing_tools_with_session(engine)` (gated `native-browser`)

**Ripple — SDK `native-browser` feature removal:** the SDK feature currently
does `["oxicode-agent/native-browser", "dep:oxibrowser-core"]`. Once the
browser re-exports are gone, enabling it pulls `oxibrowser-core` into a crate
that no longer uses it — a dead feature. Remove it, and fix two dependents:

- **CLI feature def** (`oxicode-cli/Cargo.toml:125`): change
  `native-browser = ["oxicode-agent/native-browser", "oxicode-sdk/native-browser"]`
  → `native-browser = ["oxicode-agent/native-browser"]` (drop the SDK side).
- **CI `clippy-native-browser` job** (`.github/workflows/ci.yml`) runs
  `cargo clippy -p oxicode-sdk --features native-browser` — that feature no
  longer exists. Since the CLI now defaults to `native-browser`, the regular
  `clippy --workspace` job already compiles the Blitz stack. Repurpose the
  job to `cargo clippy -p oxicode-cli` (default features = native-browser on)
  so the edition-2024 lifetime check the job exists for still runs.
- **AGENTS.md** "native-browser feature must always compile" verification
  commands: update the `cargo clippy -p oxicode-sdk --features native-browser`
  example to the new CLI target.
- `native_browser_tools()` / `native_browser_tools_with_config(config)`
  (gated `native-browser`)

`full_tools(cwd, engine)` mixes coding + browser tools. Decision: **leave a
browser-free `full_tools`** is out of scope; instead remove the browser
portion and keep `coding_tools`/`readonly_tools` where they are. `full_tools`
moves to agent too (it is pure agent-layer assembly) **or** is dropped if
unused. (Acceptance: no external caller breaks; CLI does not use it.)

### 2. Remove browser surface from oxicode-sdk

- `lib.rs:359-378` — delete the `#[cfg(feature="browser")]` and
  `#[cfg(feature="native-browser")]` re-export blocks (BrowseTool,
  BrowserEngine, BrowseSessionTool, OxicodeBrowserEngine, BrowserEvent, etc.).
- `agent_builder.rs:403-435` — delete `.browsing()`,
  `.browsing_with_config()`, `.native_browser()`.
- `tool_factory.rs` — delete the six browser factory fns (after move).
- `prelude.rs:12-15,22-25` — delete browser re-exports.
- `Cargo.toml` — remove `native-browser` feature, remove `"browser"` from the
  `unstable` umbrella list, remove the `"browser" = []` line, drop the
  `oxibrowser-core` optional dependency.
- **Keep** `Capability::WebBrowse` (capability/mod.rs), the security
  middleware's `"browse"|"browse_extract"` name check (middleware.rs), and
  `CapabilitySet::browser` (authorizer.rs). These are capability *contract*,
  string/name-based, and do not reference browser types — they survive and
  remain meaningful for products that register their own browser tools.

**CLI update** (`bootstrap.rs:272-277`): change
`oxicode_sdk::tool_factory::browsing_tools_with_session(engine)` →
`oxicode_agent::tools::browse::browsing_tools_with_session(engine)`.

### 3. `native-browser` becomes a CLI default feature

`oxicode-cli/Cargo.toml`:
```toml
default = ["self-update", "native-browser"]
```
Effect: `cargo install oxicode` ships browse tools out of the box —
"browsing-equipped AI agent" identity in every build. README documents
`--no-default-features --features self-update` for the lightweight build.

Note: this does **not** touch the SDK — the SDK is now browser-free (change
#2). Only the CLI opts into native-browser.

### 4. `read` gains a lightweight HTTP reader-mode path

Currently `read` rejects `http://`/`https://` (the internal-URL resolver
explicitly returns `None` for web schemes — `url_router.rs:77`). So
`web_search → read` is broken: the agent finds a URL but cannot read its body
without `browse`.

**Insert** an HTTP branch in `read.rs::execute`, between the internal-URL
dispatch (line 362) and the `PathGuard` filesystem validation (line 364):
- Detect `http://` / `https://` prefix on `path_str`.
- `reqwest::get` (already a dep) → response bytes → content-type check
  (text/html, application/json, text/plain, application/xml).
- HTML → reader-mode markdown via a maintained lightweight crate (no JS
  rendering, no `oxibrowser-core`). Strip `<script>/<style>/<nav>/<footer>`,
  extract main content, convert to markdown. JSON/text returned as-is.
- Hard caps: response size limit (e.g. 2 MiB), redirect cap, timeout (15 s).
- Return `AgentToolResult::success(markdown)` with url/title metadata.

**Role split (explicit):** `read http://…` = fast static body (no JS, all
builds); `browse url` = full browser (JS/SPA/screenshots/vision). The system
prompt (change below) teaches the agent this split.

Dependency choice is resolved during implementation; candidate crates to
evaluate (pick one, prefer maintained + lightweight + no browser-engine
transitive deps): `htmd`, `html2md`, `readability`+manual. If none is
satisfactory, fall back to a small hand-rolled tag-stripper over the bytes
(the agent already has `reqwest`; no other new dep needed).

### 5. System prompt: web research route guidance

Add a short guideline to the system prompt (in
`oxicode-cli/src/prompt/system_prompt.rs` `default_tool_snippets`/guidelines)
describing the 3-step research route:
`web_search` (discover) → `read <url>` (static body, fast) → `browse <url>`
(dynamic/JS/screenshots/vision). Plus a note that `browse` screenshots feed
the vision-routing tier swap.

## Explicit non-goals (deferred)

- Layer-2 AI primitives: `act` (NL→ref grounding), `extract` (schema→struct),
  `observe` bbox/coordinates. → P1.
- Multi-tab `browse_session`, storage-state save/restore, network
  mocking/console/dialog. → P2.
- `browse_session` as an MCP server. → P3.
- PDF export (upstream `Page.printToPDF` is CDP-only).

## Verification

- `cargo build --workspace` (default features — now includes native-browser on
  CLI).
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy -p oxicode-sdk --features native-browser -- -D warnings` —
  must still pass (SDK no longer has native-browser feature; verify the gate
  job adapts, or that this command degrades gracefully).
- `cargo nextest run --workspace`
- New tests: read HTTP path (mock server → reader-mode output);
  factory-moved smoke (browsing_tools_with_session builds browse+session).
- `cargo package -p oxicode-sdk --allow-dirty` succeeds (no dangling
  references to removed symbols).

## Breaking change / CHANGELOG

Public-symbol removal from `oxicode-sdk` (the browser re-exports, factories,
`.browsing()` builder, `browser`/`native-browser` features). Low blast radius
(gated behind `unstable` + `default=[]`; oxios marked 🔜 not consuming), but
per the ownership contract this gets a CHANGELOG `### Changed` **Breaking**
entry under `[Unreleased]`.
