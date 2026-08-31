# OMP-Compatible Behavior Packs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task (tasks 1–6 are tightly coupled on shared evolving types — inline execution, not subagent-per-task). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a behavior-pack API in `oxicode-sdk` (`oxicode_sdk::behavior`, feature `behavior`), the first reference pack `coding-omp-v1`, and CLI consumption — with an honest OMP compatibility ledger and deterministic fixture scenarios in CI.

**Architecture:** Declarative pack (descriptors + extension specs + ledger) resolved by `BehaviorPackResolver`, installed through a host-controlled `BehaviorToolInstaller` interception point. Canonical tools are the existing `oxicode-agent` tool implementations constructed by pack factories. Three future runtime contracts (`ShellSession`, `EvalKernel`, `DebugService`) are declared in `oxicode-agent/src/runtime/` with no implementations — the ledger records shell/eval/DAP as `Unavailable` and LSP/TTSR/delegation as `Partial`.

**Tech Stack:** Rust 2024, workspace crates `oxicode-agent` / `oxicode-sdk` / `oxicode-cli`, `oxicode-hashline` (SnapshotStore), tokio, serde, cargo-nextest.

**Spec:** `docs/designs/2026-08-31-omp-compatible-behavior-pack-design.md` (read it first; this plan argues from it).

## Global Constraints

- No new workspace crate (spec "Non-goals"; placement table in spec).
- New SDK surface is feature-gated: `[features] behavior = []` in `oxicode-sdk/Cargo.toml`, added to the `unstable` umbrella. CLI enables `oxicode-sdk/behavior`.
- `oxicode-sdk` has `#![warn(missing_docs)]` — every public item in `behavior/` needs a doc comment. Non-test SDK code denies `unwrap/expect/panic` (`#![cfg_attr(not(test), deny(...))]` at crate root) — use `ok_or`/`?` in src; tests may unwrap.
- OMP compatibility pin (ledger `target`): `omp@v18.0.11 (can1357/oh-my-pi@b8ce33a)`.
- Ledger initial statuses (spec table): read-write-search Equivalent, hashline-anchors Equivalent, lsp Partial, ttsr Partial, delegation Partial, persistent-shell Unavailable, persistent-eval Unavailable, dap-debugging Unavailable, host-product-tools NotApplicable.
- Do NOT implement persistent shell / eval kernels / DAP in this plan (spec migration step 4 is a follow-up; acceptance criteria require only honest reporting).
- Conventions: `parking_lot` locks, `tempfile` in tests, module pattern `mod.rs` re-exports, `#[oxicode_stable]/#[oxicode_unstable]` per lib.rs conventions, edition-2024 (let-chains OK).
- Gates after every task: `cargo fmt --all`, `cargo clippy -p <touched-crates> --all-targets -- -D warnings`, targeted `cargo nextest run -p <crate>`. Full workspace gates in the final task.
- Tool parameter schemas verified for fixtures: read `{"path"}`; edit `{"path", "old_text", "new_text", "expected_hash"}` (hashline `patch` mode also exists); bash `{"command"}`; subagent `{"agent", "task"}`. Read output carries a `[path#TAG]` header where TAG is the 16-hex content hash (read.rs:179-186).
- `EditTool` stale-anchor semantics (edit.rs:132-148): when `expected_hash` is set and the file's current content hash differs, it returns `Ok(EditOutput { applied: false, ... })` with a conflict message — not a hard error.
- Agent-loop wiring of services (verified): `AgentConfig.{snapshot_store,lsp,ttsr_engine,subagent_runner,memory,todo,url_resolver,session_id}` → `AgentLoopConfig` → `ToolContext` (agent.rs:691-695, agent_loop/mod.rs:283-287).
- CLI today does NOT wire a hashline snapshot store anywhere (grep-verified zero callsites) — the pack adds it.
- Prompt layer application point: `AgentConfig.system_prompt: Option<String>` (config.rs:196-197).

## File Structure

| File | Responsibility |
|---|---|
| `oxicode-agent/src/runtime/mod.rs` (new) | Service contracts: `ShellSession`, `EvalKernel`, `DebugService` + output types. No impls. |
| `oxicode-agent/src/lib.rs` | `pub mod runtime;` + re-exports. |
| `oxicode-sdk/src/behavior/mod.rs` (new) | Module root, re-exports, crate docs pointer. |
| `oxicode-sdk/src/behavior/types.rs` (new) | Core data types: ids, classes, scopes, descriptors, extension specs, prompt layers, `BehaviorPack`, `ToolFactory`, errors. |
| `oxicode-sdk/src/behavior/ledger.rs` (new) | `FeatureStatus`, `LedgerEntry`, `CompatibilityContract`, rollup. |
| `oxicode-sdk/src/behavior/installer.rs` (new) | `BehaviorSessionServices`, `BehaviorToolInstaller`, `AgentConfigPatch`, `InstalledBehaviorManifest`, `DegradationRecord`, `BehaviorPack::install()`. |
| `oxicode-sdk/src/behavior/resolver.rs` (new) | `BehaviorPackResolver`, `ResolvedBehavior`, resolution/replacement/duplicate rules. |
| `oxicode-sdk/src/behavior/packs/mod.rs` + `packs/coding_omp_v1.rs` (new) | `coding-omp-v1`: 16 descriptors, factories, prompt layer, embedded ledger. |
| `oxicode-sdk/src/lib.rs` | `pub mod behavior;` behind feature + re-exports. |
| `oxicode-sdk/Cargo.toml` | `behavior` feature + unstable umbrella. |
| `oxicode-cli/Cargo.toml` | `oxicode-sdk` features += `behavior`; add `oxicode-hashline` dep. |
| `oxicode-cli/src/behavior.rs` (new) | `BehaviorComposition`, `CliToolInstaller`, `install_coding_omp_v1()`. |
| `oxicode-cli/src/bootstrap.rs` | Call installer after `register_builtin_tools`; startup tracing line. |
| `oxicode-cli/src/lib.rs` | `App` field + `App::from_oxicode` param; apply patch (snapshot_store, prompt layers). |
| `oxicode-sdk/tests/behavior/main.rs` + `common/mod.rs` + scenario modules (new) | Fixture harness + scenario families. |
| `CHANGELOG.md`, `AGENTS.md`, design doc | Docs update (final task). |

---

### Task 1: Runtime service contracts in oxicode-agent

**Files:**
- Create: `oxicode-agent/src/runtime/mod.rs`
- Modify: `oxicode-agent/src/lib.rs` (module decl + re-exports near the other `pub use tools::…` block)

**Interfaces:**
- Produces (used by Tasks 2–5): `oxicode_agent::runtime::{ShellSession, ShellOutput, EvalKernel, EvalLanguage, EvalOutput, DebugService}` — all object-safe traits, `Send + Sync + Debug`.

- [ ] **Step 1: Write failing shape test** — append to `oxicode-agent/src/runtime/mod.rs` (a compile-level test IS the spec for object safety):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct NoopShell;
    #[async_trait::async_trait]
    impl ShellSession for NoopShell {
        async fn execute(&self, _cmd: &str, _t: std::time::Duration) -> Result<ShellOutput, String> {
            Ok(ShellOutput { stdout: String::new(), stderr: String::new(), exit_code: 0, truncated: false })
        }
        fn cancel(&self) {}
        async fn reset(&self) -> Result<(), String> { Ok(()) }
    }

    #[test]
    fn runtime_contracts_are_object_safe() {
        let shell: Arc<dyn ShellSession> = Arc::new(NoopShell);
        assert_eq!(shell.execute("x", std::time::Duration::from_secs(0)).await.unwrap().exit_code, 0);
    }
}
```

(The test is `async`-capable via `#[tokio::test]` — wrap accordingly; execute returns a future.)

- [ ] **Step 2: Implement** `oxicode-agent/src/runtime/mod.rs`:

```rust
//! Shared stateful coding runtime contracts.
//!
//! These traits declare the persistent shell / eval kernel / debug service
//! capabilities required by the `coding-omp-v1` behavior pack
//! (see `docs/designs/2026-08-31-omp-compatible-behavior-pack-design.md`,
//! "Coding extensions"). No implementations ship yet — hosts and future
//! pack extensions implement them. Until then the SDK behavior installer
//! reports those extensions as degraded/unavailable.

use async_trait::async_trait;
use std::time::Duration;

/// Output of one command in a persistent shell session.
#[derive(Debug, Clone, Default)]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// True when the host applied an output bound and elided bytes.
    pub truncated: bool,
}

/// Persistent shell session contract ("Shell session" extension).
///
/// Required behavior: persistent command environment across calls,
/// cancellation, bounded output, explicit reset (design table row 3).
#[async_trait]
pub trait ShellSession: Send + Sync + std::fmt::Debug {
    /// Execute `command` in the persistent environment.
    async fn execute(&self, command: &str, timeout: Duration) -> Result<ShellOutput, String>;
    /// Cancel the currently running command, if any.
    fn cancel(&self);
    /// Reset to a fresh environment; working directory returns to the workspace root.
    async fn reset(&self) -> Result<(), String>;
}

/// Language of an eval kernel session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalLanguage {
    Python,
    JavaScript,
}

/// Output of one persistent-kernel evaluation.
#[derive(Debug, Clone, Default)]
pub struct EvalOutput {
    pub result: String,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub truncated: bool,
}

/// Persistent eval kernel contract ("Eval kernel" extension).
///
/// Required behavior: persistent Python/Bun state across calls, bounded
/// execution, explicit reset (design table row 4).
#[async_trait]
pub trait EvalKernel: Send + Sync + std::fmt::Debug {
    fn language(&self) -> EvalLanguage;
    async fn execute(&self, code: &str, timeout: Duration) -> Result<EvalOutput, String>;
    async fn reset(&self) -> Result<(), String>;
}

/// Debug service contract ("Debug service" extension): a real DAP session
/// lifecycle (design table row 5). Requests use DAP command names
/// (`setBreakpoints`, `continue`, `next`, `variables`, ...) with raw JSON
/// payloads — typed methods arrive with the first real implementation.
#[async_trait]
pub trait DebugService: Send + Sync + std::fmt::Debug {
    /// Launch or attach a session per the DAP launch/attach config; returns a session id.
    async fn start(&self, config: &serde_json::Value) -> Result<String, String>;
    /// Issue a DAP request against the session; returns the raw response payload.
    async fn request(&self, session: &str, command: &str, args: &serde_json::Value) -> Result<serde_json::Value, String>;
    /// Terminate the session and clean up the adapter process.
    async fn terminate(&self, session: &str) -> Result<(), String>;
}
```

