# Oxi Foundation and oxibrain Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make oxicode a first-class Oxi Foundation host: resolve provider profiles and packages from one shared installation, and use oxibrain as its only durable-memory authority while preserving explicit user control and normal coding-agent approvals.

**Architecture:** Oxi Foundation v1 is a versioned filesystem/protocol contract, not a broker or a new mandatory Rust crate. oxicode reads the neutral contract, maps package-declared abstract capabilities to its existing workspace/access/tool policy, and uses a typed oxibrain client for durable memory. Its agent engine remains independent of oxibrain and continues to run with a truthful degraded-memory state if the daemon is unavailable.

**Tech Stack:** Rust 2024, Tokio, existing `oxicode-sdk` ports, existing `oxicode-ai` provider registry, OS Keychain, typed oxibrain JSON-RPC client, existing package manager and package locks.

## Global constraints

- Oxi Foundation is shared configuration only in v1. Do not create a common daemon, model gateway, or required cross-repository runtime crate.
- The shared location is `~/.oxi/foundation/v1/`. It contains non-secret profile metadata, package resolution and trust metadata, and a compatibility manifest. Credentials remain in the OS Keychain; no secret may appear in Foundation files, lockfiles, logs, diagnostics, or telemetry.
- Resolution order is explicit process override, then Foundation profile, then a narrowly-scoped compatibility import while migration is incomplete. An absent or invalid selected profile is an error; never silently choose a different remote provider.
- oxibrain remains the durable-memory authority. Do not retain a second oxicode-owned durable memory database, JSON file, summary file, or SQLite pipeline after migration. Session-local cache is allowed only when it cannot become an authority or be replayed as durable memory.
- Loss of oxibrain connectivity is a surfaced degraded state. It must not switch to local durable memory, discard a requested write, or fabricate a successful result.
- Preserve ordinary oxicode coding behavior: workspace changes, shell execution, tool invocation, and review/approval remain governed by oxicode's own access policy. Foundation package requirements never grant a capability by themselves.
- Use typed errors and capability declarations. Do not add silent fallbacks, raw secret plumbing, or host-specific semantics to Foundation manifests.
- Make code comments and documentation English. Keep all user-visible migration and destructive-operation messages actionable.

---

## Shared Foundation v1 contract oxicode consumes

The contract is published and fixture-tested alongside the equivalent oxibrain and oxios work. oxicode must parse the following semantic shapes exactly; it may use a Rust representation appropriate to its existing configuration code.

```text
~/.oxi/foundation/v1/
├── foundation.json             # schema version and host compatibility declarations
├── profiles.json               # non-secret provider/model profiles
├── packages.lock               # immutable resolved package records
└── packages/<sha256>/          # verified immutable package content
```

`profiles.json` stores profile identity, provider/model selection, role bindings, and a Keychain locator only:

```json
{
  "schema_version": 1,
  "profiles": [{
    "id": "personal-coding",
    "provider": "anthropic",
    "model": "claude-sonnet",
    "roles": ["coding.primary", "assistant.general"],
    "credential": { "service": "dev.oxi.foundation", "account": "personal-coding" }
  }]
}
```

No `api_key`, bearer token, OAuth refresh token, or provider-specific secret is permitted. `packages.lock` records package `name`, `version`, immutable `digest`, source, trust decision, target hosts, and declared abstract requirements. Package manifests declare only abstract requirements such as `workspace.read`, `workspace.patch`, `shell.execute`, `browser.navigate`, `brain.query`, and `schedule.manage`.

For a package selected by a persona or request, oxicode evaluates each declared requirement against its existing `AccessGate`, capability resolver, workspace approval, sandbox, and tool policy. A verified package is not automatically authorized. A package that asks for an unsupported or denied requirement must be rejected before its skill/persona content is injected. Only resolved, target-compatible, digest-verified package content is eligible for loading. Workspace overlays remain local and take precedence where current oxicode policy permits them.

## Existing seams and required cutover

| Concern | Current seam | Required destination |
|---|---|---|
| CLI composition | `oxicode-cli/src/services.rs` | Build Foundation profile/package readers and Brain-backed memory at the composition root. |
| Provider runtime | `oxicode-ai/src/providers/`, `oxicode-ai/src/model_registry.rs`, `oxicode-ai/src/provider_registry.rs` | Keep generic provider implementations; populate them only from explicit overrides or Foundation-selected profiles. |
| Legacy credentials | `oxicode-cli/src/store/auth_storage.rs`, setup/settings flow | Replace plaintext durable credential ownership with Keychain locators and controlled one-time import. |
| Agent memory contract | `oxicode-agent/src/tools.rs` (`MemoryBackend`) | Implement the contract with a typed `BrainMemoryBackend`, preserving explicit destructive semantics. |
| Memory composition and URLs | `oxicode-cli/src/services.rs`, `store/memory_mnemopi.rs`, `memory_sqlite.rs`, `memory_workers.rs`, `memory_summary.rs`, `internal_urls/memory_handler.rs` | Delete local durable-memory ownership after verified migration; resolve memory URLs through Brain-backed read APIs. |
| Skills/personas/packages | `oxicode-cli/src/storage/packages/`, `oxicode-cli/src/skills/`, SDK `FileSkillLoader` and `FilePersonaProvider` | Add verified Foundation package source and capability-aware selection; preserve local workspace overlays. |

