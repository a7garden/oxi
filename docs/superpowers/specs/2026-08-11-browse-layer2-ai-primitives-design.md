# Browse Layer-2 AI Primitive: `browse_act` — Design Spec (v2)

**Date:** 2026-08-11
**Author:** oxicode agent (autonomous delegation by a7garden)
**Status:** Approved for implementation (revised after self-review)
**Target version:** 0.74.0 (post-0.73.0)
**Scope:** One new agent tool — `browse_act` — that closes the layer-2
gap exposed by the v0.73.0 browsing stack. Plus CI fix-up that the
v0.72.0 native-browser-default migration broke.

> **What changed from v1 of this spec:** v1 proposed two tools
> (`browse_act` + `browse_extract_struct`) and grounded `act` with a
> deterministic Jaccard token-overlap scorer. Review caught that a
> keyword matcher does not meet the user's stated goal — competing
> "browsing-equipped agents" (browser-use, Stagehand, Playwright MCP,
> Skyvern, AgentQL, Claude CU, OpenAI CUA) all ground via a **model**
> over the page's interactive surface. v2 makes the LLM the actual
> grounding step. `browse_extract_struct` is honestly deferred to a
> later PR (it needs LLM-driven schema-intent mapping, which is its own
> design problem) — shipping CSS-per-field as "AI primitive" would
> repeat v1's mistake.

## 1. Background — the gap

v0.73.0 shipped a strong L1 driver (`browse_session` with 30 actions, the
`BrowserTab` trait, `Observation` snapshots, `print_to_pdf`) and a working
L3 agent loop. The P1 gap is the middle layer: AI-driven element
grounding.

The calling model currently has to:

1. Read raw HTML / accessibility surface from `browse` or `browse_extract`,
2. Reason about which CSS selector matches the user's intent,
3. Pass that selector to a follow-up `click` / `fill` / `select_option`.

Step 2 is the gap. Competitors resolve it differently:

| Competitor | Grounding approach |
|---|---|
| browser-use | LLM over accessibility tree (`act(goal)`) |
| Stagehand | LLM over accessibility tree (`act` / `extract`) |
| Playwright MCP | selector synthesis + role/aria/name queries (LLM in the host loop) |
| Skyvern | DOM + vision model fusion |
| AgentQL | declarative schema → semantic DOM (LLM compiler) |
| Claude CU / OpenAI CUA | computer-use (pixel coordinates via vision) |

The common thread: **a model is in the loop, looking at the page's
interactive surface, picking the element.** This spec puts a model into
`browse_act` — the LLM is construction-injected into the tool at factory
time and called synchronously per act.

## 2. Goals

1. **`browse_act`** — given a natural-language `goal` (e.g. "click the
   blue Sign Up button"), open the page, capture its `Observation`,
   call a construction-injected LLM with `{goal, Observation}` plus a
   small candidate tier from a deterministic scorer, and dispatch the
   matched `BrowserTab` action. The **model decides** which element
   matches — the agent calling `browse_act` writes no CSS selectors.

## 3. Non-goals

- **No new browser engine.** `browse_act` consumes the existing
  `BrowserEngine` / `BrowserTab` / `Observation` types from
  `oxicode-agent/src/tools/browse/engine.rs`. The native `oxibrowser`
  backend is the source of `Observation`; the existing JS walk in
  `oxibrowser_backend.rs` continues to be best-effort (same caveat
  documented in `engine.rs`).
- **No new SDK port.** `browse_act` is an agent-layer tool (`AgentTool`
  impl), parallel to the existing `BrowseTool` / `BrowseExtractTool` /
  `BrowseSessionTool` / `BrowseScriptTool`. It never enters the SDK port
  contract.
- **No ToolContext change.** The tool receives the provider at
  construction time (injected via `BrowseActTool::new(provider, model,
  engine, config)`). ToolContext stays unchanged — agents that don't
  use `browse_act` are unaffected.
- **No coordinate-based click.** Coordinates are explicitly rejected
  (`engine.rs` documents that boa layout only approximates geometry).
  Refs are the only grounding currency.
- **`browse_extract_struct` is deferred.** A "schema-driven extract"
  primitive without LLM grounding is just CSS-per-field with extra
  ceremony; with LLM grounding it's its own design problem (intent →
  selector synthesis, schema → field mapping). Defer to a follow-up
  PR. This spec is honest about that scope.
- **No vision grounding.** Coordinate-based vision (Playwright-style
  screenshot → click(x,y)) is a separate primitive. Out of scope here;
  see §10.

## 4. Architecture

### 4.1 Construction-injected LLM (the key change from v1)