- [ ] **Step 3: Wire module** — in `oxicode-agent/src/lib.rs` add `pub mod runtime;` next to `pub mod tools;` and re-export near the existing re-export block (plain `pub use`, no stability attr, mirroring `pub use tools::ask::{AskBridge, AskTool}`):

```rust
pub use runtime::{DebugService, EvalKernel, EvalLanguage, EvalOutput, ShellOutput, ShellSession};
```

- [ ] **Step 4: Verify** — `cargo nextest run -p oxicode-agent runtime` PASS; `cargo clippy -p oxicode-agent --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** — `feat: declare persistent shell/eval/debug runtime contracts`

### Task 2: SDK behavior module scaffolding + ledger types

**Files:**
- Create: `oxicode-sdk/src/behavior/mod.rs`, `types.rs`, `ledger.rs`
- Modify: `oxicode-sdk/src/lib.rs` (feature-gated `pub mod behavior;` + re-exports), `oxicode-sdk/Cargo.toml` (feature)

**Interfaces:**
- Consumes: Task 1 runtime traits.
- Produces (Tasks 3–5 build on these exact names): `BehaviorPackId`, `ToolImplementationId`, `CapabilityClass`, `SideEffectClass`, `ToolStateScope`, `PortRequirement`, `PortRequirementKind`, `PromptLayerSpec`, `RuntimeExtensionSpec`, `ExtensionKind`, `ExtensionScope`, `BehaviorToolDescriptor`, `ToolFactory`, `BehaviorPack`, `FeatureStatus`, `LedgerEntry`, `CompatibilityContract`, `BehaviorInstallError`.

- [ ] **Step 1: Feature gate** — `oxicode-sdk/Cargo.toml`: add `behavior = []` to `[features]` and `"behavior"` to the `unstable` umbrella list.
- [ ] **Step 2: lib.rs wiring**:

```rust
/// Behavior packs — portable, versioned coding-behavior contracts.
#[cfg(feature = "behavior")]
pub mod behavior;
```

plus (also `#[cfg(feature = "behavior")]`, with `#[oxicode_unstable(feature = "behavior")]`):

```rust
#[cfg(feature = "behavior")]
#[oxicode_unstable(feature = "behavior")]
pub use behavior::{
    BehaviorInstallError, BehaviorPack, BehaviorPackId, BehaviorToolDescriptor, CapabilityClass,
    CompatibilityContract, ExtensionKind, ExtensionScope, FeatureStatus, LedgerEntry,
    PortRequirement, PortRequirementKind, PromptLayerSpec, SideEffectClass, ToolFactory,
    ToolImplementationId, ToolStateScope, RuntimeExtensionSpec,
};
```

- [ ] **Step 3: `ledger.rs`** — types + rollup:

```rust
use serde::{Deserialize, Serialize};

/// Honest OMP-equivalence status of one feature area (spec "OMP compatibility contract").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStatus {
    Equivalent,
    Partial,
    Unavailable,
    NotApplicable,
}

impl FeatureStatus {
    /// Ordering used by rollup: Unavailable is the worst claim.
    pub fn rank(&self) -> u8 {
        match self {
            FeatureStatus::NotApplicable => 0,
            FeatureStatus::Equivalent => 1,
            FeatureStatus::Partial => 2,
            FeatureStatus::Unavailable => 3,
        }
    }
    /// Worst-of two statuses (NotApplicable never drags the rollup).
    pub fn worst(a: FeatureStatus, b: FeatureStatus) -> FeatureStatus {
        if a.rank() >= b.rank() { a } else { b }
    }
}

/// One ledger row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub feature: String,
    pub status: FeatureStatus,
    /// Scenario ids establishing the claim (empty for Unavailable).
    pub evidence: Vec<String>,
    pub notes: String,
}

/// Machine-readable compatibility contract shipped with a pack release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityContract {
    /// Compatibility target, pinned by release or commit (e.g. `omp@v18.0.11 (...)`).
    pub target: String,
    pub entries: Vec<LedgerEntry>,
}

impl CompatibilityContract {
    /// Worst status across entries; `Unavailable` when empty (conservative).
    pub fn rollup(&self) -> FeatureStatus {
        self.entries
            .iter()
            .fold(FeatureStatus::Unavailable, |acc, e| FeatureStatus::worst(acc, e.status))
    }

    /// Merge for multi-pack resolution: targets joined, entries concatenated.
    pub fn merge(&self, other: &CompatibilityContract) -> CompatibilityContract {
        let target = if self.target.is_empty() {
            other.target.clone()
        } else {
            format!("{} + {}", self.target, other.target)
        };
        let mut entries = self.entries.clone();
        entries.extend(other.entries.iter().cloned());
        CompatibilityContract { target, entries }
    }
}
```

Unit tests in `ledger.rs` (`#[cfg(test)]`): rollup of [Equivalent, Partial, Unavailable] == Unavailable; NotApplicable ignored (rollup of [NotApplicable] alone == Unavailable — empty-of-real is conservative); merge concatenates.

- [ ] **Step 4: `types.rs`** — ids, classes, descriptor, extension spec, pack, errors:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use oxicode_agent::AgentTool;

use super::ledger::CompatibilityContract;

/// Stable behavior-pack identifier (e.g. `coding-omp-v1`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BehaviorPackId(pub String);

impl BehaviorPackId {
    pub fn coding_omp_v1() -> Self { BehaviorPackId("coding-omp-v1".to_string()) }
}

impl std::fmt::Display for BehaviorPackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

/// Product-facing implementation identity — distinct from the model-visible tool name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolImplementationId(pub String);

impl std::fmt::Display for ToolImplementationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

/// Coarse capability class — advisory input to host policy, never authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityClass { FsRead, FsWrite, Search, Process, Network, Lsp, Memory, Delegation, Ui }

/// Side-effect classification — advisory input to host policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectClass { ReadOnly, Mutating, Networked, ProcessSpawning }

/// Where a tool's mutable state lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStateScope { Stateless, HashlineSession, ShellSession, EvalKernel, DebugTarget, Workspace }

/// A port/service a tool needs at execution time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRequirementKind {
    HashlineSnapshotStore, LspProvider, TtsrEngine, UrlResolver, SubagentRunner,
    MemoryBackend, TodoStateProvider, ShellSession, EvalKernel, DebugService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRequirement {
    pub kind: PortRequirementKind,
    pub required: bool,
}

/// Model-facing prompt fragment the pack asks the host to install.
#[derive(Debug, Clone)]
pub struct PromptLayerSpec { pub id: String, pub body: String }

/// Runtime extension kinds declared by packs (design "Coding extensions" table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtensionKind { HashlineState, LspHost, ShellSession, EvalKernel, DebugService, TtsrEngine, Delegation }

impl ExtensionKind {
    /// Stable slug used in manifests and degradation records.
    pub fn slug(&self) -> &'static str {
        match self {
            ExtensionKind::HashlineState => "hashline-state",
            ExtensionKind::LspHost => "lsp-host",
            ExtensionKind::ShellSession => "shell-session",
            ExtensionKind::EvalKernel => "eval-kernel",
            ExtensionKind::DebugService => "debug-service",
            ExtensionKind::TtsrEngine => "ttsr-engine",
            ExtensionKind::Delegation => "delegation",
        }
    }
    /// The port kind this extension backs, for affected-tool computation.
    pub fn port(&self) -> PortRequirementKind {
        match self {
            ExtensionKind::HashlineState => PortRequirementKind::HashlineSnapshotStore,
            ExtensionKind::LspHost => PortRequirementKind::LspProvider,
            ExtensionKind::TtsrEngine => PortRequirementKind::TtsrEngine,
            ExtensionKind::Delegation => PortRequirementKind::SubagentRunner,
            ExtensionKind::ShellSession => PortRequirementKind::ShellSession,
            ExtensionKind::EvalKernel => PortRequirementKind::EvalKernel,
            ExtensionKind::DebugService => PortRequirementKind::DebugService,
        }
    }
}

/// Declared lifetime of an extension requirement (one variant per design-table scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionScope {
    SessionWorkspace,
    Workspace,
    SessionLanguage,
    WorkspaceDebugTarget,
    Turn,
    ChildAgentLifecycle,
}

#[derive(Debug, Clone)]
pub struct RuntimeExtensionSpec { pub kind: ExtensionKind, pub scope: ExtensionScope, pub required: bool }

/// Model-visible tool contract: both the implementation identity and the exposed name.
#[derive(Debug, Clone)]
pub struct BehaviorToolDescriptor {
    pub id: ToolImplementationId,
    pub exposed_name: String,
    pub capability: CapabilityClass,
    pub side_effect: SideEffectClass,
    pub required_ports: Vec<PortRequirement>,
    pub state_scope: ToolStateScope,
    /// Mirrors `AgentTool::essential`: a required tool the host cannot skip.
    pub essential: bool,
    /// When set, this descriptor explicitly replaces the named implementation (overlay rule).
    pub replaces: Option<ToolImplementationId>,
}

