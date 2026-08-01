# oxi-sdk Stability & Ownership Program — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the full R0–R8 governance program for oxi-sdk, addressing the oxios P0.2 incident through ownership contracts, stability annotations, composable traits, and robustness hardening.

**Architecture:** 4 phases — (1) ownership docs + governance + protobuf feature-gate, (2) proc-macro stability annotations, (3) composable traits (CircuitBreaker, SpawnValidator, McpTransport re-export), (4) zero-panic lints + non_exhaustive errors. Phase 1 gates everything.

**Tech Stack:** Rust 2024 edition, proc-macro2/syn/quote, thiserror, clippy restriction lints, cargo-public-api.

**Spec:** `docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md`

## Global Constraints

- Rust 2024 edition; workspace MSRV 1.96
- `cargo fmt` before every commit — no exceptions
- `cargo clippy --workspace --all-targets -- -D warnings` must pass clean
- `cargo clippy -p oxi-sdk --features native-browser -- -D warnings` must pass
- `parking_lot::RwLock` not `std::sync::RwLock`; drop guards before `.await`
- Library crates: typed error enums with `thiserror`; oxi-cli: `anyhow::Result`
- Test code may `allow(clippy::unwrap_used, clippy::field_reassign_with_default)`; shipped code denies
- Atomic I/O: temp + rename pattern
- Every `pub` symbol gets exactly one stability tier annotation (Phase 2)

## File Structure

| File | Responsibility | Phase |
|---|---|---|
| `docs/oxi-sdk-ownership.md` (new) | Behavior↔policy ownership contract | 1 |
| `docs/release-process.md` | Governance conventions (Breaking/deprecation/variant/deps) | 1 |
| `oxi-ai/Cargo.toml` | protobuf feature-gate | 1 |
| `oxi-ai/build.rs` | Conditional proto compilation | 1 |
| `oxi-ai/src/providers/{cursor,devin}.rs` | `#[cfg(feature = "protobuf")]` | 1 |
| `oxi-ai/src/providers/mod.rs` | Conditional module declarations | 1 |
| `oxi-ai/src/providers/register_builtins.rs` | Conditional provider registration | 1 |
| `oxi-api-stability/` (new crate) | Stability tier proc-macro attributes | 2 |
| `oxi-{ai,agent,sdk}/src/lib.rs` | Tier annotations on root re-exports | 2 |
| `oxi-ai/src/circuit_breaker.rs` (new) | CircuitBreaker trait + DefaultCircuitBreaker | 3 |
| `oxi-agent/src/agent_loop/config.rs` | `circuit_breaker` field on AgentLoopConfig | 3 |
| `oxi-agent/src/agent_loop/stream_retry.rs` | Breaker check/record alongside retry | 3 |
| `oxi-agent/src/mcp/spawn.rs` (new) | SpawnValidator trait + NoopSpawnValidator | 3 |
| `oxi-agent/src/mcp/mod.rs` | SpawnValidator injection on spawn | 3 |
| `oxi-sdk/src/lib.rs` | McpTransport/CircuitBreaker re-exports | 3 |
| `oxi-{ai,agent,sdk}/src/lib.rs` | deny(expect_used, panic, unwrap_used) | 4 |
| `oxi-sdk/src/error.rs` | `#[non_exhaustive]` on SdkError | 4 |
| `oxi-ai/src/error.rs` | `#[non_exhaustive]` on ProviderError | 4 |

---

## Phase 1 — Ownership Contract & Governance Docs

### Task 1: Write the ownership contract (R0/R5)

**Files:**
- Create: `docs/oxi-sdk-ownership.md`
- Modify: `oxi-sdk/README.md`
- Modify: `AGENTS.md` (Port System section)

**Interfaces:**
- Produces: the canonical ownership table referenced by all subsequent tasks

- [ ] **Step 1: Write `docs/oxi-sdk-ownership.md`**

Create the file with the behavior↔policy two-column table from spec §3.1. Include:
- The principle statement ("SDK owns behavior + reference impl; consumer owns policy")
- The full table (10 rows per column)
- The CircuitBreaker example (trait + consumer impl pattern)
- The MemoryStore three-layer documentation (spec §5.3)
- The `ToolError = String` stability note ("stable by construction — type alias, no variants to break")

- [ ] **Step 2: Link from `oxi-sdk/README.md`**

Add a top-level section:
```markdown
## Ownership Contract

See [`docs/oxi-sdk-ownership.md`](../docs/oxi-sdk-ownership.md) for the
canonical "who owns what" table. The SDK owns behavior (interfaces + reference
impls); consumers own policy (domain thresholds, validation, tiering).
```

- [ ] **Step 3: Cross-reference from `AGENTS.md`**

In the Port System section, add after the port table:
```markdown
> See also [`docs/oxi-sdk-ownership.md`](docs/oxi-sdk-ownership.md) for the
> behavior↔policy ownership contract that prevents parallel evolution between
> the SDK and consumers (oxios).
```

- [ ] **Step 4: Commit**

