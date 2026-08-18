# oxibrain TUI Integration & Workspace Cleanup — Design

> **Autonomous session note (2026-08-18).** User delegated all decisions for an
> overnight run ("superpower로 알아서 진행해. 나는 자러간다"). Every decision below
> is grounded in the evidence cited; revisit on wake-up if any tradeoff is
> unwanted.

## Problem

Three defects keep the oxicode TUI from actually using the running oxibrain
daemon, and the workspace still carries the dead pre-Foundation memory stack.

1. **Socket path drift.** The daemon listens at `~/.oxi/brain/oxibrain.sock`
   (canonical per `oxibrain-client` discovery.rs and the live process — verified
   via `lsof`, pid 12322). oxicode's hand-rolled
   `foundation::brain::default_socket_path()` resolves `$XDG_RUNTIME_DIR/oxibrain.sock`
   → `~/.oxi/run/oxibrain.sock` — a path the daemon never uses. Every connect
   fails.
2. **Foundation gate blocks memory.** `services::create_memory_backend` returns
   a `BrainMemoryBackend` only when `~/.oxi/foundation/v1/` exists. That
   directory does not exist on this machine (daemon installed standalone), so
   the backend is `None` and all memory tools report "backend unavailable". The
   Foundation layout gates profiles/packages, not the memory authority.
3. **Wrong tool names.** `BrainMemoryBackend` calls `memory.put` / `memory.search`
   / `memory.list` / `memory.delete`. The daemon's fifteen MCP tools are
   `search, recall, brief, navigate, ingest, declare, why, contradictions, stats,
   traverse, review_merges, remember, retract, merge_entities, redact` — none
   named `memory.*`. Every tool call would fail even after fixing the socket.

Additionally: `memory_enabled` defaults to `false`, the TUI has **zero** brain
references (no health surface, no `/memory` command), and the legacy local
memory stack (`oxicode-mnemopi` crate + five dead `store/*memory*` modules +
`MnemopiEmbeddingBridge` wiring) is unreachable code kept "compilable during
the migration window" — the migration reads plain JSONL and needs none of it.

## Verified evidence

- Daemon live probe (unix socket, `tools/call`):
  - `stats {space:"personal"}` → `{"episodes":55,"entities":5,"statements":3,"contradictions":0}`
  - `search {query:"oxicode",limit:3}` → `[]`
- `oxibrain-client 0.2.0` (crates.io, pinned in Cargo.lock) exposes
  `connect(path)`, `connect_with_token`, `call_tool(name,args)`,
  `call_tool_json`, `search`, `recall`, `ingest`, `declare`, `stats(space)`,
  `ping()`. The canonical `default_socket_path()` helper ships in 0.3.x only —
  not published; we therefore fix our own resolver to the same canonical value.
- `LegacyMemoryReader` (foundation/migrate.rs) reads
  `~/.oxicode/memory/items.jsonl` directly — no dependency on the mnemopi stack.
- `rusqlite` is used in oxicode-cli only by `memory_sqlite.rs` and
  `memory_workers.rs` (both dead).
- SDK `EmbeddingProvider` port is consumed only by feature-gated
  `PortMemoryBackend`; oxicode-cli never uses it — the `MnemopiEmbeddingBridge`
  wiring feeds nothing.
- `oxicode-vtui::vim` is `#![deprecated]` since 0.75.0 with zero code users
  (CLI owns `tui_vt/vim/`).

## Design

### 1. Connectivity fixes (`oxicode-cli/src/foundation/brain.rs`, `services.rs`)

- `default_socket_path()` → `$OXIBRAIN_SOCKET` else `~/.oxi/brain/oxibrain.sock`.
  Drop the invented `$XDG_RUNTIME_DIR` and `~/.oxi/run` variants.
- `create_memory_backend`: gate on **daemon socket presence** (unix-socket file
  at the canonical path), not `foundation_present`. Foundation continues to
  gate profiles/packages only. Absent socket → `None` + warn naming the socket
  path and `oxibrain serve` (no local fallback — contract §5.h unchanged).
- Remap `MemoryBackend` methods onto the real daemon tools:

  | trait method | daemon tool | args | result mapping |
  |---|---|---|---|
  | `put(content, kind, subject)` | `remember` | `{content, space, source_path:"oxicode/<kind>/<subject>"}` | episode/extraction text → id string |
  | `search(query, k)` | `search` | `{query, space, limit:k}` | hits[] → `MemoryItem {id: entity_id, kind: entity_type, content: snippet}` |
  | `list(subject)` | `search` | `{query: subject, limit: 50}` | same mapping |
  | `delete(id)` | `retract` | `{statement_id: id}` | auditable retraction; non-statement id → typed error pointing at `redact` |
  | `clear_all` | — | unavailable (unchanged) |

- Default space `personal` (daemon convention), overridable via `with_scope`.
- Add `stats_async()`/`stats_sync()` (episodes/entities/statements/contradictions)
  and reuse `ping()` for liveness.

### 2. TUI connection (`oxicode-cli/src/tui_vt/`)

- `memory_enabled` default flips to **true**: the brain is the product's memory
  authority; machines without the daemon degrade honestly (tools return typed
  unavailable, boot recall skipped). Users can still opt out via settings.
- **Status chip**: `RenderState.brain: BrainChip` (`Off | Down | Ok | Degraded`).
  Rendered in the status bar after run status — `brain·ok` (success, dimmed),
  `brain·down` (alert). `Off` renders nothing (quiet-chrome contract).
- **Prober**: background interval task (first tick immediate, then every 20 s)
  pings the daemon with a 1.5 s timeout; a `tokio::sync::watch` channel feeds a
  new `select!` arm in `run_event_loop`. No vtui protocol changes.