## Implementation tasks

### 1. Publish the oxicode side of the contract and migration boundary

- [ ] Create `docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`. Copy the Foundation v1 schema, resolver precedence, trust rules, host capability mapping, error states, and cross-host fixture location from this plan. Link the canonical oxibrain integration document and the Oxios RFC without creating an oxicode dependency on either repository.
- [ ] Update `README.md`, `docs/INDEX.md`, `oxicode-cli/ARCHITECTURE.md`, and `oxicode-ai/ARCHITECTURE.md` to describe oxicode as an Oxi Foundation host, profile-driven provider selection, and Brain-backed durable memory. Remove claims that local Mnemopi/SQLite/JSON memory is the default durable authority.
- [ ] Rewrite `docs/custom-providers.md` to describe the implemented provider registry honestly: provider implementations remain extensible, but normal interactive selection is profile-driven and credentials are Keychain-resolved. Keep only documented explicit automation overrides.
- [ ] Mark `docs/designs/omp-adoption-2/11-mnemopi-backend.md` superseded with a link to the new contract and this plan. Do not silently delete historical rationale.
- [ ] Add a short compatibility matrix naming the three hosts and their roles: oxibrain owns durable memory/projection, oxicode owns code execution, and oxios owns orchestration/experience. State that oxios embeds `oxicode-sdk`; oxios must not normally spawn the oxicode CLI.

### 2. Add Foundation discovery, version negotiation, and typed configuration

- [ ] Add `oxicode-cli/src/foundation/mod.rs`, `profiles.rs`, `packages.rs`, and `compatibility.rs`. Define typed, serde-validated representations for `foundation.json`, `profiles.json`, and `packages.lock`; reject unknown major schema versions and malformed Keychain locators before provider initialization.
- [ ] Extend `OxicodePaths` in `oxicode-cli/src/services.rs` with a derived Foundation root using `$OXI_FOUNDATION_HOME` when explicitly set, otherwise `~/.oxi/foundation/v1`. Keep `OXICODE_HOME` for oxicode-owned sessions, caches, local overlays, and non-secret state only.
- [ ] Implement profile resolution as a pure decision function with inputs `{ explicit_profile, explicit_environment_override, requested_role, foundation_profiles, compatibility_import }` and one result or typed reason. Precedence is explicit environment override for automation, explicit selected profile, role-compatible Foundation profile, then one-time legacy import only while migration is enabled. Ambiguous role candidates and absent credentials must fail visibly.
- [ ] Make the first-run and setup paths read Foundation compatibility before mutating any local state. If no Foundation installation exists, explain how to initialize it or choose local/offline operation; do not create fake credentials or silently write a profile.
- [ ] Add shared cross-repository JSON fixtures under `tests/fixtures/oxi-foundation/v1/` for valid profiles, unknown schema, duplicate profile IDs, unsupported target, malformed locator, bad digest, denied requirement, and role ambiguity. Consume the same fixture copies/fixtures in each host CI until a neutral crate is justified by duplicated nontrivial parsing.

### 3. Replace plaintext provider ownership with Keychain-backed profile resolution

- [ ] Introduce a Keychain-backed `AuthProvider` implementation at `oxicode-cli/src/foundation/credentials.rs`. It receives a selected profile's `{ service, account }` locator, retrieves only the requested secret, and returns a typed unavailable/locked/not-found error without exposing the value in `Debug` or `Display`.
- [ ] Wire Foundation profile resolution and the Keychain provider into `build_oxicode_with_catalog` in `oxicode-cli/src/services.rs`. Preserve `oxicode-ai`'s generic `ProviderRegistry` and model registry; register the selected provider/model instance only after profile and credential validation succeeds.
- [ ] Remove normal interactive provider selection from plaintext `auth.json` and mutable provider settings. Keep a temporary one-time importer in `store/auth_storage.rs` that moves a legacy secret into Keychain, writes only the locator/profile migration marker, reports exactly what was migrated, and never re-exports the secret.
- [ ] Require explicit acknowledgement before the importer reads a legacy plaintext file. After a successful export/import verification, offer archival outside the active credential path; never delete a user file automatically.
- [ ] Keep environment credentials as an explicit, documented non-persistent automation override. Log only provider/profile ID, role, and source class (`environment`, `keychain`, or `unavailable`), never values or account names that leak private identities.
- [ ] Add table-driven tests for precedence, profile-role matching, unknown provider/model, absent Keychain entry, locked Keychain, legacy-import refusal, and redacted diagnostics. Test that a failed profile does not silently select another remote provider.