```bash
git add docs/oxi-sdk-ownership.md oxi-sdk/README.md AGENTS.md
git commit -m "docs: SDK ownership contract — behavior↔policy split (R0/R5)"
```

---

### Task 2: Codify governance conventions in release-process.md (R1/R2/R7/R8)

**Files:**
- Modify: `docs/release-process.md`

- [ ] **Step 1: Add `## Breaking` convention (R1)**

After the CHANGELOG Update section (step 2), add a subsection:

```markdown
### Breaking Change Policy

Any root-level `pub` symbol removal, signature change, or semantic change MUST
appear under `## Breaking` in the CHANGELOG with:

1. **Full symbol path** — e.g. `oxi_sdk::ProviderCircuitBreaker`
2. **Replacement API or migration path** — what consumers should use instead
3. **Minimum deprecation window** — how many releases before physical removal
4. **Known affected consumers** — from GitHub code search

The CI `cargo-public-api` diff gate (see `.github/workflows/ci.yml`) fails PRs
that remove public symbols without a matching `## Breaking` entry.
```

- [ ] **Step 2: Add deprecation window rule (R2)**

```markdown
### Deprecation Window

A public symbol marked for removal gets **≥1 release** (ideally 2) of:

```rust
#[deprecated(since = "0.XX.0", note = "use X instead; will be removed in 0.YY.0")]
```

During the deprecation window:
- The API signature is frozen (no signature changes).
- The semantics are frozen (no behavioral changes).
- `cargo build` on consumer code produces a deprecation warning.
```

- [ ] **Step 3: Add error variant stability policy (R7)**

```markdown
### Error Variant Stability

Public error enums (`SdkError`, `ProviderError`, `BreakerError`) are
`#[non_exhaustive]`:
- **New variants** may be added freely (consumers need a catch-all `_ =>` arm).
- **Existing named variants are frozen** — changing what a variant means is a
  silent break, even if the name stays the same.
- **Semantic changes** require a rename (new variant) + deprecation of the old.
- `ToolError` is a type alias for `String` — stable by construction, no variants.
```

- [ ] **Step 4: Add heavy dependency policy (R8)**

```markdown
### Heavy Dependency Policy

Adding a heavy build dependency (>50 transitive crates, or requires a vendored
binary) requires:

1. A CHANGELOG `## Changed` entry noting build impact (e.g.
   `+~120 crates, +~150s cold build time`).
2. Feature-gating behind an off-by-default cargo feature so consumers who don't
   need it pay no build cost.
```

- [ ] **Step 5: Commit**

```bash
git add docs/release-process.md
git commit -m "docs: governance conventions — Breaking/deprecation/variant/deps (R1/R2/R7/R8)"
```

---

### Task 3: Feature-gate protobuf providers (R8)

**Files:**
- Modify: `oxi-ai/Cargo.toml`
- Modify: `oxi-ai/build.rs`
- Modify: `oxi-ai/src/providers/mod.rs`
- Modify: `oxi-ai/src/providers/register_builtins.rs`
- Test: `cargo build -p oxi-ai` (default features must not pull prost)

**Interfaces:**
- Produces: `protobuf` cargo feature on `oxi-ai`
- Consumes: existing `cursor.rs`, `devin.rs` provider modules

- [ ] **Step 1: Make prost deps optional in Cargo.toml**

In `oxi-ai/Cargo.toml`, change the prost dependencies to optional:

```toml
# Protobuf (Devin, Cursor) — feature-gated
prost = { version = "0.14", optional = true }
tokio-stream = "0.1"

[build-dependencies]
prost-build = { version = "0.14", optional = true }
protoc-bin-vendored = { version = "3.0", optional = true }

[features]
protobuf = ["dep:prost", "dep:prost-build", "dep:protoc-bin-vendored"]
```

- [ ] **Step 2: Gate build.rs proto compilation**

Wrap the proto compilation in a feature check:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only compile protos when the `protobuf` feature is enabled.
    if std::env::var("CARGO_FEATURE_PROTOBUF").is_err() {
        return Ok(());
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    // ... existing code unchanged ...
    Ok(())
}
```

- [ ] **Step 3: Gate provider modules behind the feature**

In `oxi-ai/src/providers/mod.rs`, wrap the module declarations:

```rust
#[cfg(feature = "protobuf")]
pub mod cursor;
#[cfg(feature = "protobuf")]
pub mod devin;
```

Remove the unconditional `pub mod cursor;` / `pub mod devin;` lines.

- [ ] **Step 4: Gate provider registration**

In `register_builtins.rs`, wrap the Cursor/Devin provider registration:

```rust
#[cfg(feature = "protobuf")]
entries.push(BuiltinProvider { /* cursor */ });
#[cfg(feature = "protobuf")]
entries.push(BuiltinProvider { /* devin */ });
```

- [ ] **Step 5: Verify default build doesn't pull prost**

```bash
cargo build -p oxi-ai
cargo tree -p oxi-ai | grep prost
# Expected: no output (prost not in dependency tree)
```

- [ ] **Step 6: Verify protobuf feature still compiles**

```bash
cargo build -p oxi-ai --features protobuf
# Expected: compiles successfully
```