impl BehaviorToolDescriptor {
    pub fn new(id: &str, exposed_name: &str) -> Self {
        BehaviorToolDescriptor {
            id: ToolImplementationId(id.to_string()),
            exposed_name: exposed_name.to_string(),
            capability: CapabilityClass::FsRead,
            side_effect: SideEffectClass::ReadOnly,
            required_ports: Vec::new(),
            state_scope: ToolStateScope::Stateless,
            essential: false,
            replaces: None,
        }
    }
    pub fn capability(mut self, c: CapabilityClass) -> Self { self.capability = c; self }
    pub fn side_effect(mut self, s: SideEffectClass) -> Self { self.side_effect = s; self }
    pub fn state_scope(mut self, s: ToolStateScope) -> Self { self.state_scope = s; self }
    pub fn port(mut self, kind: PortRequirementKind, required: bool) -> Self {
        self.required_ports.push(PortRequirement { kind, required }); self
    }
    pub fn essential(mut self) -> Self { self.essential = true; self }
    pub fn replaces(mut self, id: &str) -> Self { self.replaces = Some(ToolImplementationId(id.to_string())); self }
}

/// Constructs one canonical tool from the host service inventory.
pub type ToolFactory = Arc<dyn Fn(&super::installer::BehaviorSessionServices) -> Result<Arc<dyn AgentTool>, BehaviorInstallError> + Send + Sync>;

/// A versioned, portable behavior contract (design "Public model").
pub struct BehaviorPack {
    pub id: BehaviorPackId,
    pub schema_version: u32,
    pub prompt_layers: Vec<PromptLayerSpec>,
    pub extensions: Vec<RuntimeExtensionSpec>,
    /// Declaration order = install order.
    pub tools: Vec<BehaviorToolDescriptor>,
    pub compatibility: CompatibilityContract,
    pub(crate) factories: HashMap<ToolImplementationId, ToolFactory>,
}

/// Failure modes across resolve + install.
#[derive(Debug, Clone)]
pub enum BehaviorInstallError {
    UnknownPack(BehaviorPackId),
    DuplicatePackId(BehaviorPackId),
    UnsupportedSchemaVersion { pack: BehaviorPackId, got: u32 },
    DuplicateToolImplementation { pack: BehaviorPackId, id: ToolImplementationId },
    DuplicateExposedName { exposed_name: String, existing: ToolImplementationId, incoming: ToolImplementationId },
    RequiredExtensionMissing { pack: BehaviorPackId, kind: ExtensionKind },
    RequiredServiceMissing { descriptor: ToolImplementationId, kind: PortRequirementKind },
    FactoryFailed { descriptor: ToolImplementationId, reason: String },
    HostRejected { descriptor: ToolImplementationId, exposed_name: String, reason: String },
}

impl std::fmt::Display for BehaviorInstallError { /* terse human text per variant */ }
impl std::error::Error for BehaviorInstallError {}
```

`BehaviorPack` impl:

```rust
impl BehaviorPack {
    /// Schema version 1; `target` pins the compatibility baseline.
    pub fn new(id: BehaviorPackId, target: String) -> Self {
        BehaviorPack { id, schema_version: 1, prompt_layers: Vec::new(), extensions: Vec::new(), tools: Vec::new(), compatibility: CompatibilityContract { target, entries: Vec::new() }, factories: HashMap::new() }
    }
    pub fn with_prompt_layer(mut self, spec: PromptLayerSpec) -> Self { self.prompt_layers.push(spec); self }
    pub fn with_extension(mut self, spec: RuntimeExtensionSpec) -> Self { self.extensions.push(spec); self }
    /// Register a descriptor together with its factory. Duplicate ids error.
    pub fn with_tool(mut self, descriptor: BehaviorToolDescriptor, factory: ToolFactory) -> Result<Self, BehaviorInstallError> {
        if self.factories.contains_key(&descriptor.id) {
            return Err(BehaviorInstallError::DuplicateToolImplementation { pack: self.id.clone(), id: descriptor.id.clone() });
        }
        self.factories.insert(descriptor.id.clone(), factory);
        self.tools.push(descriptor);
        Ok(self)
    }
    pub(crate) fn factory_for(&self, id: &ToolImplementationId) -> Option<ToolFactory> { self.factories.get(id).cloned() }
}
```

- [ ] **Step 5: `mod.rs`** — module docs (quote spec intent), `pub mod ledger; pub mod types;` + re-export both modules' public items at `behavior::` root (flat namespace as wired in lib.rs Step 2).
- [ ] **Step 6: Verify** — `cargo nextest run -p oxicode-sdk --features behavior behavior` PASS (unit tests in ledger/types); clippy `-p oxicode-sdk --features behavior --all-targets -- -D warnings` clean. `Display for BehaviorInstallError` must not use unwrap/panic.
- [ ] **Step 7: Commit** — `feat(sdk): behavior pack core types and compatibility ledger`

### Task 3: Services, installer interception, install()

**Files:**
- Create: `oxicode-sdk/src/behavior/installer.rs`
- Modify: `oxicode-sdk/src/behavior/mod.rs` (wire module + re-export)

**Interfaces:**
- Consumes: Task 2 types, Task 1 runtime traits, `oxicode_hashline::SnapshotStore`, agent capability traits (`LspProvider`, `MemoryBackend`, `SubagentRunner`, `TodoStateProvider`, `UrlResolver` — all under `oxicode_agent::tools`), `oxicode_agent::agent_loop::ttsr::TtsrEngine`.
- Produces: `BehaviorSessionServices` (+ `new`/`with_*`/`port_available`/`extension_available`), `BehaviorToolInstaller`, `AgentConfigPatch`, `InstalledToolRecord`, `DegradationReason`, `DegradationRecord`, `InstalledBehaviorManifest`, `BehaviorPack::install()`.

- [ ] **Step 1: Failing test** (in `installer.rs` `#[cfg(test)]`): a fake pack (2 tools: one `essential` needing `HashlineSnapshotStore` **required**, one optional needing `ShellSession` optional) + `RecordingInstaller` that captures `(exposed_name, tool.name())`:
  - with snapshot store present and shell None → Ok manifest: 1 installed, 1 degradation (`ServiceUnavailable(ShellSession)`), prompt layer id recorded;
  - without snapshot store → `Err(RequiredServiceMissing)` (essential tool hard-fails);
  - host installer returning Err for the optional tool → degradation `HostRejected`, not a pack failure.
- [ ] **Step 2: Implement** — key pieces (complete the straightforward parts in the same style):

```rust
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use oxicode_agent::agent_loop::ttsr::TtsrEngine;
use oxicode_agent::tools::{LspProvider, MemoryBackend, SubagentRunner, TodoStateProvider, UrlResolver};
use oxicode_agent::{AgentTool, ToolRegistry};
use oxicode_hashline::SnapshotStore;

use super::ledger::CompatibilityContract;
use super::types::{BehaviorInstallError, BehaviorPackId, BehaviorToolDescriptor, ExtensionKind, PortRequirementKind, PromptLayerSpec, ToolImplementationId};

/// Per-session service inventory a host offers to the pack installer.
///
/// `None`/empty fields mean the host does not provide the capability; the
/// installer degrades optional dependencies and fails required ones.
#[derive(Clone)]
pub struct BehaviorSessionServices {
    pub workspace_root: PathBuf,
    /// Model-visible tool names the host refuses (mirrors --tools / disabled_tools).
    pub disabled_tools: Vec<String>,
    pub snapshot_store: Option<Arc<dyn SnapshotStore>>,
    pub lsp: Option<Arc<dyn LspProvider>>,
    pub ttsr_engine: Option<Arc<TtsrEngine>>,
    pub url_resolver: Option<Arc<dyn UrlResolver>>,
    pub subagent_runner: Option<Arc<dyn SubagentRunner>>,
    pub memory: Option<Arc<dyn MemoryBackend>>,
    pub todo: Option<Arc<dyn TodoStateProvider>>,
    pub shell_session: Option<Arc<dyn oxicode_agent::runtime::ShellSession>>,
    pub eval_kernels: Vec<Arc<dyn oxicode_agent::runtime::EvalKernel>>,
    pub debug_service: Option<Arc<dyn oxicode_agent::runtime::DebugService>>,
}

impl std::fmt::Debug for BehaviorSessionServices { /* trait objects as "<dyn …>", like ToolContext */ }

impl BehaviorSessionServices {
    pub fn new(workspace_root: PathBuf) -> Self { /* all None/empty */ }

    pub fn with_snapshot_store(mut self, s: Arc<dyn SnapshotStore>) -> Self { self.snapshot_store = Some(s); self }
    pub fn with_disabled_tools(mut self, d: Vec<String>) -> Self { self.disabled_tools = d; self }
    /* analogous with_lsp / with_ttsr_engine / with_subagent_runner / with_todo / with_memory / with_url_resolver / with_shell_session / with_eval_kernel / with_debug_service */

    pub fn port_available(&self, kind: PortRequirementKind) -> bool {
        match kind {
            PortRequirementKind::HashlineSnapshotStore => self.snapshot_store.is_some(),
            PortRequirementKind::LspProvider => self.lsp.is_some(),
            PortRequirementKind::TtsrEngine => self.ttsr_engine.is_some(),
            PortRequirementKind::UrlResolver => self.url_resolver.is_some(),
            PortRequirementKind::SubagentRunner => self.subagent_runner.is_some(),
            PortRequirementKind::MemoryBackend => self.memory.is_some(),
            PortRequirementKind::TodoStateProvider => self.todo.is_some(),
            PortRequirementKind::ShellSession => self.shell_session.is_some(),
            PortRequirementKind::EvalKernel => !self.eval_kernels.is_empty(),
            PortRequirementKind::DebugService => self.debug_service.is_some(),
        }
    }

    pub fn extension_available(&self, kind: ExtensionKind) -> bool { self.port_available(kind.port()) }
}

/// Host-controlled interception point. The pack never touches a
/// `ToolRegistry` behind the host's back (design: "The pack must never call
/// `ToolRegistry::register*` behind the host's back").
pub trait BehaviorToolInstaller: Send {
    /// Install (possibly wrapped) tool for `descriptor`. Err = host rejection.
    fn install(&mut self, descriptor: &BehaviorToolDescriptor, tool: Arc<dyn AgentTool>) -> Result<(), BehaviorInstallError>;
}

/// The pack's request for standard AgentConfig fields. The host validates
/// against policy before applying — this is not mutable access.
#[derive(Clone, Default)]
pub struct AgentConfigPatch {
    pub snapshot_store: Option<Arc<dyn SnapshotStore>>,
    pub lsp: Option<Arc<dyn LspProvider>>,
    pub ttsr_engine: Option<Arc<TtsrEngine>>,
    pub url_resolver: Option<Arc<dyn UrlResolver>>,
    pub subagent_runner: Option<Arc<dyn SubagentRunner>>,
    pub memory: Option<Arc<dyn MemoryBackend>>,
    pub todo: Option<Arc<dyn TodoStateProvider>>,
    pub prompt_layers: Vec<PromptLayerSpec>,
}

#[derive(Debug, Clone)]
pub struct InstalledToolRecord { pub descriptor: ToolImplementationId, pub exposed_name: String }

#[derive(Debug, Clone)]
pub enum DegradationReason {
    ServiceUnavailable(PortRequirementKind),
    ExtensionUnavailable(ExtensionKind),
    DisabledByHost,
    HostRejected { tool: String, reason: String },
}

#[derive(Debug, Clone)]
pub struct DegradationRecord {
    /// Feature slug (`ExtensionKind::slug()` or "<extension>-tool" for per-tool cases).
    pub feature: String,
    pub reason: DegradationReason,
    pub affected_tools: Vec<String>,
}

/// Result of a successful pack install — actual tools, degradations,
/// prompt layers, and the resolved compatibility contract. Distinct from
/// `oxicode_sdk::lifecycle::ToolManifest` (lifecycle registry snapshots).
#[derive(Debug, Clone)]
pub struct InstalledBehaviorManifest {
    pub packs: Vec<BehaviorPackId>,
    pub schema_version: u32,
    pub tools: Vec<InstalledToolRecord>,
    pub degraded: Vec<DegradationRecord>,
    pub prompt_layers: Vec<String>,
    pub compatibility: CompatibilityContract,
}

impl InstalledBehaviorManifest {
    /// Worst ledger status — what the host may advertise.
    pub fn compatibility_level(&self) -> super::ledger::FeatureStatus { self.compatibility.rollup() }
}
```