### 4. Load Foundation packages through oxicode's existing policy gates

- [ ] Extend `oxicode-cli/src/storage/packages/types.rs`, `lockfile.rs`, and `manager.rs` with Foundation package provenance: resolved digest, target hosts, abstract requirements, trust decision, and immutable content root. Do not overload an unverified package source into a trusted Foundation record.
- [ ] Add a Foundation package source adapter in `oxicode-cli/src/foundation/packages.rs`. Verify the content digest before discovery, require `oxicode` in targets, and reject untrusted/unknown packages before handing resources to `PackageManager`.
- [ ] Translate abstract package requirements into existing `SimpleAccessGate`, `TomlCapabilityResolver`, workspace approval, and tool policy decisions in one dedicated adapter. Mapping is host-local: `workspace.read` and `workspace.patch` remain subject to workspace/worktree policy; `shell.execute` remains subject to command approval/sandbox; `brain.query` requires a connected, scoped Brain client. Requirements may be denied even for trusted packages.
- [ ] Wire the filtered package resources to `FileSkillLoader`/`FilePersonaProvider` in `build_oxicode_with_catalog`. Resolve a persona and request first; inject only the selected compatible skills, never every installed skill into every agent context.
- [ ] Preserve local workspace overlays as a separate higher-precedence layer according to current oxicode policy. They are not written to Foundation storage and cannot mutate Foundation lock records.
- [ ] Add tests for digest mismatch, missing `oxicode` target, denied requirement, unsupported requirement, overlay precedence, stable ordered resolution, and the invariant that a denied package contributes no prompt, tool, persona, or extension content.

### 5. Make oxibrain the sole durable-memory backend

- [ ] Add an optional typed oxibrain client dependency and feature boundary for the CLI integration. The SDK and `oxicode-agent` remain port-defined and must not depend directly on store/database types, MCP framing, or oxibrain adapter crates.
- [ ] Implement `BrainMemoryBackend` in `oxicode-cli/src/foundation/brain_memory.rs` for the existing `oxicode_agent::tools::MemoryBackend` contract. Construct it with a scoped typed Brain client, caller identity, repository/workspace provenance, and session identifier—not a raw socket or secret string passed through agent code.
- [ ] Map memory operations explicitly: `put` creates a provenance-bearing Brain capture/declaration; `search` and `list` use cited Brain retrieval; `delete` resolves to auditable retraction when supported by the target and reserves redaction for explicit destructive operations; `clear_all` is unavailable unless the Brain protocol can enumerate a bounded scope and the user supplies the existing explicit confirmation. Never translate deletion into an invisible local row removal.
- [ ] Update the four agent-facing memory tools and slash-command responses so they return Brain IDs, citations/sources, scope, and a degraded/unavailable result where appropriate. Preserve the existing confirmation gate for destructive commands. Do not claim that a write succeeded until Brain acknowledges the ledger append.
- [ ] Make `memory_info`, `trigger_consolidation`, `trigger_harmonize`, and `enqueue_consolidation` truthful. Brain consolidation is a request to produce derived episodes with sources and uncertainty; it is not an oxicode summary-file rebuild, source-code reflection, or an implicit note edit.
- [ ] Replace `MemoryProtocolHandler` in `oxicode-cli/src/internal_urls/memory_handler.rs` with a read-only Brain-backed resolver. It must render cited retrieval/declaration data and immutable artifacts; it must not expose or create `~/.oxicode/memory` as an alternate durable store.
- [ ] In `services.rs`, install `BrainMemoryBackend` and the Brain-backed URL handler at the composition root. Connection setup must use the Foundation/Brain discovery handshake, scope the client to the active workspace/session, and surface unavailable daemon state before agent execution begins.
- [ ] Define one explicit degraded behavior: ordinary code work may proceed, while a durable-memory tool call returns a typed unavailable result containing the recovery command/endpoint state. No Mnemopi, SQLite, JSON, or file-summary fallback is permitted.

### 6. Migrate and remove every legacy durable-memory authority

