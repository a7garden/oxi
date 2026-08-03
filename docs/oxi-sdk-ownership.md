# oxicode-sdk Ownership Contract

> **Status:** Approved (Phase 1 of SDK Stability & Ownership Program)
> **Source:** [`docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md`](superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md) §3.1, §5.1, §5.3
> **Audience:** oxicode maintainers and consumer authors (oxios, oxios-kernel, etc.)

This document is the canonical "who owns what" table for the SDK ↔ consumer
boundary. It exists because the oxios **P0.2 incident** (2026-08-01) showed that
when neither side knows which layer owns a capability, the SDK and the consumer
silently evolve in parallel and the consumer's build breaks without warning.

---

## 1. Principle

> **The SDK owns behavior + reference implementation; the consumer owns policy.**

- **SDK** — *how* a capability behaves. Trait definition + one reference impl
  per trait. Reference impls are *examples*, not policy. They can change between
  releases without breaking a consumer's trait impl.
- **Consumer** — *what thresholds, validation, or tiering* apply to their
  domain. Domain-specific breakers, sandbox rules, memory tiering, etc.

The split is mechanical: the SDK never prescribes a numeric threshold, an
allow-list, or a tier policy, because those depend on the consumer's traffic
profile, security posture, and storage budget. The SDK provides the *machinery*
(check/record_success/record_failure, store/recall/forget, validate_command/
sanitize_env); the consumer provides the *policy*.

When in doubt: **a reference impl that ships with the SDK is illustrative, not
authoritative.** A consumer that depends on a reference impl's exact thresholds
will break on the next minor release. Implement the trait instead.

---

## 2. Ownership Table

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

Every row is the same shape: a *behavior* on the left (interface + SDK-shipped
reference impl) paired with a *policy slot* on the right (where the consumer
plugs in their own implementation or numeric thresholds).

The pairing prevents the most common cross-evolution failure: a consumer writes
domain policy (right column) that accidentally shadows an SDK reference impl
(left column). When that happens, the consumer forks the SDK's machinery instead
of just plugging in their policy. Examples from P0.2:

- oxios implemented its own MCP client because the SDK's MCP transport didn't
  match oxios's spawn-validation policy. After this contract, oxios implements
  `SpawnValidator` (policy, right column) and keeps the SDK's MCP transport
  (behavior, left column).
- oxios implemented its own circuit breaker because `DefaultCircuitBreaker`'s
  thresholds didn't match oxios's A2A traffic profile. After this contract,
  oxios implements `CircuitBreaker` for `A2ACircuitBreaker` (policy, right
  column) and keeps the SDK's `DefaultCircuitBreaker` for general HTTP (behavior,
  left column).

---

## 3. Reference Pattern: `CircuitBreaker`

The pattern applies to every behavior trait in the table. The `CircuitBreaker`
example is canonical.

### 3.1 SDK owns: the trait + a reference impl

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
impl CircuitBreaker for DefaultCircuitBreaker { /* ... */ }

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BreakerError {
    #[error("circuit open: too many consecutive failures")]
    Open,
}
```

Wired via `AgentLoopConfig.circuit_breaker: Option<Arc<dyn CircuitBreaker>>`.
When `None`, no circuit breaking occurs (existing behavior preserved).

### 3.2 Consumer owns: domain thresholds

```rust
// In oxios (sister repo):
use oxicode_sdk::{CircuitBreaker, BreakerError};
use std::sync::Mutex;

pub struct A2ACircuitBreaker {
    failure_threshold: u32,   // oxios's A2A traffic profile: 12/min
    reset_timeout: Duration,  // oxios's recovery window: 30s
    state: Mutex<State>,
}

impl CircuitBreaker for A2ACircuitBreaker {
    fn check(&self) -> Result<(), BreakerError> {
        // oxios's policy: open after 12 failures, half-open after 30s
        /* ... */
    }
    fn record_success(&self) { /* oxios's recovery rules */ }
    fn record_failure(&self) { /* oxios's trip rules */ }
}
```

### 3.3 Why this is stable

- `CircuitBreaker` has **3 methods**. Adding a method is a breaking change for
  any consumer impl, so new methods go through the deprecation window (≥1
  release with `#[deprecated]`).
- `DefaultCircuitBreaker` can change thresholds, state machine, internal fields
  freely — consumers who impl the trait don't see those changes.
- `BreakerError` is `#[non_exhaustive]` from creation, so new variants don't
  break consumer `match` arms (consumers MUST have a catch-all `_ =>`).

---

## 4. MemoryStore Three-Layer Design (R6 §5.3)

The apparent duplication between `MemoryStore` and `MemoryBackend` is **by
design** and not a bug to collapse:

| Layer | Owner | Purpose |
|---|---|---|
| `MemoryStore` (SDK port) | SDK | Storage contract (`store` / `recall` / `forget`) — the consumer-facing port |
| `MemoryBackend` (oxicode-agent) | SDK | Agent-tool-facing interface (`store` / `search` / `list` / `delete`) |
| `PortMemoryBackend` | SDK | Adapter: bridges `MemoryBackend` → `MemoryStore` port |

Collapsing these would force every consumer into the SDK's storage model. The
three-layer design lets a consumer:

1. **Implement `MemoryStore` (the port)** and get the agent's memory tools for
   free via `PortMemoryBackend` — the consumer writes only storage code, the
   SDK wires up the tool surface.
