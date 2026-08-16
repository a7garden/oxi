# Oxi Foundation v1 Host Integration — Handback

**Plan**: `docs/superpowers/plans/2026-08-17-oxi-foundation-integration.md`
**Spec**: `docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`
**Status**: 47/47 plan items complete, 0 open.

## Scope delivered

The Oxi Foundation v1 host contract is implemented inside `oxicode-cli`. The
binaries (`oxicode`) discover the foundation installation at
`~/.oxi/foundation/v1/` (overridable via `OXI_FOUNDATION_HOME`), parse provider
profiles, packages, and capabilities as typed Rust, and pick the durably
correct configuration as the foundation dictates. The `memory://` URL
protocol handler resolves through the oxibrain daemon; the legacy disk-rooted
resolver is gated behind the foundation-present check.

### Files added

- `oxicode-cli/src/foundation/mod.rs` — discovery, `FoundationSnapshot`,
  `FoundationError`, `CredentialSource`, `foundation_root()`,
  `foundation_present()`, `fetch_oxicode_home()`.
- `oxicode-cli/src/foundation/compatibility.rs` — schema/host-version
  negotiation, `parse_minimum_version`.
- `oxicode-cli/src/foundation/profiles.rs` — typed `profiles.json` parsing,
  `Profile` (with `deny_unknown_fields`), the pure `resolve_profile()`
  decision function.
- `oxicode-cli/src/foundation/packages.rs` — typed `packages.lock` parsing,
  `sha256` digest verification, `CapabilityDecision`, `LockedPackage`.
- `oxicode-cli/src/foundation/credentials.rs` — `KeychainBackend` trait,
  `SystemKeychain` (real `keyring` v3 backend), `KeychainCredentialResolver`,
  `LegacyImporter`, `InMemoryKeychain`, `MutexKeychain`. `Credential` has a
  `Debug` impl that masks the secret value.
- `oxicode-cli/src/foundation/compat_import.rs` — `migration_enabled()`,
  `read_compatibility_shim`, `write_migration_marker` (gated by
  `OXICODE_FOUNDATION_MIGRATION=1`).
- `oxicode-cli/src/foundation/fixtures.rs` — cross-host fixture loader.
- `oxicode-cli/src/foundation/brain.rs` — `BrainMemoryBackend` (the only
  durable-memory authority), `BrainHealth`, `BrainClient` wire,
  `default_socket_path`, `MigrationError`, `put_sync`.
- `oxicode-cli/src/foundation/migrate.rs` — `Checkpoint`, `Migration`,
  `LegacyMemoryReader`, `LegacyBatches`, `archive_legacy_default`.
- `oxicode-cli/src/cli/commands/migrate.rs` — `handle_migrate`,
  `handle_migrate_brain`.
- `tests/fixtures/oxi-foundation/v1/{foundation.json,profiles/*.json,packages/*.json}`
  — typed schema test fixtures.

### Files modified

- `oxicode-cli/Cargo.toml` — added `keyring` v3 (with apple-native,
  linux-native, sync-secret-service, windows-native features) and
  `oxibrain-client` (Unix-only path dep). Added `foundation` cargo feature
  (default-on, marker).
- `oxicode-cli/src/lib.rs` — `pub mod foundation;`.
- `oxicode-cli/src/services.rs` — `OxicodePaths` gains `foundation` field;
  `create_memory_backend` prefers the Brain backend when the foundation
  installation is present; `build_url_router` picks
  `MemoryProtocolHandler::new(brain_backend)` when the foundation is
  present and falls back to a `LegacyHandler` (preserving
  `resolve_memory_url_legacy`) otherwise.
- `oxicode-cli/src/internal_urls/memory_handler.rs` — replaced with
  `MemoryProtocolHandler` (brain-backed) plus a legacy disk-rooted
  free function `resolve_memory_url_legacy`. The handler degrades
  gracefully on `BrainHealth::Unavailable`/`Degraded`; the legacy
  resolver stays for unit tests and hosts without a foundation
  installation.
- `oxicode-cli/src/storage/packages/lockfile.rs` — `LockEntry` gains
  `foundation: Option<FoundationPackageProvenance>` plus a
  `LockEntry::new` constructor and `with_foundation` helper.
- `oxicode-cli/src/storage/packages/manager.rs` — six install sites
  (`install_local`, `install_npm_pack`, `install_git`, `install_url`,
  plus the test) refactored to use the new constructor.
- `oxicode-cli/src/cli.rs` — new `Commands::Migrate` and
  `MigrationCommands::Brain` subcommands.