The tool receives `(Arc<dyn oxicode_ai::Provider>, oxicode_ai::Model)`
at construction. This is the same shape `Agent::new` already accepts
(`oxicode-agent/src/agent.rs`, `oxicode-sdk/src/agent_builder.rs:484-485`),
so the precedent is unambiguous — no new abstraction needed.

```rust
pub struct BrowseActTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    /// Reasoning capability — used to ground `goal` against `Observation`.
    provider: Arc<dyn oxicode_ai::Provider>,
    model: oxicode_ai::Model,
    callbacks: super::callback_mixin::BrowseCallbacks,
    tab_id_slot: Mutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}

impl BrowseActTool {
    pub fn new(
        provider: Arc<dyn oxicode_ai::Provider>,
        model: oxicode_ai::Model,
        engine: Arc<dyn BrowserEngine>,
    ) -> Self { ... }

    pub fn with_config(
        provider: Arc<dyn oxicode_ai::Provider>,
        model: oxicode_ai::Model,
        engine: Arc<dyn BrowserEngine>,
        config: BrowseConfig,
    ) -> Self { ... }
}
```

The factory wires `(provider, model)` from the existing Oxicode engine
(`oxicode-sdk::Oxicode::create_provider`). `oxicode-cli` already has
this provider in hand at composition time (`bootstrap.rs:415-431`),
so wiring `browse_act` is a straight pass-through of an existing
`Arc<dyn Provider>` + `Model`.

### 4.2 Grounding pipeline

```
caller ──> browse_act {url, goal, value?, action_hint?, timeout?}
              │
              ▼
       open tab, goto url, wait_for_condition(Load)
              │
              ▼
       tab.observe() → Observation
              │
              ▼
       candidate_tier(goal, observation)     ◀── deterministic Jaccard
              │                                  (fast pre-filter, top-N=20)
              ▼
       llm_ground(goal, candidates, observation)
              │                              ◀── LLM picks ref_id+action
              ▼                              (uses oxicode_ai::Provider::stream)
       ranked match + action
              │
              ▼
       dispatch_action(tab, selector, action, value)
              │
              ▼
       AgentToolResult { matched_ref, matched_name, action, result }
              │
              ▼
       close tab
```

#### 4.2.1 Deterministic candidate tier (Jaccard)

The scorer from v1 of this spec is retained — but **only as a
candidate-generator**. It produces a top-N list (default N=20) of
interactive visible elements ordered by score. The LLM then picks the
right one (or rejects all candidates and returns `no_match`).

This tier:

- Saves tokens: with N=20 we send ≤20 elements instead of e.g. 200
  on a busy page.
- Provides a deterministic fallback when the LLM is unavailable
  (offline build, mocked provider in tests): the tool can still
  produce a *best-effort* answer by returning the top scorer. We
  surface this in metadata as `mode: "llm" | "deterministic_fallback"`.

#### 4.2.2 LLM grounding (the actual layer-2 step)

```rust
async fn llm_ground(
    provider: &dyn oxicode_ai::Provider,
    model: &oxicode_ai::Model,
    goal: &str,
    candidates: &[ObservedElement],
    observation: &Observation,
    action_hint: Option<BrowserActAction>,
) -> Result<LLMGroundResult, BrowserError>
```

The LLM is called with a single user message:

```
GOAL: <the user's natural-language instruction>
URL: <observation.url>
TITLE: <observation.title>

CANDIDATE ELEMENTS (top-N from deterministic tier):
[{"ref_id": "e1", "role": "button", "name": "Sign Up", "tag": "button"},
 {"ref_id": "e2", "role": "link", "name": "Documentation", "tag": "a"},
 ...]

TASK: pick the single element that best matches the goal.
Return JSON: {"ref_id": "eN", "action": "click|type|fill|select_option|check|uncheck|press|hover", "value": "..." (only for type/fill/select_option/press), "reason": "..."}

If none of the candidates match the goal, return: {"ref_id": null, "reason": "..."}
```

The tool deserializes the JSON response. No tool-calling loop, no
multi-turn — just one streaming completion that we await to a single
text delta. `oxicode_ai::Provider::stream` is the entry point (already
used 100+ places across `oxicode-agent`).

### 4.3 `BrowserError` variants

Three new variants — same as v1, kept for the LLM path:

```rust
pub enum BrowserError {
    // ... existing ...
    /// The LLM could not pick a confident match.
    #[error("no match: {0}")]
    NoMatch(String),  // LLM's reason
    /// The LLM picked an action that requires `value` but provided none.
    #[error("missing value for action: {action}")]
    MissingValue { action: &'static str },
    /// LLM response couldn't be parsed as the expected JSON shape.
    #[error("grounding parse failed: {0}")]
    GroundingParse(String),
}
```