2. **Implement `MemoryBackend` directly** for a custom tool-facing interface
   (e.g. oxios's MCP-shaped memory API) — the consumer owns the tool surface
   and doesn't go through the SDK port.

The SDK ships **both** because they serve different composition roots:

- `oxicode-cli` uses option (1): `FileMemoryStore` or `InMemoryMemoryStore` impls
  the port, and the agent tools call them through `PortMemoryBackend`.
- `oxios` uses option (2): its MCP-shaped memory service implements
  `MemoryBackend` directly because its tool surface differs from the SDK's.

This is a stable design. No code change is planned; this section exists to
document the ownership so neither side accidentally collapses a layer.

`PortMemoryBackend` itself is initially `#[unstable]` (recently added adapter
surface); it graduates to `#[stable]` after it proves useful in production.

---

## 5. `ToolError` Stability Note

`ToolError` in `oxicode-agent/src/tools.rs` is:

```rust
pub type ToolError = String;
```

This is **stable by construction** — it is a type alias for `String`, not an
enum with variants that could be silently broken.

Why this matters:

- There are no `ToolError::SomeVariant` arms in consumer code to break. A
  consumer matches on `Err(msg)` and inspects `msg` as a string. The Rust
  compiler guarantees `ToolError` is always exactly `String`.
- We do not need `#[non_exhaustive]` (that's for enums). We do not need a
  deprecation window for any change to `ToolError` — there is no change to make.
- The cost is no typed-error pattern matching. Consumers that want structured
  errors should compose their own error type on top of the message string, or
  migrate to a typed error port (separate design).

This note is here so future maintainers don't "fix" `ToolError` by turning it
into an enum without realizing they've just introduced a stability surface that
every consumer must track.

For comparison, the typed error types in the SDK (`SdkError`, `ProviderError`,
`BreakerError`, `McpError`) are enums that **do** carry a stability commitment:

- New variants can be added freely (they're `#[non_exhaustive]`).
- Existing named variants are **frozen** — semantic changes to an existing
  variant require a rename + deprecation of the old name.
- Consumers MUST have a catch-all arm (`_ =>`) in their matches.

See `docs/release-process.md` for the full variant-stability policy.

---

## 6. Where This Contract Is Enforced

The four enforcement mechanisms below are listed in the order consumers will
encounter them (and in order of compile-time strictness, from weakest to
strongest). Each is a layer of defense, not a substitute for the others.

- **Tier annotations** (`oxicode-api-stability`): every public symbol gets
  `#[stable]`, `#[unstable]`, or `#[internal]`. These render as colored
  badges in `cargo doc` and are **discoverability aids, not compile signals**
  — a proc-macro attribute cannot register a custom lint, so consumers cannot
  turn on a `#![warn(unstable_used)]` that fires on tier annotations. The
  badge exists to make the API surface self-documenting; it does not gate
  builds. **This is a scope-narrowing from the original R3 proposal** (see
  `docs/superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md`
  §4.1 for the original spec). The real machine-enforced signals are the
  `#[cfg(feature = "...")]` gates, the deprecation convention, and the
  `cargo-public-api` CI gate below.
- **Deprecation convention** (`docs/release-process.md`): a public symbol
  marked for removal gets ≥1 release of `#[deprecated(since, note)]` (emitted
  natively by the `oxicode-api-stability::deprecated` macro) with a migration
  path before physical removal. During the window the API signature and
  semantics are frozen. This is the first **compile signal** a consumer
  hits.
- **CHANGELOG enforcement** (R1): every root-level `pub` symbol removal,
  signature change, or semantic change MUST appear under `## Breaking` with
  full symbol path, replacement API, deprecation window, and known affected
  consumers. The `.github/workflows/api-diff.yml` job captures the public API
  surface on every PR; the gate is currently observational and a future
  iteration will diff against `main` and fail the build on undocumented
  removals.
- **Lint policy** (R4): library crates deny `clippy::unwrap_used`,
  `clippy::expect_used`, and `clippy::panic` outside `#[cfg(test)]` so the
  SDK can't ship a panicking reference impl that consumers would silently
  rely on. Test code keeps its idiomatic match-arm panics via an expanded
  `cfg_attr(test, allow(...))` list.

### 6.1 Why the tier annotations alone are insufficient

The original R3 spec asked for a consumer-enforceable signal — oxios turns
on `#![warn(unstable_used)]` and its build fails if it builds on an
unstable SDK symbol. That intent is correct; the implemented mechanism
(doc badges) does not deliver it. The **real** gates consumers have today
are:

1. `#[cfg(feature = "...")]` on the SDK side (e.g. `oxicode-sdk` re-exports
   `BrowseTool` / `BrowserEngine` only with `oxicode-sdk = { features =
   ["unstable"] }`). A consumer that doesn't enable the feature cannot
   accidentally build on it.
2. `#[deprecated(since, note)]` from the `oxicode-api-stability::deprecated`
   macro, which the compiler treats as a regular deprecation warning. A
   consumer that ignores the warning keeps the old behavior; a consumer
   that turns on `#![deny(deprecated)]` will fail to compile.
3. The `cargo-public-api` CI gate (observational today, enforcing in a
   future iteration).

When you add a tier annotation, ask: **does the consumer have a way to
turn this signal into a compile failure?** If not, the annotation is
purely cosmetic. The mechanism that *does* work is a `#[cfg(feature)]`
gate plus a CHANGELOG `## Breaking` entry.

---

## 7. References

- [Spec §3.1 R0/R5 — Ownership Contract](superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md#31-r0r5--ownership-contract)
- [Spec §5.1 R6 — CircuitBreaker](superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md#51-circuitbreaker-new)
- [Spec §5.3 — MemoryStore reconciliation](superpowers/specs/2026-08-01-sdk-stability-ownership-program-design.md#53-memorystore-reconciliation)
- [`docs/PORT_GUIDE.md`](PORT_GUIDE.md) — the 15 port traits and reference impls
- [`docs/release-process.md`](release-process.md) — deprecation window, CHANGELOG `## Breaking` convention
- [`AGENTS.md`](../AGENTS.md) — Port System section (cross-reference target)
