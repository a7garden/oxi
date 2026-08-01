# oxi-sdk Stability & Ownership Program (R0–R8)

> **Date:** 2026-08-01
> **Status:** Approved — spec for implementation planning
> **Source:** `oxios/docs/production-audit/2026-08-01-ideal-oxi-sdk-proposal.html`
> **Baseline:** oxi 0.63.0
> **Scope:** Full program — all 9 requests (R0–R8)

---

## 1. Motivation

The oxios sister repo experienced the **P0.2 incident**: oxi-sdk 0.61 silently
removed 7 root-level public symbols (`ProviderPool`, `RateLimitPolicy`,
`CircuitBreakerConfig`, `ProviderCircuitBreaker`, `MultiProviderBuilder`,
`RoutingConfig`, `MultiProviderConfig`) with zero `#[deprecated]` warnings, zero
CHANGELOG `## Removed` entries, and zero stability-tier annotations. oxios's
build broke because it had built core resilience on symbols it didn't know were
opt-in.

Two structural defects combined to cause this:

- **Defect A — ownership ambiguity.** oxios implemented its own MCP client,
  memory backend, and circuit breaker because the SDK had equivalent features
  that didn't match oxios's domain policies. Neither side knew what the other
  owned.
- **Defect B — governance absence.** 166 CHANGELOG sections (0.56–0.63) had
  zero `## Removed`/`## Breaking` entries. `#[deprecated]` count: 0. Stability
  tier annotations: 0.

This program addresses both axes simultaneously: ownership boundaries (R0/R5/R6)
and governance machinery (R1/R2/R3/R7/R8), plus internal robustness (R4).

---

## 2. Program Structure — 4 Phases

```
Phase 1 — Ownership Contract & Governance Docs   (R0, R5, R1, R8)
Phase 2 — Stability Annotations                   (R3 proc-macro, R2 deprecated)
Phase 3 — Composable Traits                       (R6)
Phase 4 — Robustness                              (R4 zero-panic, R7 error stability)
```

Phase 1 gates everything: the tier annotations, trait extraction, and error
policy all reference "who owns what." Phases 2–4 can overlap once the ownership
contract is written.

---

## 3. Phase 1 — Ownership Contract & Governance Docs

### 3.1 R0/R5 — Ownership Contract

**Deliverable:** `docs/oxi-sdk-ownership.md`

The contract defines a **behavior ↔ policy split**: the SDK owns behavior
(interfaces + reference implementations); consumers own policy (domain-specific
thresholds, validation, tiering).

| SDK owns (behavior + reference impl) | Consumer owns (policy) |
|---|---|
| Agent loop (`Agent::run_streaming`) | Domain-specific tools |
| Provider transport (Anthropic, OpenAI, …) | Channels (Web, CLI, Telegram) |
| Tool-calling protocol | Process scheduling / job queues |
| Default retry (3×, exponential backoff) | A2A / inter-agent messaging protocol |
| Credential store (`AuthProvider` port) | Knowledge base (markdown/vector) |
| Catalog (models.dev sync) | RBAC / path sandbox |
| Token usage tracking | Memory compression (`Snapcompact`) |
| MCP transport + message format | MCP spawn validation policy |
| `MemoryStore` port trait | Memory tiering policy (Hot/Warm/Cold) |
| `CircuitBreaker` behavior trait + `DefaultCircuitBreaker` | A2A/domain traffic thresholds |

**Principle:** the SDK owns *how* a capability behaves (the trait + a reference
impl); the consumer owns *what thresholds/validation* apply to their domain.
Example:

- SDK: `pub trait CircuitBreaker { fn check(); fn record_success(); fn record_failure(); }` + `DefaultCircuitBreaker` reference impl.
- Consumer: implements `CircuitBreaker` for its domain breaker (`A2ACircuitBreaker`) with its own thresholds.
- SDK reference impl changes don't break consumer trait impls.

**Publication (R5):** the ownership contract is linked from:
- `oxi-sdk/README.md` (top-level "Ownership Contract" section)
- `AGENTS.md` Port System section (cross-reference)