### 4.4 Tool input (unchanged from v1)

```json
{
  "url": "string (required) — page to act on; opens a fresh tab",
  "goal": "string (required) — natural-language action description",
  "value": "string (optional) — value to type/fill/select/press",
  "action_hint": "string (optional) — narrows the LLM's action choice",
  "timeout": "integer (optional, default 30) — seconds for the whole
              pipeline; the LLM call has its own internal budget via
              StreamOptions"
}
```

### 4.5 Tool output

```json
{
  "matched_ref": "eN" | null,
  "matched_name": "Sign Up" | null,
  "matched_role": "button" | null,
  "action": "click" | "..." | null,
  "selector": "[data-oxicode-ref=\"eN\"]" | null,
  "score": 0.83,
  "result": "ok" | "no_match" | "missing_value",
  "mode": "llm" | "deterministic_fallback",
  "reason": "the LLM's free-text explanation, if any",
  "candidates_considered": 20
}
```

## 5. Error handling

The tool surfaces failures as `ToolError` (existing `From<BrowserError>`
impl in `engine.rs:42-46`). Distinct failure paths:

- `LLM returns no ref_id` → `BrowserError::NoMatch(reason)` + the tool
  result carries `matched_ref: null` and the LLM's reason. The calling
  model can refine the `goal` and retry.
- `LLM returns action that needs value but value is empty` →
  `BrowserError::MissingValue`.
- `LLM response isn't parseable JSON` → `BrowserError::GroundingParse`.
- **Provider error** (network / auth / model-not-found) →
  `BrowserError::Backend(provider_error_string)`. Falls through to the
  deterministic tier automatically; `mode` reflects the fallback.
- **Tab navigation fails** → `BrowserError::Navigation`.

## 6. Testing (TDD, red-green-refactor)

Per the project's testing policy, with a MockProvider pattern that
already exists at `oxicode-agent/src/advisor/agent_advisor.rs:109-131`
(`NopProvider`) and `oxicode-sdk/src/lifecycle/supervisor.rs:813-831`
(`MockProvider`).

- `BrowseActTool` unit tests (TDD):
  - **candidate tier** (deterministic):
    - token-overlap scoring (mock `Observation`)
    - tie-breaking (button > link > textbox)
    - filters hidden / non-interactive
    - returns top-N (default 20, configurable)
  - **LLM grounding path** (MockProvider):
    - MockProvider that returns a valid `{"ref_id": "e2", ...}` JSON →
      assert `BrowseActTool::execute` selects e2 and dispatches `click`.
    - MockProvider that returns `{"ref_id": null}` →
      assert `BrowserError::NoMatch` surfaces; `result.mode == "llm"`.
    - MockProvider that returns invalid JSON →
      assert `BrowserError::GroundingParse`.
    - MockProvider that errors →
      assert tool falls back to deterministic tier; `result.mode ==
      "deterministic_fallback"`.
  - **action dispatch**:
    - LLM returns `{"ref_id": "e2", "action": "type", "value": "hi"}` →
      assert `tab.type_(selector, "hi")` was called.
    - LLM returns `{"ref_id": "e2", "action": "type"}` with empty value →
      assert `BrowserError::MissingValue`.
    - LLM returns `{"ref_id": "e2", "action": "press", "value": "Enter"}` →
      assert `tab.press("Enter")` was called.
  - **factory wiring**:
    - `browsing_tools` / `browsing_tools_with_session` register
      `BrowseActTool` (with `(provider, model)` from caller).
- Integration: existing factory tests still pass; the new
  `BrowseActTool` is reachable via `native-browser` factory.

## 7. Documentation

- Doc comments on every public type / fn in the new file (no
  `#[allow(missing_docs)]`).
- `CHANGELOG.md` `[Unreleased]`: `### Added` block for `browse_act`
  with the LLM-grounded contract, plus an explicit `### Deferred` note
  that `browse_extract_struct` (intent-based structured extraction)
  needs its own design pass and is on the P4 roadmap.
- AGENTS.md / docs: none needed — the tool is agent-layer, not SDK
  contract (per the v0.72.0 browsing-identity decision).

## 8. Rollout / risks

- **Risk:** Every `browse_act` call costs one LLM completion. On a
  page where the agent already iterated several times, this adds up.
  **Mitigation:** the deterministic candidate tier prunes the
  observation to ≤20 elements before the LLM call. The prompt is
  fixed-size (one user message); total tokens ~1k-2k input, ~50
  output. Acceptable for an act that would otherwise need a
  full-page read + multi-step reasoning.