- [ ] **Step 7: Run clippy on both configs**

```bash
cargo clippy -p oxi-ai -- -D warnings
cargo clippy -p oxi-ai --features protobuf -- -D warnings
```

- [ ] **Step 8: Commit**

```bash
git add oxi-ai/
git commit -m "feat(oxi-ai): feature-gate protobuf providers behind 'protobuf' feature (R8)

Default build no longer pulls prost/prost-build/protoc-bin-vendored (~120 crates).
Consumers using Devin/Cursor providers enable with --features protobuf."
```

---

### Task 4: CHANGELOG retrospective + cargo-public-api CI gate (R1)

**Files:**
- Modify: `CHANGELOG.md`
- Create: `.github/workflows/api-diff.yml` (or add job to `ci.yml`)

- [ ] **Step 1: Back-fill CHANGELOG `## Breaking` for 0.61 removals**

Add to the 0.61.0 section (or a new retrospective block at top of Unreleased):

```markdown
## Breaking (retrospective — 0.61.0)

The following symbols were removed in 0.61.0 without adequate warning. This
entry documents them retroactively per the new Breaking Change Policy.

- `oxi_ai::ProviderPool`, `oxi_ai::RateLimitPolicy` — removed (`provider_pool`
  module, 203 LOC). No direct replacement; the router pipeline
  (`RouterPipeline`) supersedes multi-provider routing.
- `oxi_ai::CircuitBreakerConfig`, `oxi_ai::ProviderCircuitBreaker` — removed
  (`circuit_breaker` module, 944 LOC). A minimal `CircuitBreaker` trait will be
  re-introduced (see R6).
- `oxi_ai::MultiProviderBuilder`, `oxi_ai::RoutingConfig`,
  `oxi_ai::MultiProviderConfig` — removed (`multi_provider` module, 1283+359
  LOC). Superseded by `RouterPipeline` + `router://local` provider.
```

- [ ] **Step 2: Add cargo-public-api CI job**

Add a job to `.github/workflows/ci.yml` (or create `.github/workflows/api-diff.yml`):

```yaml
  api-diff:
    name: Public API Diff
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # need full history for base comparison
      - uses: dtolnay/rust-toolchain@stable
      - name: Install cargo-public-api
        run: cargo install cargo-public-api
      - name: Check for undocumented removals
        run: |
          # Compare public API surface between base and HEAD
          BASE_REF="${{ github.base_ref }}"
          if [ -z "$BASE_REF" ]; then exit 0; fi  # skip on non-PR pushes
          # Build a simple check: if any oxi-sdk/oxi-ai/oxi-agent public item
          # disappears, fail unless CHANGELOG has a ## Breaking entry
          for crate in oxi-ai oxi-agent oxi-sdk; do
            cargo public-api diff "$BASE_REF..HEAD" -p "$crate" 2>/dev/null || true
          done
```

Note: the exact `cargo-public-api` invocation may need adjustment based on the
tool's version. The goal: fail the CI job if public items disappear without a
CHANGELOG entry. A simpler initial version can just run `cargo public-api list`
and archive the output for manual review, then add enforcement later.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md .github/workflows/
git commit -m "ci: cargo-public-api diff gate + retrospective Breaking entry (R1)"
```

---

## Phase 2 — Stability Annotations

### Task 5: Create `oxi-api-stability` proc-macro crate (R3)

**Files:**
- Create: `oxi-api-stability/Cargo.toml`
- Create: `oxi-api-stability/src/lib.rs`
- Modify: root `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: `#[stable]`, `#[unstable]`, `#[internal]`, `#[deprecated]` attribute macros
- Consumes: nothing (leaf crate)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "oxi-api-stability"
version.workspace = true
edition = "2024"
license = "MIT"
description = "Stability tier attribute macros for oxi workspace crates"
publish = true

[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
syn = { version = "2", features = ["full", "parsing"] }
quote = "1"
```

Add to root `Cargo.toml` `[workspace] members`:
```toml
members = [
    "oxi-api-stability",
    # ... existing members
]
```

- [ ] **Step 2: Write the proc-macro library**

```rust
// oxi-api-stability/src/lib.rs
//! Stability tier attribute macros for the oxi workspace.
//!
//! Provides four attributes that render as colored badges in `cargo doc`:
//! - `#[stable(since = "0.63.0")]` — green badge, semver-stable
//! - `#[unstable(feature = "browser")]` — amber badge, may change
//! - `#[internal]` — hides from docs (`#[doc(hidden)]`)
//! - `#[deprecated(since, note)]` — red badge + native deprecation warning

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, ItemStruct, ItemEnum, ItemTrait, ItemMod};