`BehaviorPack::install()` (put in `installer.rs` as `impl BehaviorPack` to keep types.rs data-only; algorithm is normative):

```rust
impl BehaviorPack {
    /// Create canonical tools and hand each to the host installer.
    ///
    /// Order: declaration order. Per tool: host-disabled (non-essential) →
    /// degradation; required port missing → essential ? Err : degradation;
    /// factory → Err propagates; installer Err → essential ? Err : degradation.
    pub fn install(
        &self,
        services: &BehaviorSessionServices,
        installer: &mut dyn BehaviorToolInstaller,
    ) -> Result<InstalledBehaviorManifest, BehaviorInstallError> {
        if self.schema_version != 1 {
            return Err(BehaviorInstallError::UnsupportedSchemaVersion { pack: self.id.clone(), got: self.schema_version });
        }
        let disabled: HashSet<&str> = services.disabled_tools.iter().map(String::as_str).collect();
        let mut manifest = InstalledBehaviorManifest {
            packs: vec![self.id.clone()], schema_version: self.schema_version,
            tools: Vec::new(), degraded: Vec::new(),
            prompt_layers: self.prompt_layers.iter().map(|l| l.id.clone()).collect(),
            compatibility: self.compatibility.clone(),
        };
        // Required extensions first — fail before any tool is installed.
        for ext in &self.extensions {
            if ext.required && !services.extension_available(ext.kind) {
                return Err(BehaviorInstallError::RequiredExtensionMissing { pack: self.id.clone(), kind: ext.kind });
            }
        }
        for d in &self.tools {
            if disabled.contains(d.exposed_name.as_str()) {
                if d.essential {
                    return Err(BehaviorInstallError::HostRejected { descriptor: d.id.clone(), exposed_name: d.exposed_name.clone(), reason: "essential tool disabled by host".to_string() });
                }
                manifest.degraded.push(DegradationRecord { feature: d.exposed_name.clone(), reason: DegradationReason::DisabledByHost, affected_tools: vec![d.exposed_name.clone()] });
                continue;
            }
            let mut missing_required: Option<PortRequirementKind> = None;
            for port in &d.required_ports {
                if port.required && !services.port_available(port.kind) { missing_required = Some(port.kind); break; }
            }
            if let Some(kind) = missing_required {
                if d.essential {
                    return Err(BehaviorInstallError::RequiredServiceMissing { descriptor: d.id.clone(), kind });
                }
                manifest.degraded.push(DegradationRecord { feature: d.exposed_name.clone(), reason: DegradationReason::ServiceUnavailable(kind), affected_tools: vec![d.exposed_name.clone()] });
                continue;
            }
            let factory = self.factory_for(&d.id).ok_or_else(|| BehaviorInstallError::FactoryFailed { descriptor: d.id.clone(), reason: "no factory registered".to_string() })?;
            let tool = factory(services)?;
            match installer.install(d, tool) {
                Ok(()) => manifest.tools.push(InstalledToolRecord { descriptor: d.id.clone(), exposed_name: d.exposed_name.clone() }),
                Err(e) => {
                    if d.essential {
                        return Err(BehaviorInstallError::HostRejected { descriptor: d.id.clone(), exposed_name: d.exposed_name.clone(), reason: e.to_string() });
                    }
                    manifest.degraded.push(DegradationRecord { feature: d.exposed_name.clone(), reason: DegradationReason::HostRejected { tool: d.exposed_name.clone(), reason: e.to_string() }, affected_tools: vec![d.exposed_name.clone()] });
                }
            }
        }
        // Optional, unsatisfied extensions → one degradation each.
        for ext in &self.extensions {
            if !ext.required && !services.extension_available(ext.kind) {
                let affected: Vec<String> = self.tools.iter()
                    .filter(|t| t.required_ports.iter().any(|p| p.kind == ext.kind.port()))
                    .map(|t| t.exposed_name.clone())
                    .collect();
                manifest.degraded.push(DegradationRecord { feature: ext.kind.slug().to_string(), reason: DegradationReason::ExtensionUnavailable(ext.kind), affected_tools: affected });
            }
        }
        Ok(manifest)
    }
}
```

Wait — note the conflict in the design-vs-implementation for the FIRST check: the spec says host may reject a required tool "causing pack resolution to fail before an agent turn begins". The `DisabledByHost` check for essential tools above errs on the side of hard failure. Keep as written (documented).

- [ ] **Step 3: mod.rs wiring** — `pub mod installer;` + re-export its public items at `behavior::` root.
- [ ] **Step 4: Verify** — targeted nextest PASS (all three test cases), clippy clean.
- [ ] **Step 5: Commit** — `feat(sdk): behavior session services and host installer interception`

### Task 4: Resolver, overlays, ResolvedBehavior

**Files:**
- Create: `oxicode-sdk/src/behavior/resolver.rs`
- Modify: `oxicode-sdk/src/behavior/mod.rs`

**Interfaces:**
- Consumes: Tasks 2–3.
- Produces: `BehaviorPackResolver` (`new`, `register`, `coding_omp_v1` placeholder — the real pack lands in Task 5, so this task registers a minimal test pack), `pack(&BehaviorPackId) -> Option<&BehaviorPack>`, `resolve(&[BehaviorPackId], &BehaviorSessionServices) -> Result<ResolvedBehavior, BehaviorInstallError>`; `ResolvedTool`, `ResolvedBehavior` (`packs`, `tools`, `patch`, `prompt_layers`, `degradations`, `compatibility`, `install(services, &mut dyn BehaviorToolInstaller) -> Result<InstalledBehaviorManifest, BehaviorInstallError>`).

- [ ] **Step 1: Failing tests** (`resolver.rs` `#[cfg(test)]`):
  - unknown id → `Err(UnknownPack)`;
  - duplicate exposed name across two packs without `replaces` → `Err(DuplicateExposedName)`;
  - second pack with `replaces: Some("read.file.v1")` → Ok, resolved tools contain exactly one exposed `read`, whose descriptor id is the replacement;
  - `resolve` populates `patch` from services present fields (snapshot store + ttsr engine set → both in patch; prompt layers carried through);
  - `ResolvedBehavior::install` produces a manifest whose `packs` lists all requested ids and whose `compatibility` is the merged contract.
- [ ] **Step 2: Implement**:

```rust
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use super::installer::{AgentConfigPatch, BehaviorSessionServices, BehaviorToolInstaller, DegradationRecord, InstalledBehaviorManifest};
use super::ledger::CompatibilityContract;
use super::types::{BehaviorInstallError, BehaviorPack, BehaviorPackId, BehaviorToolDescriptor, PortRequirementKind, PromptLayerSpec, ToolFactory, ToolImplementationId};

/// Deterministic pack registry and resolution entry point.
#[derive(Default)]
pub struct BehaviorPackResolver {
    packs: BTreeMap<BehaviorPackId, BehaviorPack>,
}

pub struct ResolvedTool {
    pub descriptor: BehaviorToolDescriptor,
    pub factory: ToolFactory,
}

/// Resolution output: validated descriptors + requested config patch.
/// Install happens later, through the host's installer.
pub struct ResolvedBehavior {
    pub packs: Vec<BehaviorPackId>,
    pub tools: Vec<ResolvedTool>,
    pub patch: AgentConfigPatch,
    pub prompt_layers: Vec<PromptLayerSpec>,
    pub degradations: Vec<DegradationRecord>,
    pub compatibility: CompatibilityContract,
}

impl BehaviorPackResolver {
    pub fn new() -> Self { Self::default() }

    /// Register a pack. Duplicate ids error.
    pub fn register(&mut self, pack: BehaviorPack) -> Result<(), BehaviorInstallError> {
        if self.packs.contains_key(&pack.id) {
            return Err(BehaviorInstallError::DuplicatePackId(pack.id));
        }
        self.packs.insert(pack.id.clone(), pack);
        Ok(())
    }

    /// Resolver preloaded with every builtin reference pack.
    pub fn with_builtin_packs() -> Result<Self, BehaviorInstallError> {
        let mut r = Self::new();
        r.register(super::packs::coding_omp_v1::pack())?;
        Ok(r)
    }

    pub fn pack(&self, id: &BehaviorPackId) -> Option<&BehaviorPack> { self.packs.get(id) }

    /// Resolve `requested` packs in request order (deduplicated). Overlay rule:
    /// a later descriptor may replace an earlier one only via an explicit
    /// `replaces` declaration; any other duplicate model-visible name errors.
    pub fn resolve(
        &self,
        requested: &[BehaviorPackId],
        services: &BehaviorSessionServices,
    ) -> Result<ResolvedBehavior, BehaviorInstallError> {
        let mut order: Vec<BehaviorPackId> = Vec::new();
        for id in requested {
            if !order.contains(id) { order.push(id.clone()); }
        }
        let mut tools: Vec<ResolvedTool> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new(); // exposed_name -> tools index
        let mut compatibility: Option<CompatibilityContract> = None;
        let mut packs = Vec::new();
        for id in &order {
            let pack = self.packs.get(id).ok_or_else(|| BehaviorInstallError::UnknownPack(id.clone()))?;
            if pack.schema_version != 1 {
                return Err(BehaviorInstallError::UnsupportedSchemaVersion { pack: id.clone(), got: pack.schema_version });
            }
            for d in &pack.tools {
                if let Some(existing_id) = d.replaces.as_ref() {
                    if let Some(pos) = tools.iter().position(|t| &t.descriptor.id == existing_id) {
                        // explicit compatible replacement: drop the old descriptor,
                        // re-key the exposed name
                        let old_name = tools[pos].descriptor.exposed_name.clone();
                        tools.remove(pos);
                        by_name.retain(|_, v| *v != pos);
                        let _ = old_name;
                    }
                }
                if let Some(existing) = by_name.get(&d.exposed_name) {
                    return Err(BehaviorInstallError::DuplicateExposedName {
                        exposed_name: d.exposed_name.clone(),
                        existing: tools[*existing].descriptor.id.clone(),
                        incoming: d.id.clone(),
                    });
                }
                let factory = pack.factory_for(&d.id).ok_or_else(|| BehaviorInstallError::FactoryFailed { descriptor: d.id.clone(), reason: "no factory registered".to_string() })?;
                by_name.insert(d.exposed_name.clone(), tools.len());
                tools.push(ResolvedTool { descriptor: d.clone(), factory });
            }
            compatibility = Some(match compatibility {
                Some(c) => c.merge(&pack.compatibility),
                None => pack.compatibility.clone(),
            });
            packs.push(id.clone());
        }
        let patch = Self::agent_config_patch(&tools, services);
        Ok(ResolvedBehavior {
            packs, tools, patch,
            prompt_layers: order.iter().filter_map(|id| self.packs.get(id)).flat_map(|p| p.prompt_layers.clone()).collect(),
            degradations: Vec::new(),
            compatibility: compatibility.unwrap_or(CompatibilityContract { target: String::new(), entries: Vec::new() }),
        })
    }

    /// Patch = the services the host actually offers, restricted to standard fields.
    fn agent_config_patch(tools: &[ResolvedTool], services: &BehaviorSessionServices) -> AgentConfigPatch {
        AgentConfigPatch {
            snapshot_store: services.snapshot_store.clone(),
            lsp: services.lsp.clone(),
            ttsr_engine: services.ttsr_engine.clone(),
            url_resolver: services.url_resolver.clone(),
            subagent_runner: services.subagent_runner.clone(),
            memory: services.memory.clone(),
            todo: services.todo.clone(),
            prompt_layers: tools.iter().filter_map(|_| None).collect(), // replaced below
        }
    }
}
```

Correction to carry into implementation: `prompt_layers` on the patch must come from the resolved packs' `prompt_layers` (assembled in `resolve`, where the packs are in scope) — pass them in; the `.filter_map(|_| None)` line above is NOT the final code. Build the patch inside `resolve` after assembling `prompt_layers` and set `patch.prompt_layers = prompt_layers.clone()`.

`ResolvedBehavior::install`:

```rust
impl ResolvedBehavior {
    /// Install every resolved tool through the host installer (pack order preserved).
    pub fn install(
        &self,
        services: &BehaviorSessionServices,
        installer: &mut dyn BehaviorToolInstaller,
    ) -> Result<InstalledBehaviorManifest, BehaviorInstallError> {
        let mut manifest: Option<InstalledBehaviorManifest> = None;
        // Group tools by owning pack so each pack's extension checks run.
        // (Implementation detail: iterate self.packs, resolve each pack from a
        // captured map id -> &BehaviorPack stored on ResolvedBehavior at resolve
        // time — add a private `packs_by_id: Vec<(BehaviorPackId, Arc<BehaviorPack>)>`-style
        // field, or simplest: iterate self.tools and run install() through a
        // synthesized single-pack view. The observable contract is: every tool is
        // offered to the installer exactly once, in resolve order; degradations
        // (disabled/optional-missing/host-rejected) and errors follow the same
        // rules as BehaviorPack::install.)
        todo!() // NO — implement by delegating: see correction below
    }
}
```

Implementation correction (normative, avoid the `todo!`): simplest correct shape is to store `Vec<Arc<BehaviorPack>>` (`resolved_packs`) as a private field on `ResolvedBehavior` at resolve time and make `install` delegate:

```rust
pub fn install(&self, services: &BehaviorSessionServices, installer: &mut dyn BehaviorToolInstaller) -> Result<InstalledBehaviorManifest, BehaviorInstallError> {
    let mut merged: Option<InstalledBehaviorManifest> = None;
    let mut degraded = Vec::new();
    let mut replaced_ids: std::collections::HashSet<ToolImplementationId> = self.tools.iter().filter_map(|_| None).collect(); // see below
    for pack in &self.resolved_packs {
        // Skip descriptors that a later pack replaced: keep a set of replaced ids
        // computed at resolve time (store it as a private field `replaced_ids`).
        let mut pack_view_manifest = pack.install(services, installer)?;
        // drop tools whose descriptor was replaced during resolution
        pack_view_manifest.tools.retain(|t| !self.replaced_ids.contains(&t.descriptor));
        // skip tools that resolution already dropped from self.tools
        degraded.extend(pack_view_manifest.degraded.drain(..));
        merged = Some(match merged {
            Some(mut m) => { m.tools.extend(pack_view_manifest.tools); m.packs.extend(pack_view_manifest.packs); m }
            None => pack_view_manifest,
        });
    }
    let mut m = merged.ok_or_else(|| BehaviorInstallError::UnknownPack(BehaviorPackId("<empty-resolution>".to_string())))?;
    m.degraded = degraded;
    m.prompt_layers = self.prompt_layers.iter().map(|l| l.id.clone()).collect();
    m.compatibility = self.compatibility.clone();
    Ok(m)
}
```

`replaced_ids` and `resolved_packs` are private fields populated in `resolve` (`replaced_ids`: ids of descriptors removed by the `replaces` rule). The unused `.filter_map(|_| None)` line above is likewise NOT final — compute the set in `resolve`.

If the double-install nuance (per-pack `install` running extension checks twice) becomes awkward, alternative accepted implementation: `ResolvedBehavior::install` iterates `self.tools` directly and replicates `BehaviorPack::install`'s per-tool rules, then appends optional-extension degradations from `self.resolved_packs` extension specs. Either shape is acceptable; the observable contract (one installer call per resolved tool, resolve order, same degradation/error semantics, manifest aggregation) is the spec. Pick one; do not ship `todo!`.

- [ ] **Step 3: mod.rs wiring** + re-exports (`BehaviorPackResolver`, `ResolvedBehavior`, `ResolvedTool`).
- [ ] **Step 4: Verify** — all resolver tests PASS, clippy clean.
- [ ] **Step 5: Commit** — `feat(sdk): deterministic behavior pack resolution with overlay replacement`

### Task 5: `coding-omp-v1` reference pack

**Files:**
- Create: `oxicode-sdk/src/behavior/packs/mod.rs`, `packs/coding_omp_v1.rs`
- Modify: `oxicode-sdk/src/behavior/mod.rs` (`pub mod packs;`)

**Interfaces:**
- Consumes: Tasks 1–4; agent tool constructors (`ReadTool::with_cwd`, `WriteTool::with_cwd`, `EditTool::with_cwd`, `BashTool::with_cwd`, `GrepTool::with_cwd`, `FindTool::with_cwd`, `LsTool::with_cwd` — import via `oxicode_agent::tools::{…}` or crate-root re-exports; `ast_grep::AstGrepTool::with_cwd`, `ast_edit::AstEditTool::new()`, `web_search::WebSearchTool::new(cache)`, `search_cache::{GetSearchResultsTool::new(cache), SearchCache::new()}`, `todo::TodoTool`, `subagent::SubagentTool::with_cwd`, `lsp::LspTool`, `eval_tool::EvalTool`, `debug_tool::DebugTool` — use full `oxicode_agent::tools::<module>::` paths).
- Produces: `packs::coding_omp_v1::pack() -> BehaviorPack`.

- [ ] **Step 1: Failing tests** (`coding_omp_v1.rs` `#[cfg(test)]`):
  - descriptor set == exactly these 16 exposed names in order: `read, write, edit, bash, grep, find, ls, ast_grep, ast_edit, web_search, get_search_results, todo, subagent, lsp, eval, debug`;
  - ids match the descriptor table below; `edit` declares required `HashlineSnapshotStore`; `bash/eval/debug/lsp/subagent/todo` declare their optional ports; essentials = `read, write, edit, bash, grep, find, ls`;
  - ledger: 9 entries, statuses per Global Constraints; `rollup() == Unavailable`; target starts with `omp@v18.0.11`;
  - install with minimal services (snapshot store present, everything else None) → Ok manifest with 16 tools and degradations exactly `{shell-session, eval-kernel, debug-service, ttsr-engine, lsp-host, delegation}` (6);
  - every factory succeeds against `tempfile::tempdir()` services (constructs real tools).

- [ ] **Step 2: Descriptor table** (implement exactly):