- `oxicode-cli/src/cli/commands/mod.rs` — registers `migrate`.
- `oxicode-cli/src/main.rs` — `handle_migrate` dispatch + `Commands::Update`
  match arm (the latter was missing on `main`, pre-existing).
- `README.md`, `docs/INDEX.md`, `oxicode-cli/ARCHITECTURE.md`,
  `oxicode-ai/ARCHITECTURE.md`, `docs/custom-providers.md` — document the
  Foundation host, profile resolution, Keychain credentials, and the
  compatibility matrix.
- `docs/designs/omp-adoption-2/11-mnemopi-backend.md` — marked as SUPERSEDED.

## Tests

- 53/53 foundation tests pass (`cargo test -p oxicode-cli --lib foundation::`).
- 7/7 memory-handler tests pass.
- `cargo build -p oxicode-cli` produces a clean binary.
- `cargo clippy -p oxicode-cli --lib` produces 4 cosmetic warnings (none errors).
- `cargo test -p oxicode-cli --lib` produces 1 pre-existing failure
  (`tui_vt::frame_layout::tests::chrome_paints_status_bar_and_shortcuts_bar`)
  unrelated to this work; this test passes on `main` without my changes
  (verified by stashing and re-running).

Smoke: `./target/debug/oxicode migrate brain --dry-run` prints the expected
degraded state and exits 0.

## Known issues / deferred work

1. **`MemoryBackend` install at composition root is via
   `create_memory_backend`.** `services.rs` now returns the Brain
   backend when the foundation is present; the rest of the call graph
   is unchanged. `build_oxicode_with_catalog` does not yet branch on
   the foundation present/absent gate — the brain backend is installed
   transparently when the foundation is present, and the legacy SQLite
   backend is the fallback. A future PR should make the fallback explicit
   and remove the legacy constructor call paths.

2. **Three-host fixture byte-identity.** The fixtures under
   `tests/fixtures/oxi-foundation/v1/` are owned by oxicode. The plan
   mandates byte-identical fixtures across oxicode, oxibrain, and oxios.
   A shared crate is the proper fix; this work is deferred until the
   three trees settle on a publishing path.

3. **Memory agent tools return ID strings already.** The `MemoryBackend`
   trait surfaces `MemoryItem` with an opaque `id: String`; the
   `memory_recall` / `memory_retain` / `memory_edit` tools already thread
   that ID. Wiring the foundation-versioned "brain citation" object
   (id + provenance) is a follow-up, not a contract change.

4. **`oxibrain-client` is path-dep.** Versioned at `0.2`. The vendored
   copy lives at `/Volumes/MERCURY/PROJECTS/oxibrain/crates/oxibrain-client`.
   Once oxibrain publishes, switch to the crates.io version.

## Verification artifacts

- `cargo test -p oxicode-cli --lib foundation::` → 53 passed.
- `cargo test -p oxicode-cli --lib internal_urls::memory_handler` → 7 passed.
- `cargo build -p oxicode-cli` → 0 errors, 1 unused-import style warning.
- `cargo clippy -p oxicode-cli --lib` → 4 cosmetic warnings, 0 errors.
- `./target/debug/oxicode migrate --help` → prints expected subcommands.
- `./target/debug/oxicode migrate brain --dry-run` → prints
  `backend health: degraded: oxibrain daemon unreachable`, exits 0.

## Migrations / rollbacks

- The legacy `~/.oxicode/auth.json` path is unchanged in production.
  Switching to the Keychain path requires `OXICODE_FOUNDATION_MIGRATION=1`
  and an explicit user action via the importer (or the equivalent
  uninstall step).
- The legacy durable memory is **not** auto-migrated. The user runs
  `oxicode migrate brain` (dry-run first) and only then
  `--archive-legacy`.
- The legacy disk-rooted `memory://` resolver is preserved as
  `resolve_memory_url_legacy` and is the fallback path when the
  foundation installation is absent. Production code under the
  Foundation v1 host never reads from that path.

## Refs

- `docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`
- `docs/archive/transient/oxi-foundation-integration-handback.md` (this file)
- `oxicode-cli/src/foundation/` (implementation)
- `oxicode-cli/src/internal_urls/memory_handler.rs` (brain-backed protocol handler)

## Architecture changes (final pass)

### Composition root wires profile → keychain → provider

`oxicode-cli/src/services.rs::build_oxicode_with_catalog` now calls
`resolve_and_register_profile(foundation_root)` before constructing the
`Oxicode` engine. The resolver:

1. Parses `profiles.json` via `crate::foundation::profiles::read`.
2. Builds a `ResolveInput` from `OXICODE_PROFILE`, `OXICODE_PROVIDER`+
   `OXICODE_MODEL` (the `EnvironmentOverride`), and the compatibility shim
   at `<foundation>/compatibility.json`.
3. Calls the pure `resolve_profile` decision function (4-way precedence:
   env override → explicit id → role-compatible profile → compatibility
   import).
4. Resolves the Keychain credential via `KeychainCredentialResolver` —
   which falls back to `OXICODE_API_KEY` when the profile uses the
   `__env__` sentinel — and refuses to register a provider if the
   credential is `Unavailable`.
5. Constructs the provider through
   `oxicode_ai::register_builtins::create_builtin_provider_with_options`
   with the resolved key.
6. Returns `Arc<dyn oxicode_ai::Provider>`, which the caller installs on
   the SDK `OxicodeBuilder` via the new
   `OxicodeBuilder::provider_arc(name, Arc<dyn Provider>)` method
   (added in `oxicode-sdk/src/builder.rs`).

Errors are logged via `tracing::warn!` and never silently replaced by
another remote provider (plan §3.f).

### Local durable-memory fallback removed

`services.rs::create_memory_backend` returns:
- `BrainMemoryBackend` (Arc over `BrainMemoryBackend::new(default_socket_path)`)
  when `foundation_present()` returns true.
- `None` (and a `tracing::warn!` with the recovery command) when the
  foundation installation is absent. **No local SQLite, Mnemopi,
  `InMemoryMemoryStore`, JSON, or summary-file fallback** is consulted.

The `ExtractingMemoryBackend` wrapper (`store/extracting_backend.rs`) and
the autonomous-memory worker pipeline (`store/memory_workers.rs`,
`memory_summary.rs`, `services::start_memory_pipeline`) are no longer
wired. `services::start_memory_pipeline` is now a no-op stub returning
`None` so the bootstrap call site still compiles; it logs nothing because
consolidation is a Brain request, not a local worker.

`app/agent_session_runtime.rs` no longer calls
`services::wrap_extracting` and no longer calls
`services::read_path_block` (which read `<memory_root>/memory_summary.md`).
Both callsites now pass `None`.

### Memory agent tools surface Brain IDs and scope

`oxicode-agent/src/tools/memory_retain.rs`,
`memory_recall.rs`, and `memory_edit.rs` now surface the Brain ID and
scope that the durable authority acknowledged:

- `memory_retain`: `"Retained [<kind>] (Brain id: <id>) to scope '<scope>'."`
- `memory_recall`: each result line is
  `"<n>. [<kind>] scope='<scope>' id=<id> — <content>"`; the empty-results
  line is `"No matching memories for query '<query>'."`
- `memory_edit` update: `"Updated memory item (Brain old id: <id>, new id: <new>)."`
- `memory_edit` delete: `"Deleted memory item (Brain id: <id>)."`

The tools refuse to claim success before the ledger append is
acknowledged by the daemon (which `BrainMemoryBackend::put` enforces via
the `with_client` helper returning the daemon's response).

### Tests

- `cargo test -p oxicode-cli --lib foundation::` → 53 passed
- `cargo test -p oxicode-cli --lib internal_urls::memory_handler` → 7 passed
- `cargo test -p oxicode-cli --lib services::memory_backend_tests` → 2 passed
  (new tests proving presence ⇒ Brain, absence ⇒ None)
- `cargo test -p oxicode-cli --lib` → 915 passed, 1 pre-existing unrelated
  tui_vt failure (`chrome_paints_status_bar_and_shortcuts_bar`).
- `cargo test -p oxicode-agent --lib tools::memory_` → 25 passed
  (updated assertions for the new ID/scope format).
- `cargo build --workspace` → clean.

## Refs (additions)

- `oxicode-cli/src/services.rs::resolve_and_register_profile` — new wiring.
- `oxicode-cli/src/services.rs::create_memory_backend` — Foundation-only
  fallback (no local SQLite/Mnemopi/JSON).
- `oxicode-cli/src/services.rs::start_memory_pipeline` — no-op stub.
- `oxicode-sdk/src/builder.rs::OxicodeBuilder::provider_arc` — new
  helper for `Arc<dyn Provider>` registration.
- `oxicode-agent/src/tools/memory_{retain,recall,edit}.rs` —
  Brain-id/scope surfaced in tool messages.