**Acceptance criteria:**
- [ ] `docs/oxi-sdk-ownership.md` exists with the two-column table above.
- [ ] `oxi-sdk/README.md` links to it.
- [ ] `AGENTS.md` cross-references it.

### 3.2 R1 — CHANGELOG `## Removed` / `## Breaking` enforcement

**Convention** (codified in `docs/release-process.md`):

Any root-level `pub` symbol removal, signature change, or semantic change MUST
appear under `## Breaking` (0.x semver) with:
1. Full symbol path (e.g. `oxi_sdk::ProviderCircuitBreaker`)
2. Replacement API or migration path
3. Minimum deprecation window (N releases before removal)
4. Known affected consumers

**CI gate:** a `cargo-public-api` diff job in CI that compares the public API
surface between `main` and the PR branch. If symbols disappeared, the job fails
unless a `## Breaking`/`## Removed` CHANGELOG entry covers them.
`cargo-public-api` is an established tool (builds docs, diffs the item list) —
preferred over a hand-rolled xtask for robustness.

**Retrospective:** back-fill the CHANGELOG with a `## Breaking` entry for the
0.61 removals (circuit breaker, provider pool, multi-provider) — this
acknowledges the P0.2 breakage retroactively.

**Acceptance criteria:**
- [ ] CI gate (`cargo-public-api` diff) runs on PRs and fails on undocumented removals.
- [ ] CHANGELOG has a retrospective `## Breaking` entry for the 0.61 removals.

### 3.3 R8 — Dependency hygiene

**Current state:** `oxi-ai/Cargo.toml:65-74` pulls `prost` + `prost-build` +
`protoc-bin-vendored` for the **Devin + Cursor protobuf providers**. These are
heavy build dependencies (~120 transitive crates, significant cold-build time).

**Policy** (in `release-process.md`): adding a heavy build dependency requires:
1. A CHANGELOG `## Changed` entry noting build impact (e.g. `+~120 crates, +~150s cold build time`).
2. Consideration of feature-gating for consumers who don't need it.

**Action — feature-gate protobuf providers:**

```toml
# oxi-ai/Cargo.toml
[features]
protobuf = ["dep:prost", "dep:prost-build", "dep:protoc-bin-vendored"]
```

The Devin + Cursor provider modules move behind `#[cfg(feature = "protobuf")]`.
Default features exclude `protobuf`; consumers who use those providers enable it
explicitly.

**Acceptance criteria:**
- [ ] `release-process.md` documents the heavy-dep policy.
- [ ] `oxi-ai` has a `protobuf` feature (off by default) gating `prost`/`prost-build`/`protoc-bin-vendored`.
- [ ] Devin/Cursor providers compile only with `--features protobuf`.
- [ ] `cargo build -p oxi-ai` (default features) does not pull prost.

---

## 4. Phase 2 — Stability Annotations

### 4.1 R3 — New proc-macro crate: `oxi-api-stability`

**Deliverable:** new workspace crate `oxi-api-stability/` (`proc-macro = true`).

Leaf crate — depends only on `proc-macro2`, `syn`, `quote`. No internal `oxi-*`
deps. Consumed by `oxi-ai`, `oxi-agent`, `oxi-sdk`.

Provides four attribute macros:

```rust
#[stable(since = "0.63.0")]
// Renders a green doc badge in cargo doc. Semver-stable: cannot be removed
// without a deprecation window of ≥1 minor release.

#[unstable(feature = "browser")]
// Renders an amber badge. May change at any time. Consumers opt in knowingly.

#[deprecated(since = "0.64.0", note = "use X instead; removed in 0.66.0")]
// Renders a red badge + emits the native #[deprecated] attribute.

#[internal]
// Emits #[doc(hidden)]. The symbol is pub for architectural reasons but not
// intended for consumer use.
```

**Expansion:** each attribute expands to `#[doc(alias = "...")]` /
`#[doc = "..."]` attributes that render as colored stability badges in
`cargo doc` output. The `#[deprecated]` variant additionally emits the native
`#[deprecated]` attribute so `cargo build` produces warnings.

**Consumer opt-in — two layers:**