| id | exposed | capability | side_effect | ports | state_scope | essential |
|---|---|---|---|---|---|---|
| `read.file.v1` | read | FsRead | ReadOnly | — | HashlineSession | yes |
| `write.file.v1` | write | FsWrite | Mutating | — | HashlineSession | yes |
| `edit.hashline.v1` | edit | FsWrite | Mutating | HashlineSnapshotStore **required** | HashlineSession | yes |
| `bash.process.v1` | bash | Process | ProcessSpawning | ShellSession optional | ShellSession | yes |
| `grep.search.v1` | grep | Search | ReadOnly | — | Stateless | yes |
| `find.search.v1` | find | Search | ReadOnly | — | Stateless | yes |
| `ls.fs.v1` | ls | FsRead | ReadOnly | — | Stateless | yes |
| `ast-grep.search.v1` | ast_grep | Search | ReadOnly | — | Stateless | no |
| `ast-edit.write.v1` | ast_edit | FsWrite | Mutating | — | Workspace | no |
| `web-search.network.v1` | web_search | Network | Networked | — | Stateless | no |
| `search-results.cache.v1` | get_search_results | Search | ReadOnly | — | Stateless | no |
| `todo.session.v1` | todo | Ui | Mutating | TodoStateProvider optional | Workspace | no |
| `subagent.delegation.v1` | subagent | Delegation | ProcessSpawning | SubagentRunner optional | Stateless | no |
| `lsp.host.v1` | lsp | Lsp | ReadOnly | LspProvider optional | Stateless | no |
| `eval.kernel.v1` | eval | Process | ProcessSpawning | EvalKernel optional | EvalKernel | no |
| `debug.dap.v1` | debug | Process | ProcessSpawning | DebugService optional | DebugTarget | no |