- **Risk:** LLM picks the wrong ref. **Mitigation:** the tool
  surfaces `matched_ref`, `matched_name`, `matched_role`, `reason` in
  its result so the calling model can detect a wrong pick and retry
  with a sharper goal.
- **Risk:** `Observation` JS walk is best-effort (documented in
  `engine.rs`). The LLM only sees what `observe()` returns.
  **Mitigation:** the `Observation` best-effort caveat is preserved
  unchanged. The `browse_act` tool description tells the agent to
  fall back to `browse` + `browse_extract` when `Observation` is
  sparse.
- **Risk:** Provider may not be wired in some test harnesses.
  **Mitigation:** tests use `MockProvider`; the tool's deterministic
  fallback path covers offline / provider-missing scenarios and is
  surfaced in `mode` so it's never silent.
- **Risk:** Two tools (`browse_act` + existing `browse`) overlap in
  capability. **Mitigation:** the system prompt and tool description
  draw a clear line: `browse` = page content + selectors; `browse_act`
  = grounded action by intent. The agent can choose either; `browse_act`
  is the right default for "click that button" and similar.

## 9. Acceptance criteria

### 9.1 `browse_act`

- `cargo fmt --all -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo clippy -p oxicode-cli -- -D warnings` clean (the CI
  native-browser clippy job's filter).
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`
  passes.
- `cargo nextest run --workspace -E 'not (test(slow) | test(/^net_/) |
  test(requires_network) | test(/^bench_/))'` passes.
- The new tool is reachable from `browsing_tools_with_session` (rides
  the same `native-browser` feature gate as `BrowseScriptTool`).
- `browsing_tools` / `browsing_tools_with_config` also expose it (the
  tool is always-compiled, like `BrowseExtractTool`).
- At least 12 new unit tests in `browse_act_tool.rs` (candidate tier +
  LLM path + action dispatch + factory wiring), all passing under the
  workspace smoke filter.

### 9.2 CI restore (v0.72.0 native-browser-default migration cleanup)

v0.72.0 promoted `native-browser` to a default feature of `oxicode-cli`,
which transitively pulls in `oxibrowser-render` (Blitz/Stylo/Taffy/vello)
and the `yeslogic-fontconfig-sys` build script. The system dep
`libfontconfig1-dev` was added to the two clippy jobs at v0.73.0
(`ci.yml:43-44`, `64-65`), but the **smoke-test** job (`ci.yml:99`) and
the **msrv** job (`ci.yml:149`) still install only `libssl-dev`. Both
run `cargo build` / `cargo test --no-run` over the workspace, which
compiles and links oxibrowser-render; without fontconfig on the runner,
`yeslogic-fontconfig-sys`'s build script panics with `pkg-config could
not find fontconfig`. This has been broken since v0.71.0 → v0.72.0 and
stays red across v0.72.0 → v0.73.0.

**Fix:** add `libfontconfig1-dev` to the apt-get install line in both
jobs. Single line change per job, parallel to the existing clippy jobs.

- **Evidence (read from CI run 31467470837, not speculated):**
  - `Smoke Test (PR only)` job, exit 101, panicked in
    `yeslogic-fontconfig-sys v6.0.1` build script: `Package fontconfig
    was not found in the pkg-config search path`.
  - `MSRV (1.96)` job, exit 1, identical error in the same build script.
- **Cargo doc** job also failed in the same run with two broken
  intra-doc links — fixed in this PR by qualifying the references as
  `super::engine::BrowseWaitCondition` / `super::engine::Observation`.

**Acceptance for CI fix:**

- `ci.yml` smoke-test job's apt-get installs `libfontconfig1-dev`.
- `ci.yml` msrv job's apt-get installs `libfontconfig1-dev`.
- Local `cargo nextest run --workspace` smoke subset still passes
  (regression guard — the change is CI-only).

## 10. Out of scope (deferred)

- **`browse_extract_struct` (intent-based structured extraction)** —
  the LLM-in-the-loop cousin to `browse_act` for *extracting*
  structured data from a page given a schema intent. Needs its own
  design pass (intent → selector synthesis, schema → field mapping,
  list-row scoping). Deferred to a follow-up PR.
- **Vision-grounded clicking** (Playwright-style screenshot → click(x,y)
  coordinates) — needs a vision-capable model in the tool's reach AND
  `BrowserTab` coordinates support. Deferred.
- **Self-healing selectors** (re-resolve on stale ref) — needs a notion
  of selector confidence + retry budget. Deferred.
- **Multi-step planning** ("click login, then type email, then type pw")
  — the model composes via repeated tool calls today; this is a planner
  concern, not a primitive concern.
- **Cross-tab grounding** (act on tab B using tab A's observation) —
  Deferred.