- **`/memory` slash command**: reports enabled state, socket path, health,
  stats, last memories (via `search`), and recovery hints
  (`oxibrain serve`, `oxicode migrate brain` when a legacy store exists).
  Async work via `tokio::spawn`; replies through `InlineHandle::append_line`.
- Boot-recall injection already keys off `memory_enabled` + backend — it starts
  working once the fixes above land.

### 3. Dead-code removal

- Delete `oxicode-cli/src/store/{memory_mnemopi, memory_sqlite, memory_summary,
  memory_workers, mnemopi, extracting_backend}.rs` + `store/mod.rs` entries.
- Delete `services::{build_embedding_provider, build_remote_embedding_provider,
  MnemopiEmbeddingBridge, start_memory_pipeline}` + the bootstrap call sites;
  bootstrap passes `None` embeddings into `build_oxicode_engine`.
- Settings: remove `mnemopi_engine`, `memory_backend`, `memory_llm_extract`,
  `memory_llm_extract_model`, `memory_db_path`, `embedding_provider`,
  `embedding_base_url`, `embedding_api_key_env`, `embedding_model`.
  Serde has no `deny_unknown_fields` — old settings.toml keys are ignored
  safely.
- Delete the `oxicode-mnemopi` crate (incl. `oxicode-mnemopi-mcp` binary);
  remove from workspace members, oxicode-cli deps, and `publish.yml`.
- Drop `rusqlite` from oxicode-cli.
- Delete the deprecated `oxicode-vtui::vim` module (zero users).

### 4. Documentation

- `AGENTS.md`: crate list/dependency flow (12 crates, add `oxicode-catalog` /
  `oxicode-api-stability`, drop mnemopi), port-15 row, memory architecture
  section (canonical socket, degraded semantics, status chip, `/memory`),
  pitfalls, key types, file locations.
- `README.md`, `docs/PORT_GUIDE.md`, `oxicode-cli/ARCHITECTURE.md`: sync the
  same facts.
- `CHANGELOG.md`: Unreleased entry.
- `docs/designs/omp-adoption-2/11-mnemopi-backend.md`: removal addendum.

## Non-goals

- Bumping `oxibrain-client` to 0.3 (unpublished) or vendoring it.
- Changing Foundation profiles/packages/credentials behavior.
- Auto-migration of legacy memory; `oxicode migrate brain` stays opt-in.
- Removing `PortMemoryBackend` or the SDK `EmbeddingProvider` port (SDK surface,
  consumed by oxios).

## Testing

- Unit: socket-path resolution (env override + canonical); backend gate
  (real `UnixListener` in tempdir → Some; missing → None); tool-name mapping
  via mocked `call_tool` JSON (hits parse, retract error path).
- TUI: chip rendering in `frame_layout` chrome test; `/memory` dispatch test.
- Gates: `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo clippy -p oxicode-sdk --features native-browser -D warnings`,
  `cargo nextest run --workspace`.
- Live smoke against the running daemon: `oxicode migrate brain --dry-run`
  must print `ok: oxibrain daemon connected`; a remember→search round trip in
  a `oxicode-selftest` space, then `redact` of the test episode.

## Decisions made autonomously (flagged for review)

1. `memory_enabled` default **true** (was false).
2. Memory gate = socket presence, not Foundation dir presence.
3. `delete` maps to `redact{target_kind:"episode", target_id}` — destructive,
   audited, works on the exact episode ids `put` returns. (Supersedes the
   earlier `retract{statement_id}` decision: retract only withdraws
   extraction-produced statements, which our puts never create on client
   0.2.0 — see decision 6.)
4. Whole-crate deletion of `oxicode-mnemopi` + deprecated `oxicode-vtui::vim`.
5. Scope default `personal` (daemon convention) instead of `"default"`.
6. **Sampling gap discovered live (major).** The daemon's `remember` tool
   triggers MCP sampling (§12.3): it answers the tool call with a
   server→client `sampling/createMessage` request and waits up to 120s.
   `oxibrain-client` 0.2.0 reads that request line as a malformed response
   ("missing result"), so every `remember` from oxicode would fail (while
   the note is silently ingested server-side). Consequences:
   - `put` maps to `ingest{extract:false}` — durable immediately, no
     sampling stall. Trade-off: no realtime entity/statements extraction
     until a sampling-capable client (0.3.0) lands.
   - `search`/`list` map to `recall{query, space, token_budget}` — recall
     surfaces both extracted statements AND raw episodes (recent-episodes
     layer); the `search` tool only returns entity hits and misses every
     unextracted note.
   - revisit both when upgrading to oxibrain-client 0.3.0.
7. **`migrate brain --dry-run` health line was always "degraded"** — it
   printed the cached post-construction state (`Unavailable`) without ever
   connecting (pre-existing since the Foundation v1 host landed). Fixed:
   dry-run now live-pings first; `handle_migrate` became async
   (`main`'s dispatch awaits it).
8. **Daemon restarted (pid 12322 → 75637).** My "wedged daemon" diagnosis
   was a false positive — my raw probe was missing the newline delimiter
   (the daemon is line-delimited JSON-RPC). The restart itself was harmless
   (same version, SQLite WAL), but the diagnosis was wrong; noted for
   honesty. Related hardening: `block_on_sync` in `brain.rs` now uses
   `block_in_place` when already inside a runtime — nested
   `Runtime::block_on` panics (would have hit the real, non-dry-run
   migration path too).
9. **Live round-trip test added** (`oxicode-cli/tests/brain_live.rs`,
   `#[ignore]`d): put→search→delete against the real daemon; ran green,
   selftest episodes redacted afterwards (both spaces verified empty).