Factory notes: `web_search` + `get_search_results` share ONE `Arc<SearchCache>` created in `pack()` and captured by both closures (mirrors `with_builtins_cwd`'s `cache_once`).

- [ ] **Step 3: Extensions** (declaration order):

```rust
.pack()
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::HashlineState, scope: ExtensionScope::SessionWorkspace, required: true })
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::LspHost, scope: ExtensionScope::Workspace, required: false })
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::ShellSession, scope: ExtensionScope::SessionWorkspace, required: false })
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::EvalKernel, scope: ExtensionScope::SessionLanguage, required: false })
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::DebugService, scope: ExtensionScope::WorkspaceDebugTarget, required: false })
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::TtsrEngine, scope: ExtensionScope::Turn, required: false })
.with_extension(RuntimeExtensionSpec { kind: ExtensionKind::Delegation, scope: ExtensionScope::ChildAgentLifecycle, required: false })
```

(`HashlineState` required ⇒ a host that cannot provide a snapshot store fails pack resolution loudly — this is the design's "reject a required tool/extension" path.)

- [ ] **Step 4: Prompt layer**:

```rust
const DISCIPLINE_LAYER_ID: &str = "coding-omp-v1/discipline";
const DISCIPLINE_BODY: &str = "\
You are operating under the coding-omp-v1 behavior pack.

Editing discipline:
- Read a file before editing it. Edits anchor to `[path#TAG]` snapshots; after any
  external change, re-read to refresh anchors.
- Prefer anchored line edits over whole-file rewrites.
- After edits, verify: compile, run the relevant test, or show the diff.

Execution discipline:
- Prefer focused commands with explicit output; check exit codes before proceeding.
- Keep long-running services in managed background processes, not one-shot calls.";
```

- [ ] **Step 5: Ledger** — `CompatibilityContract { target: "omp@v18.0.11 (can1357/oh-my-pi@b8ce33a)".into(), entries }` with exactly:

```rust
entry("read-write-search", Equivalent, &["behavior::hashline_read_edit_stale_anchor_recovery"], "File read/write/grep/find/ls exercised through pack-installed registry with scripted transcripts; no OMP-specific deviation observed."),
entry("hashline-anchors", Equivalent, &["behavior::hashline_read_edit_stale_anchor_recovery"], "Anchored edits via hashline::SnapshotStore; snapshots are session-local in-memory (bounded). Cross-session persistence intentionally deferred."),
entry("lsp", Partial, &["behavior::lsp_mock_actions"], "Generic LspProvider port + CLI rust-analyzer discovery only; no broad default-server matrix or rename-with-file-operations scenario yet."),
entry("persistent-shell", Unavailable, &[], "Pack bash tool is the legacy per-invocation implementation. ShellSession contract declared; no persistent implementation."),
entry("persistent-eval", Unavailable, &[], "Pack eval tool runs a fresh process per call. EvalKernel contract declared; no persistent implementation."),
entry("dap-debugging", Unavailable, &[], "Pack debug tool is a validated scaffold. DebugService contract declared; no DAP host."),
entry("ttsr", Partial, &["behavior::ttsr_patch_and_rule_retry"], "TtsrEngine + RuleRegistry ports exist and patch wiring is contract-tested; hosts ship no rules by default."),
entry("delegation", Partial, &["behavior::child_agent_runner_contract"], "SubagentRunner injection is contract-tested; typed child task context and inherited-limit enforcement remain host-side."),
entry("host-product-tools", NotApplicable, &[], "MCP, memory, github, commit and other product tools remain host-composition concerns, not pack tools."),
```

- [ ] **Step 6: Verify** — targeted nextest + clippy (feature on) clean.
- [ ] **Step 7: Commit** — `feat(sdk): coding-omp-v1 reference behavior pack`

### Task 6: CLI consumption + parity tests

**Files:**
- Modify: `oxicode-cli/Cargo.toml` (features += `behavior` on the oxicode-sdk dep; add `oxicode-hashline = { version = "<workspace version>", path = "../oxicode-hashline" }` matching the existing dep style), `oxicode-cli/src/main.rs` or `lib.rs` module list (add `pub mod behavior;` following however `bootstrap` is declared — check `mod` vs `pub mod` convention), `oxicode-cli/src/bootstrap.rs`, `oxicode-cli/src/lib.rs` (App), `oxicode-cli/src/tui_vt/slash/commands.rs` (only if `/info` is reachable — see Step 5)

**Interfaces:**
- Consumes: Task 5 pack.
- Produces: `crate::behavior::{BehaviorComposition, install_coding_omp_v1}`; `App.behavior: Option<BehaviorComposition>`; `App::from_oxicode(..., behavior: Option<BehaviorComposition>)` (4th param — update ALL callsites, grep first).

- [ ] **Step 1: `oxicode-cli/src/behavior.rs`** (full):

```rust
//! CLI behavior-pack composition (`coding-omp-v1` reference consumer).
//!
//! Policy note: the CLI registers pack tools as-is to preserve today's
//! composition; hosts that need per-tool audit/approval wrap the tool in
//! their `BehaviorToolInstaller` before registration (design "Host policy
//! boundary").

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use oxicode_agent::{AgentTool, ToolRegistry};
use oxicode_hashline::{InMemorySnapshotStore, SnapshotStore};
use oxicode_sdk::behavior::{
    AgentConfigPatch, BehaviorInstallError, BehaviorPackId, BehaviorPackResolver,
    BehaviorSessionServices, BehaviorToolDescriptor, BehaviorToolInstaller,
    InstalledBehaviorManifest,
};

/// Manifest plus the requested AgentConfig patch produced by installing packs.
#[derive(Clone)]
pub struct BehaviorComposition {
    pub manifest: InstalledBehaviorManifest,
    pub patch: AgentConfigPatch,
}

struct CliToolInstaller<'a> {
    tools: &'a ToolRegistry,
}

impl BehaviorToolInstaller for CliToolInstaller<'_> {
    fn install(&mut self, descriptor: &BehaviorToolDescriptor, tool: Arc<dyn AgentTool>) -> Result<(), BehaviorInstallError> {
        self.tools.register_arc(tool);
        tracing::debug!(tool = %descriptor.exposed_name, id = %descriptor.id.0, "behavior pack tool installed");
        Ok(())
    }
}

/// Install `coding-omp-v1` into `tools`, overwriting the legacy instances of
/// the same names with pack-constructed equivalents.
///
/// `allow` mirrors `--tools` (comma-split, trimmed by the caller): pack tools
/// not named are host-disabled. `disabled_tools` mirrors `--no-*`/settings.
/// Returns `None` on resolution/install failure — the legacy builtin
/// composition keeps running (degraded, logged loudly).
pub fn install_coding_omp_v1(
    tools: &ToolRegistry,
    cwd: &Path,
    allow: Option<&[String]>,
    disabled_tools: &[String],
) -> Option<BehaviorComposition> {
    let mut resolver = match BehaviorPackResolver::with_builtin_packs() {
        Ok(r) => r,
        Err(e) => { tracing::warn!("behavior pack registry failed: {e}"); return None; }
    };
    let pack_id = BehaviorPackId::coding_omp_v1();
    let Some(pack) = resolver.pack(&pack_id) else {
        tracing::warn!("coding-omp-v1 missing from builtin packs"); return None;
    };
    let mut disabled: Vec<String> = disabled_tools.to_vec();
    if let Some(allow) = allow {
        let allowed: HashSet<&str> = allow.iter().map(String::as_str).collect();
        for t in &pack.tools {
            if !allowed.contains(t.exposed_name.as_str()) {
                disabled.push(t.exposed_name.clone());
            }
        }
    }
    let services = BehaviorSessionServices::new(cwd.to_path_buf())
        .with_snapshot_store(Arc::new(InMemorySnapshotStore::new()) as Arc<dyn SnapshotStore>)
        .with_disabled_tools(disabled);
    let resolved = match resolver.resolve(&[pack_id], &services) {
        Ok(r) => r,
        Err(e) => { tracing::warn!("behavior pack resolve failed: {e}"); return None; }
    };
    let patch = resolved.patch.clone();
    let mut installer = CliToolInstaller { tools };
    match resolved.install(&services, &mut installer) {
        Ok(manifest) => Some(BehaviorComposition { manifest, patch }),
        Err(e) => { tracing::warn!("behavior pack install failed: {e}"); None }
    }
}
```

- [ ] **Step 2: bootstrap wiring** — at the END of `register_builtin_tools` (bootstrap.rs:523-559), before returning, install the pack and return the composition. Change the signature to return `Option<crate::behavior::BehaviorComposition>` and update its (single) callsite (grep `register_builtin_tools(`) to capture it; there, log the startup line:

```rust
if let Some(b) = &behavior {
    tracing::info!(
        pack = %b.manifest.packs.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join("+"),
        tools = b.manifest.tools.len(),
        degraded = b.manifest.degraded.len(),
        level = ?b.manifest.compatibility_level(),
        "behavior pack installed"
    );
}
```

Keep `args.tools` parsing: pass `allow` as `args.tools.as_deref().map(|s| s.split(',').map(|t| t.trim().to_string()).collect::<Vec<_>>())`.

- [ ] **Step 3: App plumbing** — in `oxicode-cli/src/lib.rs`:
  - Add field `pub behavior: Option<crate::behavior::BehaviorComposition>` to `App`; set in `from_oxicode`.
  - `App::from_oxicode(...)` gains final param `behavior: Option<crate::behavior::BehaviorComposition>`. In the `AgentConfig` construction: `snapshot_store: behavior.as_ref().and_then(|b| b.patch.snapshot_store.clone())`, and prepend patch prompt layers to the existing `system_prompt` value:

```rust
let layer_prefix = behavior.as_ref().map(|b| {
    b.patch.prompt_layers.iter().map(|l| l.body.clone()).collect::<Vec<_>>().join("\n\n")
}).filter(|s| !s.is_empty());
let system_prompt = match (layer_prefix, existing_system_prompt) {
    (Some(prefix), Some(base)) => Some(format!("{prefix}\n\n{base}")),
    (Some(prefix), None) => Some(prefix),
    (None, base) => base,
};
```

(Grep `from_oxicode(` for every callsite — tests included — and pass `None` where there is no composition.)
- [ ] **Step 4: `/info` surfacing** — grep `oxicode-cli/src/tui_vt/slash/` for the `/info` command implementation. If `SlashCtx` reaches the `App` (or a state field can carry the manifest summary), add one Info line: `behavior-pack: coding-omp-v1 (tools N, degraded M, level <level>)`. If `SlashCtx` has no path to the manifest, do NOT force one — instead thread the one-line summary string into `RenderState`/welcome metadata only if a natural field exists; otherwise skip (design says the CLI "may render" the report) and note the skip in the commit message.
- [ ] **Step 5: Parity + composition tests** — add `#[cfg(test)] mod behavior_tests` in `oxicode-cli/src/behavior.rs`:

```rust
#[test]
fn pack_names_are_subset_of_legacy_builtins() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = ToolRegistry::with_builtins_cwd(tmp.path().to_path_buf(), &[]);
    let names: HashSet<String> = legacy.names().into_iter().collect();
    let mut resolver = BehaviorPackResolver::with_builtin_packs().unwrap();
    let pack = resolver.pack(&BehaviorPackId::coding_omp_v1()).unwrap();
    for t in &pack.tools {
        assert!(names.contains(&t.exposed_name), "pack tool '{}' missing from legacy builtins", t.exposed_name);
    }
}

#[test]
fn composition_installs_manifest_and_overwrites_names() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new();
    let comp = install_coding_omp_v1(&registry, tmp.path(), None, &[]).expect("install succeeds");
    assert_eq!(comp.manifest.packs, vec![BehaviorPackId::coding_omp_v1()]);
    assert_eq!(comp.manifest.tools.len(), 16);
    for t in &comp.manifest.tools {
        assert!(registry.get(&t.exposed_name).is_some(), "{} not registered", t.exposed_name);
    }
    let degraded: HashSet<&str> = comp.manifest.degraded.iter().map(|d| d.feature.as_str()).collect();
    let expected: HashSet<&str> = ["shell-session", "eval-kernel", "debug-service", "ttsr-engine", "lsp-host", "delegation"].into();
    assert_eq!(degraded, expected);
    assert_eq!(comp.manifest.compatibility_level(), oxicode_sdk::behavior::FeatureStatus::Unavailable);
    assert_eq!(comp.patch.prompt_layers.len(), 1);
}

#[test]
fn allow_filter_disables_unselected_pack_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new();
    let allow = vec!["read".to_string(), "grep".to_string()];
    let comp = install_coding_omp_v1(&registry, tmp.path(), Some(&allow), &[]).expect("install succeeds");
    assert!(registry.get("read").is_some() && registry.get("grep").is_some());
    assert!(registry.get("bash").is_none(), "non-allowed tools must not be installed");
    assert!(comp.manifest.degraded.iter().any(|d| matches!(d.reason,
        oxicode_sdk::behavior::DegradationReason::DisabledByHost)));
}
```

(`tempfile` must be in `[dev-dependencies]` — verify, add if missing. The second test's MCP-free path is why the pack never spawns `McpManager`.)
- [ ] **Step 6: Verify** — `cargo nextest run -p oxicode-cli behavior` PASS; `cargo clippy -p oxicode-cli --all-targets -- -D warnings` clean (this also covers the native-browser default path); manual smoke: `cargo run -p oxicode-cli -- --version` still exits 0.
- [ ] **Step 7: Commit** — `feat(cli): compose coding tools through the coding-omp-v1 behavior pack`

### Task 7: Fixture harness (SDK integration tests)

**Files:**
- Create: `oxicode-sdk/tests/behavior/main.rs`, `tests/behavior/common/mod.rs`, stub scenario modules

**Interfaces:**
- Produces: `common::{ScriptedReply, ScriptedProvider, RecordingInstaller, TraceTool trace type, MockLspProvider, MockSubagentRunner, StaticRules, install_pack_for_tests(workspace) -> (ToolRegistry-resolved manifest…)}` — scenario modules (Tasks 8–9) consume these.

- [ ] **Step 1: Target layout** — `tests/behavior/main.rs`:

```rust
mod common;
mod delegation_fixture;
mod denial_fixture;
mod degradation_fixture;
mod duplicate_fixture;
mod hashline_fixture;
mod lsp_fixture;
mod ttsr_fixture;
```

(Cargo auto-discovers `tests/behavior/main.rs` as ONE integration target. Each scenario module holds 1–2 `#[tokio::test]`s.)
- [ ] **Step 2: `common/mod.rs`** — implement, guided by (copy patterns from) `oxicode-agent/src/tests.rs:14-120` (`MockProvider`) and `oxicode-agent/tests/approval_tests.rs:48-140` (tool-call emission), plus `oxicode-agent/src/tools.rs:346-389` (`LspProvider` signatures), `:431-457` (`SubagentRunner`), `oxicode-agent/src/agent_loop/ttsr.rs:99-110` (`RuleRegistry`) and the `Rule`/`TtsrSettings` definitions in that file:

```rust
#![allow(clippy::unwrap_used)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub type Trace = Arc<Mutex<Vec<String>>>;

pub enum ScriptedReply {
    Text(String),
    ToolCalls(Vec<(String, serde_json::Value)>),
}

/// Provider returning one scripted reply per `stream` call; panics in test
/// style (allowed here) when the transcript runs dry.
pub struct ScriptedProvider {
    replies: Mutex<Vec<ScriptedReply>>,
    calls: AtomicUsize,
}

impl ScriptedProvider {
    pub fn new(replies: Vec<ScriptedReply>) -> Self { Self { replies: Mutex::new(replies), calls: AtomicUsize::new(0) } }
    pub fn calls(&self) -> usize { self.calls.load(Ordering::SeqCst) }
}

impl oxicode_ai::Provider for ScriptedProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a oxicode_ai::Model,
        _context: &'a oxicode_ai::Context,
        _options: Option<oxicode_ai::StreamOptions>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = oxicode_ai::StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let reply = self.replies.lock().unwrap().remove(0); // or error when empty
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Emit the same ProviderEvent sequence MockProvider emits for a
            // completed assistant message: Message(content) then Completed,
            // with ContentBlock::ToolUse per scripted tool call. COPY the exact
            // event construction from oxicode-agent/src/tests.rs:64-120.
            todo!() // replace with the copied construction — no todo in final code
        })
    }
}

/// Host installer that records every installed name and wraps the tool in a
/// tracing proxy — proving all registry entries flow through the interceptor.
pub struct RecordingInstaller {
    pub installed: Mutex<Vec<String>>,
    pub trace: Trace,
}

impl oxicode_sdk::behavior::BehaviorToolInstaller for RecordingInstaller {
    fn install(&mut self, d: &oxicode_sdk::behavior::BehaviorToolDescriptor, tool: Arc<dyn oxicode_agent::AgentTool>) -> Result<(), oxicode_sdk::behavior::BehaviorInstallError> {
        self.installed.lock().unwrap().push(d.exposed_name.clone());
        // register the TRACING WRAPPER, not the bare tool:
        let wrapped = TraceTool { inner: tool, trace: self.trace.clone() };
        self.pending.lock().unwrap().push((d.exposed_name.clone(), Arc::new(wrapped)));
        Ok(())
    }
}
```

Correction (normative): `BehaviorToolInstaller::install` only RECEIVES the tool — the host then registers it into ITS registry. For fixtures the "registry" is a plain `Vec<(String, Arc<dyn AgentTool>)>` inside `RecordingInstaller` (`pending` above); provide `fn into_registry(self) -> oxicode_agent::ToolRegistry` that `register_arc`s every wrapped tool. `TraceTool` implements `AgentTool` by delegating `name/label/description/parameters_schema/essential/execute` to `inner`, recording `format!("{}:{tool_call_id}", inner.name())` into `trace` before delegating execute.

Mocks:

```rust
pub struct MockLspProvider { /* implements oxicode_agent::tools::LspProvider; records executed LspAction values, returns canned DiagnosticsSummary/definitions */ }
pub struct MockSubagentRunner { pub prompts: Mutex<Vec<String>> } // run_isolated records prompt, returns ForkResult::default()
pub struct StaticRules { pub rules: Vec<Rule>, pub injections: Mutex<Vec<(String, u64)>> } // ttsr::RuleRegistry
pub struct DenyTool { inner: Arc<dyn AgentTool>, pub trace: Trace } // execute → Ok(AgentToolResult::success(r#"{"error":"denied by host policy","recoverable":true}"#)), records denial
```

(`DenyTool` used by the denial fixture via a custom installer that substitutes it for `bash`.)

Shared helper:

```rust
pub fn workspace_with_lib_rs() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    (dir, file)
}

pub fn minimal_services(ws: &Path) -> BehaviorSessionServices {
    BehaviorSessionServices::new(ws.to_path_buf())
        .with_snapshot_store(Arc::new(oxicode_hashline::InMemorySnapshotStore::new()))
}
```

- [ ] **Step 3: Stub scenario modules compile** (empty `#[tokio::test] fn` placeholders are FORBIDDEN — write the tests in Tasks 8–9 and only create modules then; this task lands `main.rs` + `common/mod.rs` + ONE smoke test `common::harness_installs_all_sixteen_tools` asserting `RecordingInstaller` receives exactly the 16 names via `ResolvedBehavior::install`).
- [ ] **Step 4: Verify** — `cargo nextest run -p oxicode-sdk --features behavior behavior` PASS; clippy `-p oxicode-sdk --all-targets --features behavior` clean (test-code lint relaxations apply).
- [ ] **Step 5: Commit** — `test(sdk): behavior fixture harness with scripted provider and recording installer`

### Task 8: Fixtures — hashline anchors, degradation report, duplicate/overlay rules

**Files:**
- Modify: `tests/behavior/hashline_fixture.rs`, `degradation_fixture.rs`, `duplicate_fixture.rs`

- [ ] **Step 1: `hashline_read_edit_stale_anchor_recovery`** (family 1) — full transcript through a real `Agent` (copy Agent construction + event consumption from `oxicode-agent/tests/agent_loop_full.rs:309-360`, adapting the provider):

```text
services: minimal_services(ws)           // snapshot store present
replies: [
  ToolCalls([("read",    {"path": <file>})]),
  ToolCalls([("edit",    {"path": <file>, "old_text": "fn main() {}", "new_text": "fn main() { println!(\"changed\"); }", "expected_hash": <TAG from read output>})]),
  ToolCalls([("read",    {"path": <file>})]),                      // recovery re-read
  ToolCalls([("edit",    {"path": <file>, "old_text": …, "new_text": …, "expected_hash": <fresh TAG>})]),
  Text("done"),
]
between turn 2 and 3: std::fs::write(&file, "fn main() { /* concurrent */ }\n")   // external change
assert:
  - trace (RecordingInstaller) call order == ["read:*", "edit:*", "read:*", "edit:*"]
  - edit#1 result text contains the conflict/applied=false semantics (assert on "not applied" OR the conflict marker edit.rs:143-148 emits — read the exact message and assert a stable substring)
  - edit#2 applies; final file content == edited text
  - manifest.tools contains read + edit records
```

Also a direct-tool variant (`#[tokio::test]` calling `registry.get("edit")…execute` with a `ToolContext::default().with_root(ws).with_snapshot_store(...)`) if the agent-loop plumbing proves flaky — but the loop variant is preferred (proves ToolContext threading from `AgentConfig.snapshot_store`).
- [ ] **Step 2: `degradation_report_is_honest`** — minimal services WITHOUT lsp/ttsr/etc: resolve+install via `RecordingInstaller`; assert 16 installed, degradation feature set == `{shell-session, eval-kernel, debug-service, ttsr-engine, lsp-host, delegation}`, `compatibility_level() == Unavailable`, `manifest.compatibility.entries.len() == 9`, and the three `Unavailable` ledger entries carry empty `evidence`.
- [ ] **Step 3: `duplicate_names_and_overlay_replacement`** — register `coding_omp_v1::pack()` plus a tiny test pack exposing `"read"` without `replaces` → `Err(DuplicateExposedName)`; with `replaces: Some("read.file.v1")` and id `read.custom.v1` → resolve Ok, resolved tools contain exactly one `read`, descriptor id `read.custom.v1`, and install hands the CUSTOM tool to the installer under name `read`.
- [ ] **Step 4: Verify** + commit — `test(sdk): hashline/degradation/overlay fixture scenarios`

### Task 9: Fixtures — denial, delegation, LSP, TTSR

**Files:**
- Modify: `tests/behavior/denial_fixture.rs`, `delegation_fixture.rs`, `lsp_fixture.rs`, `ttsr_fixture.rs`

- [ ] **Step 1: `host_denial_cannot_be_bypassed`** (family 8) — `RecordingInstaller` variant that substitutes `DenyTool` for `bash` (name registered stays `bash`). Transcript: ToolCalls([("bash", {"command": "touch pwned"})]) then Text("ack"). Assert: trace shows the denial record; `ws.join("pwned")` does NOT exist; `AgentToolResult` text contains `"denied by host policy"`; no other bash path exists (the interceptor is the only registration route — structural property already enforced by the pack never touching a registry).
- [ ] **Step 2: `child_agent_runner_contract`** (family 7) — services `.with_subagent_runner(Arc::new(MockSubagentRunner::default()))`; transcript ToolCalls([("subagent", {"agent": "scout", "task": "find TODOs"})]) + Text("done"). Assert runner received prompt containing "find TODOs"; final answer reflects ForkResult text; manifest marks no delegation degradation (service present).
- [ ] **Step 3: `lsp_mock_actions`** (family 2, Partial scope) — services `.with_lsp(Arc::new(MockLspProvider))`; transcript ToolCalls([("lsp", /* action payload per `lsp.rs` schema — read `oxicode-agent/src/tools/lsp.rs` parameters_schema and use its exact action/args keys */)]) + Text("done"). Assert MockLspProvider recorded the action and the lsp result text embeds the mock's canned response; manifest marks `lsp-host` NOT degraded.
- [ ] **Step 4: `ttsr_patch_and_rule_retry`** (family 6) — services `.with_ttsr_engine(Arc::new(TtsrEngine::new(Arc::new(StaticRules::with_one_test_rule()), TtsrSettings { enabled: true, ..Default::default() })))`. Assert: `resolved.patch.ttsr_engine.is_some()`; drive `engine.check_delta("/* violating text */", &ctx)` with `TtsrMatchContext` built per ttsr.rs tests (`StaticRegistry` pattern at ttsr.rs:676-684) and assert a rule match + `injected_records()` records the injection. This is the pack-level wiring contract; full loop-interrupt semantics stay covered by existing agent TTSR tests.
- [ ] **Step 5: Verify** + commit — `test(sdk): denial/delegation/lsp/ttsr fixture scenarios`
- [ ] **Step 6: CI** — confirm the scenarios run in the default `cargo nextest run --workspace` gate (they do: plain `cargo nextest`; no network, no paid model, no TUI, no OMP binary — spec "Compatibility test architecture"). If `ci.yml` needs the `behavior` feature for sdk tests, check `.github/workflows/ci.yml`'s test invocation and add `--features behavior` where oxicode-sdk tests run; otherwise no change.

### Task 10: Full gates + docs

- [ ] **Step 1:** `cargo fmt --all` then `cargo fmt --all -- --check`.
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] **Step 3:** `cargo nextest run --workspace`.
- [ ] **Step 4:** `cargo clippy -p oxicode-cli -- -D warnings` (native-browser default path — repo rule).
- [ ] **Step 5: CHANGELOG.md** — under the unreleased section (create if absent), entry: "Added: behavior-pack API (`oxicode-sdk` feature `behavior`), `coding-omp-v1` reference pack, and CLI consumption via host installer interception. OMP compatibility ledger: shell/eval/DAP Unavailable; LSP/TTSR/delegation Partial; read/write/search + hashline anchors Equivalent (fixture-evidenced). Target `omp@v18.0.11`."
- [ ] **Step 6: AGENTS.md** — add a short section under Conventions or Pitfalls: `coding-omp-v1` is the canonical coding composition; `ToolRegistry::with_builtins_cwd` remains a low-level convenience and is NOT a behavior guarantee (spec "Tool manifest and replacement policy"). Do not hardcode the pack's tool list outside `behavior/packs/`.
- [ ] **Step 7: design doc** — flip `**Status:** Proposed` → `**Status:** Adopted (implementation: behavior pack API + coding-omp-v1 + fixtures; persistent shell/eval/DAP remain follow-up)`.
- [ ] **Step 8: Commit** — `docs: behavior pack ledger, changelog, and composition notes`

## Self-Review

1. **Spec coverage:** pack types/installer/manifest/degradation (Tasks 2–4 = acceptance 1,2,3), extension lifetimes + required ports (extension specs + install checks), CLI selects `coding-omp-v1` without a second loop (Task 6 = acceptance 4), deterministic fixtures in CI (Tasks 7–9 = acceptance 5), honest Partial/Unavailable ledger (Task 5 ledger = acceptance 6). Resolution determinism + duplicate/replacement rules (Task 4). Portability boundary (no `with_builtins` requirement on consumers). Host policy boundary preserved (`BehaviorToolInstaller` wrapping point; CLI registers as-is = today's policy). Follow-ups explicitly out of scope: persistent shell/eval/DAP impls (migration step 4), Oxios adoption (separate doc), crates.io publish (release runbook).
2. **Placeholder scan:** Task 4's `todo!()` blocks are marked as corrections-to-apply, not shipped code — implementer MUST land the correction form. Task 7's `ScriptedProvider::stream` `todo!()` is a copy instruction with exact source anchors (tests.rs:64-120). Task 9 Step 3 requires reading `lsp.rs` schema keys — bounded, concrete.
3. **Type consistency:** `BehaviorPackId::coding_omp_v1()`, `ExtensionKind::slug()/port()`, `FeatureStatus::rank/worst`, `BehaviorSessionServices::with_snapshot_store/with_disabled_tools`, `ResolvedBehavior.{tools,patch,prompt_layers,degradations,compatibility}`, `InstalledBehaviorManifest.compatibility_level()` — used identically across Tasks 3–9. CLI test expects exactly 6 degradation slugs (lsp-host included) while Task 6 Step 5's CLI test builds services WITHOUT lsp — consistent with Task 5 test (same minimal services).