- [ ] Inventory all durable write/read paths before deletion: `store/memory_mnemopi.rs`, `store/memory_sqlite.rs`, `store/memory_workers.rs`, `store/memory_summary.rs`, `store/extracting_backend.rs`, `mnemopi.rs`, the legacy settings fields in `store/settings.rs`, `internal_urls/memory_handler.rs`, and every `MemoryBackend` composition in `services.rs` and `agent_session_runtime.rs`.
- [ ] Add an explicit existing-memory command path, `oxicode memory migrate-brain`, to the CLI command surface. It must inspect source format without mutation, show item count and deterministic content/provenance hash, require source and target confirmation, append equivalent Brain episodes idempotently, then compare acknowledged target count/hash before reporting success.
- [ ] Preserve provenance for each imported item: `source = oxicode`, legacy backend kind, original stable identifier, repository/workspace scope, import timestamp, and migration version. Map legacy mutable facts to assertions/declarations; retain verbatim content required for later re-resolution.
- [ ] Implement resumable migration checkpoints that contain no secret. A crash/retry must neither duplicate ledger entries nor silently skip items. Test interruption after every import stage and a second run over the same corpus.
- [ ] After successful verification, mark the legacy store as archived and read-only outside active runtime paths. Do not delete legacy databases, JSON, or user-authored summary files automatically. Provide a separate explicit destructive cleanup command only after export/replay verification.
- [ ] Remove all legacy durable-memory constructors, default selection, scheduled worker wiring, summary generation, file-backed memory URLs, and settings knobs after the migration path ships. Clean cutover means no hidden secondary authority remains.
- [ ] Keep source-code reflection separate: coding agents may still propose code/reflection changes through normal review and approval. It must not become Brain consolidation or a writing path into user notes.

### 7. Contract tests, migration tests, and user-facing smoke checks

- [ ] Add unit tests for Foundation configuration parsing and pure resolution decisions. Cover every shared invalid fixture and prove that all error formatting redacts credentials.
- [ ] Add integration tests around `build_oxicode_with_catalog` with a fake Keychain and fake Brain client: selected profiles wire the intended provider, profile errors prevent engine startup or enter an explicit configured offline mode, and no fallback provider is registered.
- [ ] Add package integration tests proving that an installed package cannot exceed host policy, denied capabilities inject nothing, and compatible persona/package resolution is deterministic across lockfile orderings.
- [ ] Add `BrainMemoryBackend` contract tests for append acknowledgement, cited search/list result conversion, retraction versus explicit redaction, unavailable daemon behavior, scope propagation, consolidation dispatch semantics, and no-local-fallback invariant.
- [ ] Add end-to-end migration fixtures for Mnemopi, SQLite, JSON, worker queue, and file-summary legacy inputs. Assert count/hash equivalence, idempotent replay, crash-resume, archive-only post-migration reads, and no duplicate Brain ledger episodes.
- [ ] Run focused tests first, then the affected workspace checks: `cargo test -p oxicode-cli`, `cargo test -p oxicode-agent`, `cargo test -p oxicode-sdk`, `cargo test -p oxicode-ai`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo fmt --all -- --check`.
- [ ] Run an interactive smoke scenario with a fake Foundation root, fake Keychain entry, and test Brain daemon: select `coding.primary`, launch an agent, invoke a permitted package skill, retain and recall a memory with citations, stop the daemon, verify visible degraded memory behavior, restart it, and verify recovery without importing/creating local durable memory.
- [ ] Before merge, run a three-host fixture compatibility job against the exact Foundation v1 fixture revision. It must reject schema drift, digest differences, and semantic changes to profile role resolution or package requirements.

## Rollout and compatibility

1. Ship documentation, fixture contract, and read-only discovery diagnostics first.
2. Ship profile/Keychain resolution behind an explicit opt-in migration flag; retain only the one-time legacy importer.
3. Ship verified package resolution with no capability elevation.
4. Ship Brain-backed memory with a test daemon and explicit degraded behavior.
5. Offer opt-in idempotent migration. Require count/hash acknowledgement before changing active defaults.
6. Flip the default only after the migration and recovery checks are stable; remove local durable-memory backends in the same cutover release, while retaining archives outside runtime paths.

Rollback means disable the Foundation integration feature before migration, or restore a verified archived legacy store through the explicit legacy recovery procedure. It must not mean silently running both durable stores or writing new memories to the old store after a verified Brain migration.

## Non-goals

- A shared Oxi Foundation broker, background daemon, or universal model gateway.
- Giving a Foundation package direct shell, workspace, browser, scheduler, or Brain access.
- Replacing oxicode's provider implementations, access policy, sandbox, worktree, approvals, or code review flow.
- Making oxibrain an authoring system, a markdown editor, or a source-code mutation mechanism.
- Automatic deletion of legacy credentials, databases, JSON files, or summary files.
- Translating an unavailable Brain daemon into a local durable-memory fallback.
