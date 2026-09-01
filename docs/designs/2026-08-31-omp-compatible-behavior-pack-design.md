# OMP-Compatible Coding Behavior Packs

**Status:** Adopted (behavior-pack API + `coding-omp-v1` + fixtures shipped; persistent shell/eval/DAP reference runtimes implemented and ledger-advanced to Partial — exposed tool routing remains follow-up)
**Date:** 2026-08-31  
**Owners:** Oxicode maintainers  
**Depends on:** \`oxicode-agent\`, \`oxicode-sdk\`, \`oxicode-hashline\`, \`oxicode-lsp\`

## Summary

Oxicode will expose an OMP-compatible coding behavior as a versioned, portable
**behavior pack**. A pack is not a second agent loop and is not a terminal UI
feature. It is a declarative contract plus a canonical installer for the tools,
runtime extensions, prompt layers, and observable semantics needed for a class
of work.

\`coding-omp-v1\` is the first reference pack. Oxicode CLI will consume it, and
other products such as Oxios can consume the same released SDK contract while
retaining their own authentication, workspace, policy, UI, and lifecycle
ownership.

This moves the portability boundary above individual tool names but below
product policy:

\`\`\`text
OMP behavior target
        │
        ▼
oxicode-sdk behavior-pack contract
        │
        ├── Oxicode CLI composition
        └── Oxios recipe host + policy envelope
\`\`\`

The design deliberately does not embed or execute the OMP binary in production.
OMP remains a compatibility target and optional benchmark oracle, not a runtime
dependency.

## Problem

The current repositories already share the agent loop through \`oxicode-sdk\`,
but that does not make their coding behavior equivalent.

The Oxicode CLI composes a broad builtin registry and wires selected SDK ports,
including hashline snapshots, session hooks, URL resolution, and an LSP
provider. Consumers can also build an \`Agent\` with only a small subset of
tools, omit optional ports, or replace tools entirely. Therefore “uses
\`Oxicode\`” is not a meaningful behavior guarantee.

The gap is also not exclusively an Oxios gap. The present Oxicode implementation
does not yet establish all of the behavior advertised by OMP as equivalent:

- \`coding_tools()\` installs only the basic file tools; it is not a complete
  coding environment.
- LSP has a generic provider contract, but the CLI default discovery is
  currently centered on Rust.
- the current Bash tool starts a process per call rather than providing an
  OMP-style persistent shell;
- the Eval tool documents persistent state as future work;
- the Debug tool is a scaffold rather than a complete DAP host.

Porting those behaviors independently into every consumer would recreate the
same drift. Giving every product the entire builtin registry directly would
instead let product-specific policy be bypassed. A portable composition
contract is required.

The external compatibility baseline is the upstream
[oh-my-pi repository](https://github.com/can1357/oh-my-pi), pinned by release
or commit in the compatibility ledger rather than assumed to be a moving
“latest” target.

## Goals

1. Define one released, versioned coding behavior contract that all Oxicode
   consumers can install.
2. Keep canonical tool semantics, stateful coding services, and compatibility
   tests in Oxicode rather than duplicating them in consumers.
3. Give a host complete control over which tools are exposed, how every tool is
   wrapped, and whether a call is allowed or requires approval.
4. Support both a CLI composition and non-terminal products without changing
   the underlying coding behavior.
5. Make OMP compatibility an auditable set of claims rather than a marketing
   label.

## Non-goals

- Reproducing OMP's TUI, renderer, key bindings, or binary protocol.
- Making a pack choose credentials, RBAC, path sandboxing, approval UX,
  tenancy, scheduling, or data retention for a host product.
- Replacing the existing generic \`Agent\` loop with a coding-specific loop.
- Claiming every OMP feature is equivalent before its acceptance scenarios pass.
- Adding a new workspace crate merely to name this abstraction.

## Decision

Add a behavior-pack API to \`oxicode-sdk\`. The API describes requirements and
installs canonical tool implementations through a host-controlled interception
point. The first supplied pack is \`coding-omp-v1\`.

The SDK owns:

- the pack identifier, schema version, prompt-layer requirements, tool
  descriptors, and compatibility claims;
- canonical coding-tool implementations and their shared state semantics;
- portable extension contracts and reference implementations where no
  product-specific authority is required;
- compatibility fixtures and trace/tree assertions.

The consuming product owns:

- selecting a pack and overlays for a request;
- workspace roots, credentials, policy, quotas, feature flags, and approval;
- adapting product services to declared ports;
- wrapping each installed tool before it enters the registry;
- rendering events and persisting session history.

This is a stricter boundary than today’s \`with_builtins()\`: a consumer can no
longer need to know a pack’s individual tool list in order to reproduce its
behavior, but it never has to surrender its policy boundary.

## Public model

The names below are illustrative Rust API names, not a commitment to their
exact module spelling.

\`\`\`rust
pub struct BehaviorPack {
    pub id: BehaviorPackId,            // e.g. "coding-omp-v1"
    pub schema_version: u32,
    pub prompt_layers: Vec<PromptLayerSpec>,
    pub tools: Vec<BehaviorToolDescriptor>,
    pub extensions: Vec<RuntimeExtensionSpec>,
    pub compatibility: CompatibilityContract,
}

pub struct BehaviorToolDescriptor {
    pub id: ToolImplementationId,      // stable implementation identity
    pub exposed_name: String,          // model-visible name
    pub capability: CapabilityClass,
    pub side_effect: SideEffectClass,
    pub required_ports: Vec<PortRequirement>,
    pub state_scope: ToolStateScope,
}

pub trait BehaviorToolInstaller: Send {
    fn install(
        &mut self,
        descriptor: &BehaviorToolDescriptor,
        tool: Arc<dyn AgentTool>,
    ) -> Result<(), BehaviorInstallError>;
}
\`\`\`

\`BehaviorPack::install()\` receives a \`BehaviorSessionServices\` object and a
\`BehaviorToolInstaller\`. It creates the canonical tools and asks the host to
install them one at a time. The host may:

- wrap the tool in an access gate, audit layer, approval layer, or telemetry
  adapter and install that wrapper;
- reject an unavailable optional tool and return a structured degradation
  record;
- reject a required tool, causing pack resolution to fail before an agent turn
  begins.

The pack must never call \`ToolRegistry::register*\` behind the host’s back.
The installer result is an \`InstalledBehaviorManifest\` that records the
actual tools, disabled capabilities, extension status, and compatibility level
for the turn. It is distinct from the existing lifecycle \`ToolManifest\`,
which records state-snapshot registry metadata.

\`BehaviorPack\` is declarative at selection time. Installation may allocate
per-session resources, but cannot consult model output or widen host authority.

## Composition and module placement

No new workspace crate is needed.

| Location | Responsibility |
|---|---|
| \`oxicode-sdk/src/behavior/\` | Public pack types, resolver/installer contract, manifests, compatibility metadata, and AgentConfig augmentation contract. |
| \`oxicode-agent/src/tools/\` | Canonical model-facing tool implementations and tool-specific service traits. |
| \`oxicode-agent/src/runtime/\` | Shared stateful coding runtimes that have no product authority: persistent shell/eval session protocols, TTSR integration adapters, and debug-service contracts. |
| \`oxicode-hashline/\` | Anchored edit snapshots and hashline state; it remains the owner of \`hashline::SnapshotStore\`. |
| \`oxicode-lsp/\` | Transport/client primitives. A reusable SDK adapter may be feature-gated in \`oxicode-sdk\`, while server discovery/configuration remains host-owned. |
| \`oxicode-cli/\` | CLI-specific provider configuration, terminal presentation, authentication, and the reference consumer composition. |

The optional SDK feature that exposes a generic LSP host adapter may depend on
\`oxicode-lsp\`; it must not make \`oxicode-lsp\` depend on the SDK or CLI.

## Behavior-pack resolution

Packs are selected by explicit identifier. Products may layer a small number of
declared overlays, for example \`coding-omp-v1 + git-review-v1\`. Resolution
must be deterministic and must reject duplicate model-visible tool names unless
an overlay explicitly declares a compatible replacement.

\`\`\`text
requested pack ids + host feature inventory
        │
        ▼
BehaviorPackResolver
        │ validates schema, dependencies, replacement rules
        ▼
ResolvedBehavior
        │
        ├── required ports and extensions
        ├── canonical prompt layers
        ├── canonical tool descriptors
        └── compatibility/degradation report
\`\`\`

A behavior pack may request an \`AgentConfigPatch\` for standard SDK fields
such as hashline snapshots, LSP provider, TTSR engine, URL resolver, session
hooks, and subagent runner. It does not receive unrestricted mutable access to
an \`AgentConfig\`; the host builds the final configuration after validating
the patch against policy.

## Coding extensions

\`coding-omp-v1\` defines extension requirements separately from tools so their
lifetime is visible and testable.

| Extension | Scope | Required behavior |
|---|---|---|
| Hashline state | session + workspace | bounded anchored snapshots for reads and safe patch/edit verification |
| LSP host | workspace | capability discovery, diagnostics, navigation, edits/renames, lifecycle cleanup |
| Shell session | session + workspace | persistent command environment, cancellation, output bounds, explicit reset |
| Eval kernel | session + language | persistent Python/Bun state, bounded execution, tool bridge only through host policy |
| Debug service | workspace + debug target | real DAP session lifecycle, breakpoint/control/output events |
| TTSR engine | turn | rule evaluation and retry/repair metadata without hidden model invocations |
| Delegation | child-agent lifecycle | typed child task context, inherited limits, cancellation and result collection |

The current separate process Bash behavior and stateless Eval behavior are
legacy implementations. They remain available to legacy registry consumers,
but \`coding-omp-v1\` must select only the implementations that satisfy the
session semantics above. A feature is marked \`Partial\` in the compatibility
ledger until its extension exists and passes the relevant scenario.

The two meanings of “snapshot” must remain distinct in names and APIs:

- \`hashline::SnapshotStore\` is file/edit-anchor state;
- \`sdk::lifecycle::SnapshotStore\` is agent-lifecycle persistence.

Neither is an alias for the other.

## Tool manifest and replacement policy

Tool names are model API. Implementation identities are product-facing
stability keys. The pack manifests both.

\`\`\`text
behavior tool id: edit.hashline.v1
exposed name:      edit
capability:        workspace.write
state:             hashline session
side effect:       mutating
\`\`\`

This permits a future \`edit.hashline.v2\` to be introduced without silently
changing a pack’s claimed behavior. A replacement must declare:

1. the descriptor it replaces;
2. schema compatibility;
3. migration semantics for state;
4. compatibility scenarios it preserves or intentionally changes.

The legacy \`ToolRegistry::with_builtins_cwd\` remains a low-level convenience
API. It is not a guarantee of \`coding-omp-v1\` behavior and must not be
described as one.

## Host policy boundary

Existing SDK ports such as \`AccessGate\` are useful but intentionally coarse.
A host may need path-aware, argument-aware, tenant-aware, or approval-aware
decisions. Therefore the pack API does not replace product gates with a single
SDK boolean.

The required order is:

\`\`\`text
canonical SDK tool
    → host policy/audit/approval wrapper
    → host-owned ToolRegistry
    → Agent
\`\`\`

A behavior pack may classify a tool as read-only, mutating, networked, or
process-spawning. That classification is advisory input to policy; it can
never authorize an operation. Host denial must be surfaced as a normal,
structured tool result so the model can recover.

## OMP compatibility contract

Each pack release includes a machine-readable ledger, conceptually:

\`\`\`text
target: omp@<release-or-commit>
feature: hashline-edit
status: Equivalent | Partial | Unavailable | NotApplicable
evidence: scenario ids
notes: bounded snapshot policy and intentional deviations
\`\`\`

“Equivalent” means the listed scenarios establish the externally relevant
semantics, not byte-for-byte source or UI identity. “Partial” is mandatory when
an exposed tool exists but lacks required persistence, protocol coverage, or
failure semantics. The CLI and host products may render this report to aid
diagnosis, but they cannot upgrade a status themselves.

Initial expectations:

| Area | Initial target status | Reason |
|---|---|---|
| Read/write/search/edit, hashline anchors | Partial → Equivalent | needs pack-level installation and fixtures |
| LSP | Partial | generic port exists; broad default-server support and scenarios are incomplete |
| Persistent shell | Unavailable | current Bash is per invocation |
| Persistent Python/Bun eval | Unavailable | current Eval is stateless |
| DAP debugging | Unavailable | current Debug tool is scaffold-only |
| TTSR | Partial | ports/rules exist; pack contract and trace coverage are needed |
| Delegation | Partial | SDK supports runner injection; the reference pack needs typed, tool-capable child semantics |

No release may advertise \`coding-omp-v1\` as fully OMP-equivalent while any
required area remains \`Partial\` or \`Unavailable\`.

## Compatibility test architecture

Compatibility is tested through behavior fixtures, not a brittle comparison of
terminal output.

Each fixture contains:

- an initial workspace, including a Git repository where relevant;
- a scripted model/provider transcript;
- pack id, enabled capabilities, and service inventory;
- expected final tree or diff;
- tool-trace invariants, including call order, denial/retry semantics, and
  session-state observations;
- an expected compatibility ledger status.

Fixtures run against the SDK pack installer in normal CI. They must not require
network access, a paid model, an interactive TUI, or an installed OMP binary.
An optional maintainer job may run selected scenarios against a pinned OMP
oracle to detect target drift; its result updates evidence, not production
runtime behavior.

Required scenario families are:

1. hashline read/edit after concurrent file change and stale-anchor recovery;
2. LSP symbol navigation, rename including file-operation effects, diagnostics,
   and server restart;
3. persistent shell working-directory/environment continuity, cancellation,
   reset, and output limits;
4. persistent Python and Bun evaluation state plus policy-mediated tool bridge;
5. DAP launch/attach, breakpoint, stepping, variables, termination;
6. TTSR rule retry and terminal failure trace;
7. child-agent cancellation, inherited limits, and isolated state;
8. denial/approval behavior proving that a host wrapper cannot be bypassed.

The test runner belongs with the SDK behavior contract. Tool implementation
unit tests remain in \`oxicode-agent\`; CLI tests prove only CLI composition.

## Migration

1. Introduce behavior types, resolved manifests, installer interception, and
   deterministic resolution in \`oxicode-sdk\`.
2. Make Oxicode CLI consume \`coding-omp-v1\` through the new installer while
   preserving its current policy and presentation adapters.
3. Move or factor portable extension behavior from CLI-only composition into
   the appropriate shared module.
4. Implement the missing persistent shell, eval, and DAP extensions behind
   the pack; update ledger entries only with passing scenarios.
5. Publish the SDK release to crates.io.
6. Let external consumers adopt that released contract; they must never take a
   path dependency on the Oxicode checkout.

For one release cycle, the pack installer should emit an explicit manifest
comparison in CLI tests against the legacy composition. Intentional differences
must be documented in the compatibility ledger.

## Alternatives considered

### A separate CodingHarness runtime

Rejected. It would duplicate the agent loop, session lifecycle, event model,
and model configuration of the general runtime. A coding pack is a composition
of the same runtime, not a second operating mode with a separate execution
engine.

### Give every product \`with_builtins_cwd()\`

Rejected. It is too low-level, does not establish stateful coding semantics,
and makes it easy to bypass a host’s per-tool security and approval layer.

### Let each product freely recreate the OMP tool set

Rejected. It causes semantic drift in exactly the areas that determine coding
quality: edit anchoring, stateful execution, LSP lifecycle, retries, and
subagent behavior.

### Execute OMP behind an RPC bridge

Rejected for production. It binds products to a foreign process lifecycle,
security model, and protocol. It is useful only as an optional benchmark
oracle during compatibility testing.

## Acceptance criteria

This design is ready to implement when the SDK API supports:

- installing a pack only through a host interceptor;
- producing a complete installed manifest and degradation report;
- declaring extension lifetimes and required ports;
- selecting \`coding-omp-v1\` in Oxicode CLI without a separate agent loop;
- running deterministic fixture scenarios in CI;
- reporting unimplemented OMP behavior honestly as \`Partial\` or
  \`Unavailable\`.

Oxios adoption is specified separately in
\`/Volumes/MERCURY/PROJECTS/oxios/docs/designs/2026-08-31-execution-recipe-coding-host-design.md\`.