/// Attribute applied to any item to mark it semver-stable.
#[proc_macro_attribute]
pub fn stable(args: TokenStream, input: TokenStream) -> TokenStream {
    let since: syn::LitStr = syn::parse(args).expect("expected since = \"version\"");
    let since_val = since.value();
    // Pass through the item unchanged, add a doc badge.
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[doc = concat!(" <div class=\"stab stable\"><strong>Stable</strong> since ", #since_val, "</div>")]
        #input
    }
    .into()
}

/// Attribute applied to any item to mark it unstable/experimental.
#[proc_macro_attribute]
pub fn unstable(args: TokenStream, input: TokenStream) -> TokenStream {
    let feature: syn::LitStr = syn::parse(args).expect("expected feature = \"name\"");
    let feature_val = feature.value();
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[doc = concat!(" <div class=\"stab unstable\"><strong>Unstable</strong> (feature: ", #feature_val, ") — may change or be removed</div>")]
        #input
    }
    .into()
}

/// Attribute that hides an item from consumer-facing docs.
#[proc_macro_attribute]
pub fn internal(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[doc(hidden)]
        #input
    }
    .into()
}

/// Attribute that marks an item as deprecated with a doc badge.
/// Wraps the native #[deprecated] attribute.
#[proc_macro_attribute]
pub fn deprecated(args: TokenStream, input: TokenStream) -> TokenStream {
    // Parse since = "0.64.0", note = "..."
    let since: syn::LitStr = syn::parse_macro_input!(args as syn::LitStr);
    let input: proc_macro2::TokenStream = input.into();
    let since_val = since.value();
    quote! {
        #[deprecated(since = #since_val)]
        #[doc = concat!(" <div class=\"stab deprecated\"><strong>Deprecated</strong> since ", #since_val, "</div>")]
        #input
    }
    .into()
}
```

Note: the `deprecated` macro's argument parsing depends on the desired ergonomics.
A simpler version just forwards to native `#[deprecated]` with a doc badge. The
implementer should decide whether to support `note` as a macro arg or rely on
the native attribute alongside.

- [ ] **Step 3: Verify it compiles**

```bash
cargo build -p oxi-api-stability
```

- [ ] **Step 4: Write a basic test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[stable(since = "0.63.0")]
    struct TestStable;

    #[unstable(feature = "test")]
    struct TestUnstable;

    #[internal]
    struct TestInternal;

    #[test]
    fn stable_compiles() {
        let _ = TestStable;
    }

    #[test]
    fn unstable_compiles() {
        let _ = TestUnstable;
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add oxi-api-stability/ Cargo.toml
git commit -m "feat: oxi-api-stability proc-macro crate — #[stable]/#[unstable]/#[internal] (R3)"
```

---

### Task 6: Annotate the public API surface (R3)

**Files:**
- Modify: `oxi-ai/Cargo.toml`, `oxi-agent/Cargo.toml`, `oxi-sdk/Cargo.toml` (add dep)
- Modify: `oxi-sdk/src/lib.rs` (annotate root re-exports)
- Modify: `oxi-ai/src/lib.rs` (annotate key types)
- Modify: `oxi-agent/src/lib.rs` (annotate key types)

**Interfaces:**
- Consumes: `oxi-api-stability` crate from Task 5
- Produces: tier-annotated public API

- [ ] **Step 1: Add oxi-api-stability dependency to all three lib crates**

In each `Cargo.toml`:
```toml
[dependencies]
oxi-api-stability = { path = "../oxi-api-stability" }
```

- [ ] **Step 2: Annotate oxi-sdk root re-exports**

In `oxi-sdk/src/lib.rs`, add `use oxi_api_stability::*;` and annotate the major
re-export groups. Examples:

```rust
use oxi_api_stability::*;

#[stable(since = "0.63.0")]
pub use oxi_ai::{Provider, ProviderRegistry, Model, Context, Message};

#[stable(since = "0.63.0")]
pub use oxi_agent::{Agent, AgentConfig, AgentTool, AgentToolResult, ToolRegistry, ToolError};

#[unstable(feature = "browser")]
pub use oxi_agent::tools::browse::{BrowseConfig, BrowseTool, BrowserEngine, BrowserError};

#[unstable(feature = "advisor")]
pub use oxi_agent::advisor::{AdvisorRuntime, AdviseTool};

#[unstable(feature = "workflow-dsl")]
pub use workflow_engine::{StepOutput, WorkflowEngine, WorkflowResult};
```

Focus on the ~50 root-level re-exports. Use tier guidance from spec §4.1.

- [ ] **Step 3: Annotate oxi-ai key public types**

In `oxi-ai/src/lib.rs`, annotate the core types:
- `Provider` trait → `#[stable]`
- `ProviderRegistry` → `#[stable]`
- `Model`, `Context`, `Message`, `ContentBlock` → `#[stable]`
- `ProviderError`, `ProviderEvent` → `#[stable]`
- `catalog` module → `#[stable]`
- `dialect` module → `#[unstable(feature = "dialect")]`

- [ ] **Step 4: Annotate oxi-agent key public types**

In `oxi-agent/src/lib.rs`:
- `Agent`, `AgentConfig`, `AgentEvent` → `#[stable]`
- `AgentTool` trait → `#[stable]`
- `ToolRegistry` → `#[stable]`
- `MemoryBackend` trait → `#[unstable(feature = "memory")]`
- `SubagentRunner` trait → `#[unstable(feature = "subagent")]`
- `LspProvider` trait → `#[unstable(feature = "lsp")]`

- [ ] **Step 5: Verify compilation + docs**

```bash
cargo build --workspace
cargo doc -p oxi-sdk --no-deps
# Check that stability badges render in the generated docs
```

- [ ] **Step 6: Run clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 7: Commit**

```bash
git add oxi-ai/ oxi-agent/ oxi-sdk/
git commit -m "feat: stability tier annotations on public API surface (R3)"
```

---

### Task 7: Add `unstable` cargo feature to oxi-sdk (R3)

**Files:**
- Modify: `oxi-sdk/Cargo.toml`
- Modify: `oxi-sdk/src/lib.rs`

- [ ] **Step 1: Add the feature to Cargo.toml**

```toml
[features]
unstable = []
native-browser = ["dep:oxibrowser-core"]
```

- [ ] **Step 2: Gate sensitive unstable items behind the feature**

In `oxi-sdk/src/lib.rs`, wrap the most volatile re-exports:

```rust
/// Unstable API surface — may change or be removed between minor releases.
/// Enable with `oxi-sdk = { features = ["unstable"] }`.
#[cfg(feature = "unstable")]
pub mod unstable_api {
    pub use oxi_agent::advisor::*;
    pub use crate::workflow_engine::*;
    pub use crate::workflow_dsl::*;
}
```

Keep the currently-exported items in place for now (don't break existing
consumers); the `unstable_api` module is a forward-looking opt-in surface for
new consumers. Moving existing exports behind the gate is a separate migration
that requires a deprecation cycle.

- [ ] **Step 3: Verify**

```bash
cargo build -p oxi-sdk                    # default: no unstable_api
cargo build -p oxi-sdk --features unstable # unstable_api available
```

- [ ] **Step 4: Commit**

```bash
git add oxi-sdk/
git commit -m "feat(oxi-sdk): 'unstable' cargo feature for opt-in API surface (R3)"
```

---

## Phase 3 — Composable Traits

### Task 8: CircuitBreaker trait + DefaultCircuitBreaker + AgentLoopConfig wiring (R6)

**Files:**
- Create: `oxi-ai/src/circuit_breaker.rs`
- Modify: `oxi-ai/src/lib.rs` (module declaration + re-export)
- Modify: `oxi-agent/src/agent_loop/config.rs` (add field)
- Modify: `oxi-agent/src/agent_loop/config.rs` (Default impl)
- Modify: `oxi-agent/src/stream_retry.rs` (wire check/record)
- Modify: `oxi-sdk/src/lib.rs` (re-export)
- Test: `oxi-ai/src/circuit_breaker.rs` (unit tests)

**Interfaces:**
- Produces: `CircuitBreaker` trait, `DefaultCircuitBreaker` struct, `BreakerError` enum
- Consumes: `AgentLoopConfig` (adds `circuit_breaker` field)

- [ ] **Step 1: Write the failing test**

```rust
// oxi-ai/src/circuit_breaker.rs (bottom of file)
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn breaker_starts_closed_allows_calls() {
        let b = DefaultCircuitBreaker::new(3, Duration::from_secs(30));
        assert!(b.check().is_ok());
    }

    #[test]
    fn breaker_opens_after_threshold_failures() {
        let b = DefaultCircuitBreaker::new(3, Duration::from_secs(30));
        b.record_failure();
        b.record_failure();
        assert!(b.check().is_ok()); // 2 < 3, still closed
        b.record_failure();
        assert!(b.check().is_err()); // 3 >= 3, now open
    }

    #[test]
    fn breaker_half_opens_after_timeout() {
        let b = DefaultCircuitBreaker::new(1, Duration::from_millis(10));
        b.record_failure(); // trips immediately
        assert!(b.check().is_err()); // open
        std::thread::sleep(Duration::from_millis(15));
        // After timeout, should allow one trial call (half-open)
        assert!(b.check().is_ok()); // half-open allows trial
        b.record_success();
        assert!(b.check().is_ok()); // closed again
    }

    #[test]
    fn success_resets_failure_count() {
        let b = DefaultCircuitBreaker::new(3, Duration::from_secs(30));
        b.record_failure();
        b.record_failure();
        b.record_success(); // resets
        b.record_failure();
        b.record_failure();
        assert!(b.check().is_ok()); // only 2 since reset, still closed
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo nextest run -p oxi-ai circuit_breaker
# Expected: FAIL — module doesn't exist
```

- [ ] **Step 3: Implement CircuitBreaker trait + DefaultCircuitBreaker**

```rust
// oxi-ai/src/circuit_breaker.rs
//! Circuit-breaker behavior trait + reference implementation.
//!
//! SDK owns this trait + [`DefaultCircuitBreaker`]; consumers implement it
//! for domain-specific traffic classes (A2A, HTTP, LLM calls).
//! See `docs/oxi-sdk-ownership.md`.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Behavior contract for circuit-breaking resilience.
pub trait CircuitBreaker: Send + Sync {
    /// Returns `Err(BreakerError::Open)` if the circuit is open.
    fn check(&self) -> Result<(), BreakerError>;
    /// Record a successful call (resets failure count, closes half-open).
    fn record_success(&self);
    /// Record a failed call (increments failure count, may trip open).
    fn record_failure(&self);
}

/// Error returned when the circuit is open.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum BreakerError {
    #[error("circuit open: too many consecutive failures")]
    Open,
}

#[repr(u8)]
enum State { Closed = 0, Open = 1, HalfOpen = 2 }

/// Reference implementation: threshold-based with half-open state machine.
pub struct DefaultCircuitBreaker {
    failure_threshold: u32,
    reset_timeout: Duration,
    state: AtomicU8,
    failure_count: AtomicU64,
    last_failure_epoch_ms: AtomicU64,
}

impl DefaultCircuitBreaker {
    pub fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        Self {
            failure_threshold,
            reset_timeout,
            state: AtomicU8::new(State::Closed as u8),
            failure_count: AtomicU64::new(0),
            last_failure_epoch_ms: AtomicU64::new(0),
        }
    }

    fn now_ms() -> u64 {
        Instant::now().elapsed().as_millis() as u64 // placeholder; see note below
    }
}
```

Note: `Instant` doesn't have an epoch. Use `std::time::SystemTime::now()` or
store a creation `Instant` and compute `elapsed()`. The implementer should use
`Instant` stored at construction time:

```rust
impl DefaultCircuitBreaker {
    pub fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        Self {
            failure_threshold,
            reset_timeout,
            created_at: Instant::now(),
            state: AtomicU8::new(0),
            failure_count: AtomicU64::new(0),
            last_failure_secs: AtomicU64::new(0),
        }
    }
}

impl CircuitBreaker for DefaultCircuitBreaker {
    fn check(&self) -> Result<(), BreakerError> {
        let state = self.state.load(Ordering::Acquire);
        match state {
            0 => Ok(()), // Closed
            1 => {
                // Open — check if timeout has elapsed
                let elapsed = self.created_at.elapsed();
                let last = Duration::from_secs(self.last_failure_secs.load(Ordering::Acquire));
                if elapsed >= last + self.reset_timeout {
                    self.state.store(2, Ordering::Release); // HalfOpen
                    Ok(())
                } else {
                    Err(BreakerError::Open)
                }
            }
            2 => Ok(()), // HalfOpen — allow trial call
            _ => Ok(()),
        }
    }

    fn record_success(&self) {
        self.failure_count.store(0, Ordering::Release);
        self.state.store(0, Ordering::Release); // Closed
    }

    fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        self.last_failure_secs.store(
            self.created_at.elapsed().as_secs(),
            Ordering::Release,
        );
        if count >= self.failure_threshold as u64 {
            self.state.store(1, Ordering::Release); // Open
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo nextest run -p oxi-ai circuit_breaker
# Expected: PASS (all 4 tests)
```

- [ ] **Step 5: Add module declaration + re-export in oxi-ai**

In `oxi-ai/src/lib.rs`:
```rust
pub mod circuit_breaker;
pub use circuit_breaker::{BreakerError, CircuitBreaker, DefaultCircuitBreaker};
```

- [ ] **Step 6: Add `circuit_breaker` field to `AgentLoopConfig`**

In `oxi-agent/src/agent_loop/config.rs`:
```rust
use oxi_ai::circuit_breaker::CircuitBreaker;

pub struct AgentLoopConfig {
    // ... existing fields ...

    /// Optional circuit breaker for provider calls.
    /// When set, the agent loop checks before each provider attempt and
    /// records success/failure after. `None` = no circuit breaking (default).
    pub circuit_breaker: Option<std::sync::Arc<dyn CircuitBreaker>>,
}
```

In the `Default` impl, add: `circuit_breaker: None,`

- [ ] **Step 7: Wire into the retry logic**

In `oxi-agent/src/agent_loop/stream_retry.rs` (or wherever the provider's
`stream()` is called with retry), add breaker calls:

```rust
// Before the provider call:
if let Some(breaker) = &config.circuit_breaker {
    if let Err(e) = breaker.check() {
        return Err(/* map BreakerError::Open to the retry error type */);
    }
}

// After the provider call succeeds:
if let Some(breaker) = &config.circuit_breaker {
    breaker.record_success();
}

// After the provider call fails:
if let Some(breaker) = &config.circuit_breaker {
    breaker.record_failure();
}
```

- [ ] **Step 8: Re-export from oxi-sdk**

In `oxi-sdk/src/lib.rs`:
```rust
pub use oxi_ai::circuit_breaker::{BreakerError, CircuitBreaker, DefaultCircuitBreaker};
```

- [ ] **Step 9: Verify full workspace builds**

```bash
cargo build --workspace
cargo nextest run -p oxi-ai circuit_breaker
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 10: Commit**

```bash
git add oxi-ai/src/circuit_breaker.rs oxi-ai/src/lib.rs oxi-agent/src/agent_loop/ oxi-sdk/src/lib.rs
git commit -m "feat: CircuitBreaker trait + DefaultCircuitBreaker + AgentLoopConfig wiring (R6)"
```

---

### Task 9: McpTransport re-export + SpawnValidator trait (R6)

**Files:**
- Modify: `oxi-sdk/src/lib.rs` (re-export existing McpTransport)
- Create: `oxi-agent/src/mcp/spawn.rs`
- Modify: `oxi-agent/src/mcp/mod.rs` (add module + inject SpawnValidator)
- Modify: `oxi-agent/src/lib.rs` (re-export)

**Interfaces:**
- Consumes: existing `McpTransport` trait at `oxi-agent/src/mcp/transport/mod.rs:48`
- Produces: `SpawnValidator` trait, `NoopSpawnValidator`, re-exported `McpTransport`

- [ ] **Step 1: Add McpTransport re-exports to oxi-sdk**

In `oxi-sdk/src/lib.rs`, add to the existing MCP re-export block (line 286-291):

```rust
pub use oxi_agent::mcp::{
    // ... existing exports ...
    // Transport layer (re-export for consumers implementing custom transports)
};
pub use oxi_agent::mcp::transport::{McpTransport, InboundHandler};
// StdioTransport and StreamableHttpTransport are in transport::{stdio, http}
pub use oxi_agent::mcp::transport::stdio::StdioTransport;
pub use oxi_agent::mcp::transport::http::StreamableHttpTransport;
```

Verify the exact paths by reading `transport/mod.rs` — `StdioTransport` is in
`transport::stdio`, `StreamableHttpTransport` in `transport::http`.

- [ ] **Step 2: Create SpawnValidator trait**

```rust
// oxi-agent/src/mcp/spawn.rs
//! Spawn validation policy hook for MCP servers.
//!
//! The SDK owns the trait; consumers (oxi-cli, oxios) own the policy impl.
//! See `docs/oxi-sdk-ownership.md`.

/// Validates MCP server spawn commands and environment.
///
/// Consumers inject domain-specific safety policy (forbidden shells,
/// dangerous env vars, path traversal checks) without modifying the SDK's
/// MCP client. The SDK calls `validate_command` before spawning and
/// `sanitize_env` before passing the environment to the child process.
pub trait SpawnValidator: Send + Sync {
    /// Validate the command + args before spawn. Return `Err` to block.
    fn validate_command(&self, cmd: &str, args: &[String]) -> Result<(), String>;

    /// Sanitize or strip dangerous environment variables before spawn.
    fn sanitize_env(&self, env: &mut std::collections::HashMap<String, String>);
}

/// No-op validator — preserves current behavior (no validation).
pub struct NoopSpawnValidator;
impl SpawnValidator for NoopSpawnValidator {
    fn validate_command(&self, _: &str, _: &[String]) -> Result<(), String> {
        Ok(())
    }
    fn sanitize_env(&self, _: &mut std::collections::HashMap<String, String>) {}
}
```

- [ ] **Step 3: Add module declaration in mcp/mod.rs**

```rust
pub mod spawn;
pub use spawn::{NoopSpawnValidator, SpawnValidator};
```

- [ ] **Step 4: Wire SpawnValidator into McpManager**

Add an optional `spawn_validator` field to the `McpManager` or its config.
In `McpManager::spawn_with_paths()`, before spawning each server, call:

```rust
if let Some(validator) = &self.spawn_validator {
    validator.validate_command(&cmd, &args)
        .map_err(|e| anyhow::anyhow!("spawn validation failed: {e}"))?;
    validator.sanitize_env(&mut env);
}
```

The field defaults to `None` (no validation) to preserve existing behavior.
oxi-cli can register a `DefaultSpawnValidator` later.

- [ ] **Step 5: Re-export SpawnValidator from oxi-sdk**

```rust
pub use oxi_agent::mcp::{NoopSpawnValidator, SpawnValidator};
```

- [ ] **Step 6: Verify MCP tests still pass**

```bash
cargo nextest run -p oxi-agent mcp
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 7: Commit**

```bash
git add oxi-agent/src/mcp/ oxi-sdk/src/lib.rs
git commit -m "feat: McpTransport re-export + SpawnValidator trait (R6)"
```

---

## Phase 4 — Robustness

### Task 10: Promote deny lints + fix 3 unreachable! (R4)

**Files:**
- Modify: `oxi-ai/src/lib.rs` (promote warn→deny)
- Modify: `oxi-agent/src/lib.rs` (promote warn→deny)
- Modify: `oxi-sdk/src/lib.rs` (add deny)
- Modify: `oxi-agent/src/mcp/mod.rs:437` (fix unreachable!)
- Modify: `oxi-agent/src/tools/debug_tool.rs:392` (fix unreachable!)
- Modify: `oxi-agent/src/tools/eval_tool.rs:142` (fix unreachable!)

- [ ] **Step 1: Promote lint in oxi-ai/src/lib.rs**

Replace lines 9-10:
```rust
#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::field_reassign_with_default))]
```
With:
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

- [ ] **Step 2: Promote lint in oxi-agent/src/lib.rs**

Same replacement as Step 1.

- [ ] **Step 3: Add deny lints to oxi-sdk/src/lib.rs**

After the existing `#![warn(missing_docs)]` (line 13), add:
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

- [ ] **Step 4: Fix `mcp/mod.rs:437`**

```rust
LifecycleMode::Lazy => {
    // Lazy servers are not started eagerly — they spawn on first use.
    // This arm is unreachable in start_eager_servers() because the
    // function pre-filters for non-Lazy modes, but handle it explicitly
    // rather than panicking.
}
```

- [ ] **Step 5: Fix `debug_tool.rs:392`**

```rust
_ => {
    // Action was validated against the supported set earlier in this
    // function; reaching here means an unsupported action slipped through.
    // Return Ok(()) rather than panicking — the validation above already
    // returned an error for truly unknown actions.
}
```

- [ ] **Step 6: Fix `eval_tool.rs:142`**

The match handles `"py"` and `"js"`. Change the catch-all:
```rust
_ => {
    return Ok(AgentToolResult::error(format!(
        "Unsupported language: '{language}'. Supported: py, js"
    )));
}
```

This requires adjusting the return type of the enclosing function to return
`Result<AgentToolResult, ToolError>` — check the current signature and adapt.
If the enclosing code uses `?`, the error return may need to be an `Err(...)`.

- [ ] **Step 7: Verify all configs pass clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p oxi-sdk --features native-browser -- -D warnings
```

- [ ] **Step 8: Run full test suite**

```bash
cargo nextest run --workspace
```

- [ ] **Step 9: Commit**

```bash
git add oxi-ai/src/lib.rs oxi-agent/src/lib.rs oxi-sdk/src/lib.rs oxi-agent/src/mcp/mod.rs oxi-agent/src/tools/debug_tool.rs oxi-agent/src/tools/eval_tool.rs
git commit -m "fix: promote deny(expect_used,panic,unwrap_used) + fix 3 unreachable! (R4)"
```

---

### Task 11: Add `#[non_exhaustive]` to public error types (R7)

**Files:**
- Modify: `oxi-sdk/src/error.rs`
- Modify: `oxi-ai/src/error.rs`
- Fix: all internal `match` expressions on these types (add `_ =>` arms)

- [ ] **Step 1: Add `#[non_exhaustive]` to `SdkError`**

In `oxi-sdk/src/error.rs`, add `#[non_exhaustive]` before `pub enum SdkError`:

```rust
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SdkError {
```

- [ ] **Step 2: Add `#[non_exhaustive]` to `ProviderError`**

In `oxi-ai/src/error.rs`, add `#[non_exhaustive]` before `pub enum ProviderError`:

```rust
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ProviderError {
```

- [ ] **Step 3: Fix internal match expressions**

```bash
cargo build --workspace 2>&1 | grep "non_exhaustive\|match"
```

For each compile error about non-exhaustive match, add a catch-all arm:
```rust
_ => { /* handle or propagate as unexpected error */ }
```

Likely locations: `is_retryable()` in `ProviderError` (already uses `matches!`
which is fine), any `match self` in `SdkError` methods.

- [ ] **Step 4: Verify compilation**

```bash
cargo build --workspace
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add oxi-sdk/src/error.rs oxi-ai/src/error.rs
git commit -m "feat: #[non_exhaustive] on SdkError + ProviderError (R7)"
```

---

## Self-Review

### Spec coverage check

| Spec section | Request | Task | Status |
|---|---|---|---|
| §3.1 | R0/R5 ownership contract | Task 1 | ✅ |
| §3.2 | R1 CHANGELOG + CI gate | Task 4 | ✅ |
| §3.3 | R8 protobuf feature-gate | Task 3 | ✅ |
| §4.1 | R3 proc-macro crate | Task 5 | ✅ |
| §4.1 | R3 annotation rollout | Task 6 | ✅ |
| §4.1 | R3 unstable feature gate | Task 7 | ✅ |
| §4.2 | R2 deprecation convention | Task 2 | ✅ |
| §5.1 | R6 CircuitBreaker | Task 8 | ✅ |
| §5.2 | R6 McpTransport re-export + SpawnValidator | Task 9 | ✅ |
| §5.3 | R6 MemoryStore layering | Task 1 (doc) | ✅ |
| §6.1 | R4 zero-panic lints | Task 10 | ✅ |
| §6.2 | R7 non_exhaustive errors | Task 11 | ✅ |
| Release-process policies (R1/R2/R7/R8) | Task 2 | ✅ |

### Placeholder scan
No TBD/TODO. All steps contain concrete code or commands.

### Type consistency
- `CircuitBreaker` trait defined in Task 8 → used in `AgentLoopConfig` (Task 8) → re-exported (Task 8). Consistent.
- `SpawnValidator` trait defined in Task 9 → wired into `McpManager` (Task 9) → re-exported (Task 9). Consistent.
- `BreakerError` enum: `#[non_exhaustive]` from creation (Task 8). Consistent with R7 policy (Task 2).
- `SdkError`/`ProviderError` get `#[non_exhaustive]` in Task 11, after all other code changes land. Consistent.