1. **Doc visibility (proc-macro):** `#[unstable]` and `#[internal]` render
   visible badges in `cargo doc`, so consumers see the tier when browsing docs.
2. **Feature-gating (cargo feature):** the most sensitive unstable items are
   additionally placed behind an `unstable` cargo feature on `oxi-sdk`:
   ```rust
   // oxi-sdk/src/lib.rs
   #[cfg(feature = "unstable")]
   pub mod unstable_api {
       pub use oxi_agent::advisor::*;
       // … items that may change or disappear
   }
   ```
   Consumers access them via `oxi-sdk = { features = ["unstable"] }` — an
   explicit, machine-checked opt-in (the default build doesn't even compile
   those symbols into the consumer's dependency).

Compile-time *warnings* on unstable usage would require nightly `-Zunstable-
options` or a custom lint pass, which is out of scope. The two stable-Rust
layers above provide machine-readable opt-in (feature gate) + human-readable
signal (doc badge) without nightly.

**Annotation rollout:**
1. Annotate `oxi-sdk/src/lib.rs` root re-exports first (~50 symbols — the oxios-facing surface).
2. Expand to `oxi-ai` and `oxi-agent` public API.
3. Every `pub` item gets exactly one tier.

**Tier assignment guidance:**
- `#[stable]`: core types on the critical path (`Provider`, `Model`, `Context`, `Message`, `Agent`, `AgentConfig`, `ToolError`, `ToolRegistry`, port traits).
- `#[unstable]`: recently added or evolving surface (browser tools, advisor subsystem, workflow DSL, `PortMemoryBackend`).
- `#[internal]`: pub items that exist for architectural reasons but aren't consumer-facing (internal re-exports, test helpers surfaced as pub).

**Acceptance criteria:**
- [ ] `oxi-api-stability` crate exists and compiles.
- [ ] All three lib crates depend on it.
- [ ] Every root-level `pub` re-export in `oxi-sdk/src/lib.rs` has a tier annotation.
- [ ] Sensitive unstable items are feature-gated behind `oxi-sdk`'s `unstable` cargo feature.
- [ ] `cargo doc -p oxi-sdk` renders stability badges.

### 4.2 R2 — Deprecation convention

**Convention** (in `release-process.md`):

A public symbol marked for removal gets **≥1 release** (ideally 2) of
`#[deprecated(since, note)]` with a migration path before physical removal.
During the deprecation window:
- The API signature is frozen (no signature changes).
- The semantics are frozen (no behavioral changes).
- `cargo build` on consumer code produces a deprecation warning.

**Current state:** no active removal candidates (the P0.2 symbols are already
removed). The convention is established prospectively + the retrospective
CHANGELOG entry (R1) documents what should have happened.

**Acceptance criteria:**
- [ ] `release-process.md` documents the deprecation window rule.
- [ ] Convention is referenced from the ownership contract.

---

## 5. Phase 3 — Composable Traits (R6)

The deepest change. Three behavior traits, each following the pattern: **SDK
owns trait + reference impl; consumer swaps in domain policy.**

### 5.1 CircuitBreaker (NEW)

**Location:** `oxi-ai/src/circuit_breaker.rs` (new file).

Re-introduced as a **minimal behavior trait** — NOT the old 944-line
`ProviderCircuitBreaker`. The old impl was removed because it was "never
constructed in production"; the trait is what should have remained.

```rust
/// Behavior contract for circuit-breaking resilience.
/// SDK owns this trait + DefaultCircuitBreaker; consumers impl it
/// for domain-specific traffic classes (A2A, HTTP, etc.).
pub trait CircuitBreaker: Send + Sync {
    /// Returns Err if the circuit is open (calls should fail-fast).
    fn check(&self) -> Result<(), BreakerError>;
    /// Record a successful call (may close a half-open circuit).
    fn record_success(&self);
    /// Record a failed call (may trip the circuit open).
    fn record_failure(&self);
}

/// SDK-provided reference implementation.
/// Configurable thresholds + half-open state machine.
pub struct DefaultCircuitBreaker { /* failure_threshold, reset_timeout, state */ }
impl CircuitBreaker for DefaultCircuitBreaker { ... }

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BreakerError {
    #[error("circuit open: too many consecutive failures")]
    Open,
}
```

**Wiring:** pluggable via provider/agent construction. A provider or agent can
accept `Arc<dyn CircuitBreaker>`; if none is provided, `DefaultCircuitBreaker`
is used. oxios can pass its own `A2ACircuitBreaker` impl.

**Re-export:** `oxi_sdk::CircuitBreaker`, `oxi_sdk::DefaultCircuitBreaker`,
`oxi_sdk::BreakerError`.

**Stability:** initially `#[unstable(feature = "circuit-breaker")]` — graduates
to `#[stable]` after it proves useful in production.

**Acceptance criteria:**
- [ ] `oxi-ai` has `CircuitBreaker` trait + `DefaultCircuitBreaker`.
- [ ] `oxi-sdk` re-exports all three.
- [ ] A consumer can construct a provider/agent with a custom `CircuitBreaker` impl.
- [ ] Unit tests for the default impl (open/half-open/closed transitions).

### 5.2 McpTransport (extract from oxi-agent::mcp)

**Goal:** separate the transport interface (SDK-owned) from spawn validation
(consumer-owned policy).

Current `oxi_agent::mcp` bundles transport (stdio/sse/http message I/O) with
spawn validation (`validate_mcp_command`, `sanitize_env`). oxios has its own
stricter spawn validation and bypasses the SDK's MCP client entirely.

**Refactor:**

```rust
// oxi-agent/src/mcp/transport.rs — SDK-owned behavior trait
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn send(&self, msg: &jsonrpc::Request) -> Result<(), McpError>;
    async fn recv(&self) -> Result<Option<jsonrpc::Response>, McpError>;
    async fn close(&self) -> Result<(), McpError>;
}

// Reference impls: StdioTransport, SseTransport, HttpTransport

// oxi-agent/src/mcp/spawn.rs — consumer-owned policy hook
pub trait SpawnValidator: Send + Sync {
    fn validate_command(&self, cmd: &str, args: &[String]) -> Result<(), McpError>;
    fn sanitize_env(&self, env: &mut HashMap<String, String>);
}
```

The SDK provides a `DefaultSpawnValidator` (basic safety checks). oxi-cli and
oxios provide their own stricter impls.

**Acceptance criteria:**
- [ ] `McpTransport` trait extracted with stdio reference impl.
- [ ] `SpawnValidator` trait with `DefaultSpawnValidator`.
- [ ] Existing MCP functionality preserved (no behavioral regression).
- [ ] `oxi-sdk` re-exports both traits.

### 5.3 MemoryStore reconciliation

**No new trait.** The apparent duplication is **by design** and documented in
the ownership contract:

| Layer | Owner | Purpose |
|---|---|---|
| `MemoryStore` (SDK port) | SDK | Storage contract (store/recall/forget) |
| `MemoryBackend` (oxi-agent) | SDK | Agent-tool-facing interface (store/search/list/delete) |
| `PortMemoryBackend` | SDK | Adapter: bridges `MemoryBackend` → `MemoryStore` port |

Collapsing these would force every consumer into the SDK's storage model. The
three-layer design lets a consumer implement `MemoryStore` (the port) and get
the memory tools for free via `PortMemoryBackend`, OR implement `MemoryBackend`
directly for a custom tool-facing interface.

**Acceptance criteria:**
- [ ] Ownership contract documents the three-layer design.
- [ ] No code change required (documentation only).

---

## 6. Phase 4 — Robustness

### 6.1 R4 — Zero-panic enforcement

**Current state (verified 2026-08-01):**

| Crate | `unwrap_used` lint | Non-test `.unwrap()` | Non-test `.expect()` | Non-test `panic!` | Non-test `unreachable!` |
|---|---|---|---|---|---|
| oxi-ai | `#![warn]` (line 9) — CI `-D warnings` enforces | 0 | 0 | 0 | 0 |
| oxi-agent | `#![warn]` (line 9) — CI `-D warnings` enforces | 0 | 0 | 0 | **3** |
| oxi-sdk | **none** (only `warn(missing_docs)`) | **3** | 0 | 0 | 0 |

oxi-ai and oxi-agent already enforce unwrap-free shipped code via
`#![warn(clippy::unwrap_used)]` + CI's `-D warnings`. The raw ≈1769 `.unwrap()`
count is entirely inside `#[cfg(test)]` modules (exempted by
`#![cfg_attr(test, allow(...))]`). `.expect()` was already eliminated by the
F-3 fix in 0.59.

**This is a lint-level promotion + 6 targeted spot-fixes, not a sweep.**

**Lint policy changes:**

1. **oxi-ai + oxi-agent** — promote `warn` → `deny` and extend the set:
   ```rust
   #![cfg_attr(not(test), deny(
       clippy::expect_used,
       clippy::panic,
       clippy::unwrap_used,
   ))]
   #![cfg_attr(test, allow(
       clippy::unwrap_used,
       clippy::field_reassign_with_default,
   ))]
   ```
   Replaces the existing `#![warn(clippy::unwrap_used)]` + `#![cfg_attr(test, allow(...))]` pair.
   No code changes needed for unwrap/expect (already clean). Fix the 3 `unreachable!`.

2. **oxi-sdk** — add the same deny attributes (currently absent):
   ```rust
   #![cfg_attr(not(test), deny(
       clippy::expect_used,
       clippy::panic,
       clippy::unwrap_used,
   ))]
   #![cfg_attr(test, allow(
       clippy::unwrap_used,
       clippy::field_reassign_with_default,
   ))]
   ```
   Fix the 3 non-test `.unwrap()` sites.

**Spot-fixes (6 sites total):**

The 3 `unreachable!` in oxi-agent (all post-validation match arms):
- `mcp/mod.rs:437` — `LifecycleMode::Lazy => unreachable!()`. Convert to a
  proper error return or handle the Lazy case explicitly.
- `tools/debug_tool.rs:392` — `_ => unreachable!("action was already validated")`.
  The action is validated earlier; convert to a typed error or tighten the match.
- `tools/eval_tool.rs:142` — `_ => unreachable!()`. Same pattern; convert to
  a typed error.

The 3 `.unwrap()` in oxi-sdk (non-test): locate and convert to `?` /
`unwrap_or` / proper error variant.

If a site represents a true invariant where continuing is genuinely unsafe, a
scoped `#[allow(clippy::panic)]` with a `// SAFETY:` justification comment is
acceptable — but this is the exception, not the default.

**Acceptance criteria:**
- [ ] All three lib crates have the `deny` attributes (non-test) + `allow` (test).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes clean.
- [ ] `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` passes clean.
- [ ] Every remaining `#[allow(...)]` for panic lints has a justification comment.
- [ ] The 3 `unreachable!` + 3 `.unwrap()` sites are resolved (converted to `Result` or justified `#[allow]`).

### 6.2 R7 — Error type stability

| Error type | Location | Current | Action |
|---|---|---|---|
| `SdkError` | `oxi-sdk/src/error.rs:15` | enum, no `#[non_exhaustive]` | Add `#[non_exhaustive]` |
| `ProviderError` | `oxi-ai/src/error.rs:62` | enum, no `#[non_exhaustive]` | Add `#[non_exhaustive]` |
| `ToolError` | `oxi-agent/src/tools.rs:606` | **type alias for `String`** | Stable by construction — document; no variants to break |
| `BreakerError` | `oxi-ai/src/circuit_breaker.rs` (new) | new enum | Add `#[non_exhaustive]` from creation |
| `McpError` | `oxi-agent/src/mcp/` | existing | Verify `#[non_exhaustive]`; add if missing |

**Policy** (in `release-process.md`):
- Existing named variants are **frozen** (semantic stability). Changing what a
  variant means is a silent break, even if the name stays.
- New variants can be added freely (that's why `#[non_exhaustive]`).
- Semantic changes to an existing variant require a rename (new variant) +
  deprecation of the old name.
- Consumers MUST have a catch-all arm (`_ =>`) in their matches (enforced by
  `#[non_exhaustive]`).

**Impact on internal callers:** adding `#[non_exhaustive]` to `SdkError` and
`ProviderError` will cause compile errors in any `match` that lacks a catch-all
arm — both inside oxi and in oxi-cli. These are fixed by adding `_ =>` arms.
This is the desired behavior: it forces explicit handling of future variants.

**Acceptance criteria:**
- [ ] `SdkError` and `ProviderError` have `#[non_exhaustive]`.
- [ ] `ToolError = String` stability documented in the ownership contract.
- [ ] `release-process.md` documents the variant-stability policy.
- [ ] All internal `match` expressions on these types compile (catch-all arms added where needed).

---

## 7. New Workspace Member

```
oxi/
├── oxi-api-stability/   ← NEW proc-macro crate (Phase 2)
├── oxi-ai/
├── oxi-agent/
├── oxi-sdk/
├── oxi-cli/
├── oxi-hashline/
├── oxi-lsp/
├── oxi-mnemopi/
├── oxi-snapcompact/
└── oxi-tui/
```

`oxi-api-stability` is a leaf proc-macro crate (zero internal `oxi-*` deps).
Consumed by `oxi-ai`, `oxi-agent`, `oxi-sdk`. Does not appear in the dependency
flow of leaf crates that don't need stability annotations.

**Publish order** (for `publish.yml`): `oxi-api-stability` publishes first
(before `oxi-ai`), as it's a dependency of the lib crates.

---

## 8. Risk Analysis

| Change | Risk | Mitigation |
|---|---|---|
| `#[non_exhaustive]` on `SdkError`/`ProviderError` | Consumer/internal match arms without catch-all fail to compile | Add catch-all arms in oxi-cli + oxi crates first; consumers (oxios) need the same. This IS the point — it forces explicit handling. |
| `deny(unwrap_used, expect_used, panic)` promotion | Minimal: oxi-ai/oxi-agent already clean; only 6 spot-fixes total (3 `unreachable!` + 3 oxi-sdk `.unwrap()`) | Fix the 6 sites; verify with full clippy + test suite. |
| `CircuitBreaker` trait introduction | Adding a new trait surface that must be maintained | Keep it minimal (3 methods); initially `#[unstable]`. |
| `McpTransport` extraction | Could break existing MCP functionality | Extract incrementally; keep `McpManager` API stable; add trait behind the existing types. |
| `protobuf` feature-gate | Consumers using Devin/Cursor must enable the feature | Document in CHANGELOG `## Changed`; provide migration note. |
| New proc-macro crate | Adds build-time dependency (syn/quote/proc-macro2) | These are already in the build tree (many deps use them); negligible incremental cost. |

---

## 9. Files Touched (summary)

| Phase | Files | Type |
|---|---|---|
| 1 | `docs/oxi-sdk-ownership.md` (new), `oxi-sdk/README.md`, `AGENTS.md`, `docs/release-process.md`, `CHANGELOG.md` | Docs |
| 1 | `oxi-ai/Cargo.toml`, Devin/Cursor provider modules | Feature-gate |
| 1 | `xtask` or CI workflow for breaking-change detection | CI |
| 2 | `oxi-api-stability/` (new crate) | New crate |
| 2 | `oxi-ai/src/lib.rs`, `oxi-agent/src/lib.rs`, `oxi-sdk/src/lib.rs` + public modules | Annotations |
| 3 | `oxi-ai/src/circuit_breaker.rs` (new), `oxi-agent/src/mcp/transport.rs` (new), `oxi-agent/src/mcp/spawn.rs` (new) | New traits |
| 4 | All lib crate roots (lint attrs), 6 panic/unwrap sites, `oxi-sdk/src/error.rs`, `oxi-ai/src/error.rs` | Code |

---

## 10. Out of Scope

- **Restoring the old 944-line `ProviderCircuitBreaker`.** The new `CircuitBreaker` trait is minimal and deliberately different.
- **Collapsing `MemoryBackend` into `MemoryStore`.** The three-layer design is intentional.
- **oxios-side changes.** This program modifies oxi only; oxios adapts separately.
- **SemVer 1.0 commitment.** oxi is still 0.x; this program improves 0.x hygiene, not a 1.0 stability guarantee.
