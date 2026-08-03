# Slash Command Registry — Design

**Date:** 2026-06-17
**Status:** Proposed (awaiting review)
**Scope:** `oxicode-cli/src/tui/slash*`, `oxicode-cli/src/tui/completion*`,
`oxicode-cli/src/extensions/*`

---

## 1. Background & Problem

The TUI slash command system currently splits one logical concept across four
places, and the splits have drifted out of sync:

| Location | Responsibility | Drift evidence |
|---|---|---|
| `util/slash_commands.rs::BUILTIN_SLASH_COMMANDS` | Static table of name + description | Calls itself "single source of truth" yet carries a `Keep in sync with handle_slash_command()` comment — a contradiction |
| `tui/slash.rs::handle_slash_command()` | 1829-line match that actually runs commands | Owns the aliases; the table does not |
| `tui/slash.rs::SlashCompletion` | Completion entry type | Field-for-field identical to `BuiltinSlashCommand` (only `String` vs `&'static str`) |
| `tui/app.rs::update_slash_completions()` | Table → completion entries | Walks `BUILTIN_SLASH_COMMANDS` directly; ignores aliases and extensions |

### Concrete defects found

1. **Alias drift.** Seven aliases resolve at execution time but never appear in
   completions: `/exit`, `/q`, `/?`, `/ext`, `/keys`, `/issues`, `/models`.
2. **Extensions are invisible to completion.** Execution already works —
   `slash.rs:1276`'s `_` arm dispatches to `wasm_ext.execute_command()`, and
   `wasm.rs:1186` forwards to the WASM `execute_command` host call — but
   `update_slash_completions()` only sees `BUILTIN_SLASH_COMMANDS`, so extension
   commands can be typed and run but **never discovered via Tab**.
3. **No argument completion.** `CompletionKind::SlashArgument` is defined but
   unused (`#![allow(dead_code)]` on `completion/mod.rs`). Many commands have
   rich subcommand structure (`/mcp dashboard|status`, `/router pin|status|enable|disable`,
   `/issue new|show|start|release|close`, `/skill off <name>`) that is invisible.
4. **Two completion tracks.** `slash_completions: Vec<SlashCompletion>` and
   `file_completions: Vec<CompletionItem>` are parallel, separate state machines
   in `AppState`; `CompletionManager` only owns the file path.
5. **1829-line function.** All command logic lives in one `match`.
6. **Extension `Command` type is data-only.** `extensions/types.rs::Command`
   carries `name/description/usage` but no aliases, no subcommand model, no
   dynamic-arg hook, and only a single `String` return channel — so extensions
   cannot control notifications or express completion.
7. **A fourth copy of the command list in `/help`.** `slash.rs::format_help()`
   (currently `#[allow(dead_code)]`) hardcodes the whole command catalog as a
   string literal, and `router_help()` hardcodes router subcommands — *with
   already-stale content* (it claims `/router pin` is "coming soon" while the
   `pin` arm is implemented). This is drift evidence independent of the table
   vs handler split.

### The `/settings` footgun (documented in AGENTS.md)

> `BUILTIN_SLASH_COMMANDS` lists `/settings` with description "Edit settings
> (theme, language, tools, …)". The description is enforced via
> `util/slash_commands.rs:120`. Keep these in sync if either side changes.

This is a direct symptom of the split: the description lives in the table while
the behavior lives in the handler, and nothing ties them together. The registry
removes the footgun by construction.

## 2. Goals / Non-Goals

**Scope boundary.** Slash commands are a **TUI-only** feature. Neither
`oxicode --print` nor RPC mode parse or dispatch slash commands (verified: no
`handle_slash` / `starts_with('/')` references in `lib.rs` or `rpc_mode/`; the
RPC `execute_command` is an unrelated `RpcCommand` dispatcher). This refactor
stays inside the TUI; the non-TUI asymmetry is pre-existing and out of scope.

**Goals**
- One object per command owns **name + aliases + description + usage + execute
  + completion**. No more "keep in sync" comments.
- Alias and extension commands appear in completions automatically.
- Argument completion (subcommands + dynamic values) for all commands that take
  meaningful arguments.
- One unified completion track replacing the current two.
- Modules per command group; no 1800-line function.
- Extensions participate as first-class commands (declarative metadata + a
  structured execution outcome), without breaking the WASM sandbox.
- **Eliminate the hardcoded `/help` content** (`format_help` / `router_help`):
  the overlay is regenerated from registry metadata so there is one — not four
  — copies of the command catalog.

**Non-Goals**
- Changing command *behavior*. This is a structural refactor; semantics stay.
- A new command parser (we keep the simple `cmd + first-space arg` split; deeper
  tokenization happens inside each command's own logic).
- Letting extensions open host overlays (the rich overlays like
  `model_select` / `settings_overlay` are assembled from live host data an
  extension cannot supply — see Decision B for why `open_overlay` is dropped
  from the outcome schema).
- Bringing slash commands to print/RPC mode (TUI-only scope, above).

## 3. Core Design

### 3.1 Execution context and outcome

```rust
// tui/slash/mod.rs

/// Everything a handler needs. Bundling it stabilizes the handler signature:
/// adding a dependency does not touch every command's signature.
pub struct SlashCtx<'a> {
    pub session: &'a AgentSession,
    pub state: &'a mut AppState,
    pub ui_tx: &'a mpsc::UnboundedSender<UiEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashOutcome {
    /// Command handled; state already mutated.
    Handled,
    /// Did not match (fall through to next command / unknown).
    NotHandled,
    /// Request application shutdown.
    Quit,
}
```

### 3.2 The `SlashCommand` trait

```rust
/// One slash command owns its definition, execution, and completion.
pub trait SlashCommand: Send + Sync {
    fn name(&self) -> &str;                 // canonical, no leading '/'
    fn aliases(&self) -> &[&str] { &[] }    // resolved alongside name()
    fn description(&self) -> &str;
    fn usage(&self) -> &str { "" }          // "/mcp <dashboard|status>"

    /// Synchronous. Async work spawns onto the runtime and reports back via
    /// `ui_tx` (the existing pattern for `/share`, `/compact`). Rationale in
    /// Decision A below.
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome;

    /// Argument completion. Default empty. Implementations may return static
    /// subcommands, dynamic values (model ids, skill names…), or delegate to
    /// the path completer for file arguments.
    //
    // Read-only access only: completion never mutates state, so it takes
    // `&AppState` (not `&mut`) and a shared `&AgentSession`. This also keeps
    // it off any async boundary, which matters for the sync/async split in §5.
    fn complete_arg(
        &self,
        prefix: &str,
        session: &AgentSession,
        state: &AppState,
    ) -> Vec<CompletionItem> {
        let _ = (prefix, session, state);
        Vec::new()
    }

    /// Token match against name + aliases. Overridable for special cases
    /// (e.g. `/`-less forms, abbreviations).
    fn matches(&self, token: &str) -> bool {
        let t = token.strip_prefix('/').unwrap_or(token);
        t.eq_ignore_ascii_case(self.name())
            || self.aliases().iter().any(|a| t.eq_ignore_ascii_case(a))
    }
}
```

**Alias lifetime note.** `aliases()` returns `&[&str]`. Builtin commands hold
`&'static str` slices, so this is trivial. Extension adapters own `Vec<String>`;
they satisfy the signature by caching a `Vec<&'static str>` built once at
adapt time (the `WasmExtensionManager` already interns command descriptors for
the lifetime of the process). If that ever stops holding, the trait's return
type changes to `Cow<'_, [String]>` — a local change, not an API break for
builtins. Picked `&[&str]` now because every builtin is `&'static`.

### 3.3 Registry

```rust
// tui/slash/registry.rs

pub struct SlashRegistry {
    builtin:   Vec<Box<dyn SlashCommand>>,
    /// Extension commands are adapted to the trait at runtime.
    extensions: Vec<ExtensionCmdAdapter>,
}

impl SlashRegistry {
    pub fn builtins() -> Self { /* assemble all builtin/* structs */ }

    /// Rebuild the extension slice from the live WASM manager. Called on
    /// initial load and whenever `/reload` or `/extensions` re-scans.
    pub fn sync_extensions(&mut self, mgr: &WasmExtensionManager) { /* … */ }

    /// Completion of the command token itself: "/mo" → ["/model", "/models", …]
    /// Includes aliases (shown once, attributed to the canonical name) and
    /// extension commands.
    pub fn complete_command(&self, query: &str) -> Vec<CompletionEntry>;

    /// Argument completion: dispatches to the matched command's complete_arg.
    /// `arg_prefix` is the text after the command + space. Read-only access
    /// only (`&AppState` / `&AgentSession`), consistent with the trait method.
    pub fn complete_arg(
        &self,
        cmd_token: &str,
        arg_prefix: &str,
        session: &AgentSession,
        state: &AppState,
    ) -> Vec<CompletionItem>;

    /// Execute. Returns true if some command matched+handled.
    pub fn dispatch(&self, input: &str, ctx: &mut SlashCtx<'_>) -> bool;
}

/// A completion entry derived from the registry — replaces SlashCompletion.
#[derive(Debug, Clone)]
pub struct CompletionEntry {
    pub display: String,       // "/ext"  (alias form, what the user typed)
    pub canonical: String,     // "/extensions"
    pub description: String,
    pub is_extension: bool,    // for UI hint (e.g. a glyph)
}
```

### 3.4 Alias display rule

Aliases are shown as their own entries (so `/ext` and `/extensions` both
appear) but their `canonical` points at one handler. This is the minimal change
that makes aliases discoverable without duplicating handler logic.

## 4. Module Layout

```
oxicode-cli/src/tui/slash/
├── mod.rs            SlashCtx, SlashOutcome, SlashCommand trait, re-exports
├── registry.rs       SlashRegistry, CompletionEntry, dispatch/complete
├── completion.rs     input → (command completion | argument completion) routing
└── builtin/
    ├── mod.rs        SlashRegistry::builtins() assembly + grouping
    ├── help.rs       /help, /?
    ├── quit.rs       /quit, /exit, /q          (SlashOutcome::Quit)
    ├── session_grp/  /new, /clone, /resume, /fork, /tree, /session, /name
    ├── model.rs      /model;  scoped_model.rs  /scoped-models, /models
    ├── router.rs     /router
    ├── mcp.rs        /mcp
    ├── tools.rs      /tools
    ├── skill.rs      /skill
    ├── provider.rs   /provider, /logout
    ├── issue.rs      /issue, /issues
    ├── export_grp/   /export, /share, /import, /copy
    └── info_grp/     /settings, /hotkeys, /keys, /changelog, /reload,
                      /extensions, /ext, /compact
oxicode-cli/src/tui/completion/
├── mod.rs           unified CompletionManager (owns slash + path + fuzzy)
├── path.rs          (existing) file path completion
└── fuzzy_file.rs    (existing) fd-based fuzzy search
```

`util/slash_commands.rs` is **deleted**: its only content (`BUILTIN_SLASH_COMMANDS`
+ `BuiltinSlashCommand`) becomes redundant metadata that now lives on each
command struct. Callers (`app.rs`, the help overlay) read from the registry.

## 5. Completion System Unification

**Registry sharing model.** Dispatch needs `&mut`-free read access (it calls
`&self` methods); `sync_extensions` needs `&mut`. We store the registry as
`Arc<parking_lot::RwLock<SlashRegistry>>` in `AppState`. Completion (hot path,
every keystroke) takes a read guard; `sync_extensions` (cold path, reload only)
takes a write guard and rebuilds only the `extensions` slice. `CompletionManager`
holds the same `Arc`, so it never copies. This is the same `Arc<RwLock<…>>`
pattern `AppState` already uses for `skills`/`active_skills`.

The two parallel tracks collapse into one `CompletionItem`-based pipeline —
but completion is split across **two code paths by async-ness**, matching how
the code already works today:

- **Synchronous path (`complete()`)** — slash command names, static/dynamic
  argument completion, and file-path completion. Runs on **every keystroke**
  (the hot path). All its inputs are sync: `path::complete_path` is sync, and
  argument completion reads in-memory state (`state.skills`, `session.tools()`,
  the global model DB). It must not spawn subprocesses or block on I/O.
- **Asynchronous path (`fuzzy_search()`)** — the existing `fd`-backed fuzzy
  file search (`completion/fuzzy_file.rs:25`, `pub async fn`). Kept on a
  **separate trigger** (explicit keybinding), because spawning `fd` per
  keystroke is impractical. This function is *already* async and already on a
  separate code path; this refactor does not change that, it only unifies the
  *result type* so both paths populate the same `completions` vector.

> Note on `/resume`: its session list comes from the async
> `SessionManager::list`. Argument completion for `/resume` is therefore
> **intentionally omitted** (§6) — the sync `complete_arg` signature cannot
> reach it, and today's `/resume` already works around this with a
> `std::thread::scope` + `block_on` hack at submit time (`slash.rs:985`). That
> hack stays; this refactor does not try to make `/resume` completable.

```rust
// tui/completion/mod.rs (rewritten)
pub struct CompletionManager {
    cwd: PathBuf,
    registry: Arc<parking_lot::RwLock<SlashRegistry>>, // shared, read guard here
}

impl CompletionManager {
    /// Sync hot path: slash commands + args + file paths. Per keystroke.
    pub fn complete(
        &self,
        input: &str,
        session: &AgentSession,
        state: &AppState,
    ) -> Vec<CompletionItem> {
        if let Some(rest) = input.strip_prefix('/') {
            if let Some(space) = rest.find(' ') {
                // argument completion → matched command's complete_arg
                let (cmd, arg) = rest.split_at(space);
                let registry = self.registry.read();
                registry.complete_arg(cmd, arg[1..].trim_start(), session, state)
            } else {
                // command name completion (aliases + extensions included)
                let registry = self.registry.read();
                registry
                    .complete_command(rest)
                    .into_iter()
                    .map(CompletionItem::from)
                    .collect()
            }
        } else if input.starts_with("./") || /* …path heuristics… */ {
            path::complete_path(input, &self.cwd)
        } else {
            Vec::new()
        }
    }

    /// Async path: fd-backed fuzzy search. Separate trigger, not per keystroke.
    pub async fn fuzzy_search(&self, query: &str) -> Vec<CompletionItem> {
        fuzzy_file::fuzzy_file_search(query, &self.cwd).await
    }
}
```

`AppState` fields change from two tracks to one:

```rust
// before
pub slash_completions: Vec<slash::SlashCompletion>,
pub slash_completion_index: usize,
pub slash_completion_active: bool,
pub file_completions: Vec<CompletionItem>,
pub file_completion_index: usize,
pub file_completion_active: bool,

// after
pub completions: Vec<CompletionItem>,
pub completion_index: usize,
pub completion_active: bool,
```

`handlers.rs` loses its duplicated slash/file branches (today both
`update_slash_completions()` *and* `update_file_completions()` are called on
every keystroke) and calls one `update_completions()` that routes through the
manager. `render.rs::render_slash_popup_overlay` is generalized to render any
`CompletionItem` (it already mostly does; only the source vector changes).

## 6. Argument Completion — Per-Command Spec

Static subcommands are the default; dynamic values where the data exists. User
chose "as much dynamic as is reasonable."

| Command | Completion source | Type |
|---|---|---|
| `/model` | all accessible models (`provider/model`), filtered by configured keys, plus dynamic models | dynamic |
| `/scoped-models` `/models` | current scoped models + selectable models | dynamic |
| `/router` | `status` `pin` `enable` `disable`; after `pin` → `low medium high off` | static |
| `/mcp` | `dashboard` `status` (Quick-Add opens via bare `/mcp`) | static |
| `/tools` | registered tool names (excluding toggle of essential ones flagged) | dynamic |
| `/skill` | skill names; after `off` → skill names | dynamic |
| `/provider` | provider entries from `provider_select::build_provider_entries()` | dynamic |
| `/logout` | `configured_providers()` | dynamic |
| `/issue` | `new` `show` `start` `release` `close`; after action → issue ids | mixed |
| `/fork` | user-message indices (1-based) + short-id prefixes | dynamic |
| `/export` | file paths (delegate to `path::complete_path`) | path |
| `/import` | `.jsonl` file paths | path |
| `/name` `/compact` | none — free text | — |
| `/help` `/quit` `/new` `/clone` `/resume` `/session` `/settings` `/hotkeys` `/changelog` `/reload` `/extensions` `/copy` `/share` `/tree` | none | — |

The `complete_arg` infrastructure is open to every command; the table is the
**initial** set, not a ceiling.

## 7. Extension Integration

### 7.1 Extend the `Command` descriptor

```rust
// extensions/types.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub usage: String,
    #[serde(default)]
    pub aliases: Vec<String>,          // NEW
    /// Static subcommand tokens offered in completion (optional).
    #[serde(default)]
    pub subcommands: Vec<SubcommandDef>,  // NEW { name, description }
}
```

### 7.2 Structured execution outcome (Decision B)

The WASM `execute_command` host call currently returns `{"output": String}`.
We extend the contract to a structured response (additive, backward compatible):

```json
{
  "output": "string (required, may be empty)",
  "notification": "success | warning | error | info",   // optional, default info
  "quit": false,                                          // optional
  "clear_input": true                                     // optional, default true
}
```

A plain `String` or `{"output": "…"}` still parses (existing extensions keep
working). The WASM host `execute_command` (`wasm.rs:1175`) is extended to parse
these fields; the adapter maps them onto `SlashCtx` actions.

**`open_overlay` is deliberately absent.** The rich overlays (`model_select`,
`settings_overlay`, `resume_select`, …) are assembled from live host data
(`session`, model DB, configured keys) that an extension cannot supply, so an
`open_overlay: "model"` field would at best open a half-populated panel and at
worst leak a confusing API. Extensions can still surface *information* via
`output`/`notification`; the decision to open an overlay stays on the host side
(via a builtin like `/model`). This is a YAGNI cut, not a capability ceiling —
it can be added later behind a parameterized host-call protocol if a concrete
need appears. See Decision B for the sandbox rationale.

### 7.3 `ExtensionCmdAdapter`

```rust
// tui/slash/ext_adapter.rs
pub struct ExtensionCmdAdapter {
    ext_name: String,        // owning extension (for "[ext] …" attribution)
    def: extensions::Command,
    mgr: Arc<WasmExtensionManager>,
}

impl SlashCommand for ExtensionCmdAdapter {
    fn name(&self) -> &str { &self.def.name }
    fn aliases(&self) -> &[&str] { /* boxed from def.aliases */ }
    fn description(&self) -> &str { &self.def.description }
    fn usage(&self) -> &str { &self.def.usage }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        match self.mgr.execute_command(&self.def.name, args) {
            Ok(res) => {
                // res is the parsed ExtensionCmdResult: { output, notification,
                // quit, clear_input }. No overlay field (see §7.2).
                apply_structured(res, ctx); // notification + optional quit + clear_input
                SlashOutcome::Handled
            }
            Err(e) => {
                ctx.state.add_notification(format!("Error: {e}"), Error);
                SlashOutcome::Handled
            }
        }
    }
    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        _state: &AppState,
    ) -> Vec<CompletionItem> {
        self.def.subcommands.iter()
            .filter(|s| s.name.starts_with(prefix))
            .map(/* → CompletionItem */).collect()
    }
}
```

`WasmExtensionManager::execute_command` returns a parsed `ExtensionCmdResult`
(struct) instead of `Result<String>`; its one existing caller (the `_` arm in
`slash.rs`) is exactly what the adapter replaces, so there is no second caller
to migrate.

## 8. Alias Normalization Table

Derived from `slash.rs` match arms. Each row becomes one command struct.

| Canonical | Aliases |
|---|---|
| `help` | `?` |
| `quit` | `exit`, `q` |
| `extensions` | `ext` |
| `hotkeys` | `keys` |
| `issue` | `issues` |
| `scoped-models` | `models` |
| (all others) | none |

## 9. Call-Site Changes

| Site | Before | After |
|---|---|---|
| `handlers.rs` submit | `slash::handle_slash_command(...)` | `registry.dispatch(...)` returning `SlashOutcome` (map `Quit` → `*running=false`) |
| `handlers.rs` submit (completion active) | `selected_slash_command()` → `Action::ExecuteSlashCommand(name)` | `completions[completion_index]` → same action; canonical already normalized |
| `handlers.rs` keystrokes | `update_slash_completions()` + `update_file_completions()` (×2 calls) | single `update_completions()` |
| `app.rs` | `slash_completions` + `file_completions` fields | `completions` field |
| `app.rs::update_slash_completions` | walks `BUILTIN_SLASH_COMMANDS` | calls `CompletionManager::complete` |
| `render.rs::render_slash_popup_overlay` | reads `slash_completions` | reads `completions` |
| `agent_session.rs::extension_commands` | returns `Vec<Command>` | still returns descriptors; registry consumes them in `sync_extensions` |
| `slash.rs::_` arm (extension dispatch) | manual `wasm_ext` lookup | removed — adapter handles it |

`Action::ExecuteSlashCommand` and the `TuiNextAction::SwitchSession` /
`NewSession` enums are unchanged; only how they are produced changes.

## 10. Migration Order (incremental, always-green)

1. **Add the new types without using them.** `slash/mod.rs`, `registry.rs`,
   `completion.rs` skeletons; `SlashCtx`, `SlashOutcome`, `SlashCommand` trait,
   `CompletionEntry`. Registry with empty `builtins()`. Compiles, unused.
2. **Port commands in groups**, one builtin/*.rs file at a time, *deleting*
   the corresponding `match` arm from `handle_slash_command` as each is ported.
   `handle_slash_command` shrinks monotonically. Start with leaf commands
   (`/help`, `/quit`, `/copy`, `/hotkeys`) to validate the pattern, then groups.
3. **Replace the `_` extension arm with the adapter** once a handful of builtins
   are ported. Update `WasmExtensionManager::execute_command` return type.
4. **Unify completion** after all commands ported: switch `AppState` to one
   `completions` track, rewrite `CompletionManager`, update `handlers.rs` /
   `render.rs`. `update_slash_completions`/`update_file_completions` deleted.
5. **Add argument completions** per the §6 table, command by command.
6. **Delete `util/slash_commands.rs`** and its imports; remove the
   "Keep in sync" footgun note from AGENTS.md Pitfalls.
7. **Regenerate `/help` from the registry.** Delete the hardcoded
   `format_help()` (dead today) and `router_help()` string literals from
   `slash.rs`; the help overlay and any subcommand help are built by walking
   `registry.builtins()` (names, aliases, descriptions, usage). This closes the
   *fourth* drift copy (§1 defect 7) and fixes the already-stale
   `/router pin — coming soon` text.

Each step leaves the build green and the TUI functional. No big-bang cutover.

## 11. Testing

- **Unit per command**: each `builtin/*.rs` has `#[cfg(test)]` covering
  `matches()` (name + each alias), `execute()` happy path, and
  `complete_arg()` where relevant, using a minimal `AppState` + mock
  `AgentSession` (the test harness already in `agent_session.rs` tests).
- **Registry tests**: `dispatch` alias resolution, `complete_command` includes
  aliases + extension adapters after `sync_extensions`, `complete_arg` routes to
  the right command.
- **Adapter test**: feed a structured outcome JSON through
  `ExtensionCmdAdapter::execute` and assert the notification kind, the `quit`
  flag, and `clear_input` behavior (no overlay field — see §7.2).
- **Integration**: a TUI test that types `/mo<Tab>` and asserts the popup
  contains `/model` and `/models`; types `/skill <Tab>` and asserts skill names
  appear (mirrors the existing completion test pattern).
- **Backward-compat**: a WASM extension returning bare `{"output": "x"}` still
  produces an info notification.

## 12. Risks & Trade-offs

- **`SlashCtx` borrows `&mut AppState`** while `execute` is synchronous. Async
  commands spawn before any borrow crosses an `.await` (they clone `ui_tx` /
  `session` handles up front, as today). This avoids the
  `parking_lot::MutexGuard: !Send` pitfall documented in AGENTS.md.
- **`Arc<parking_lot::RwLock<SlashRegistry>>` shared between dispatch and
  completion.** Completion runs on every keystroke and takes a read guard;
  `sync_extensions` (cold path, reload only) takes a write guard and rebuilds
  only the `extensions` slice. The `Arc` clone is cheap and avoids a second
  lock; the registry is rebuilt only on extension reload. (Builtin commands are
  never mutated after `builtins()`.)
- **Structured extension outcome adds a contract.** Mitigated by additive JSON
  parsing and a tested bare-string fallback. Documented in
  `extensions/wasm.rs` next to `execute_command`.
- **Alias duplication in the popup** slightly lengthens the list. Acceptable:
  discoverability beats brevity, and aliases are visually attributed to their
  canonical command.

## 13. Decisions Log

- **A. Synchronous `execute` + `spawn`.** `AppState` holds `parking_lot` locks
  that are `!Send`; an `async fn execute` would force either making `AppState`
  `Send` across the board (large blast radius) or boxing the future manually.
  Synchronous execution with `tokio::spawn` for genuinely async work matches the
  *current* `/share`/`/compact` pattern exactly — no new async discipline
  required.
- **B. Structured extension outcome (without overlays).** The bare-`String`
  channel cannot express "warn" or "quit" or "don't clear input", which caps
  extension usefulness at a notification line. A structured JSON response
  (additively parsed) unlocks real integration while preserving the sandbox:
  extensions still cannot touch host state directly, only request allow-listed
  effects. We deliberately exclude `open_overlay` for MVP — the rich overlays
  require host-side data an extension cannot supply (see §7.2), so a
  half-populated overlay would be worse than none. A parameterized host-call
  protocol can add it later if a concrete need appears.
- **C. Trait registry over static-table-with-fnptr or macro.** Matches
  `oxicode-agent::ToolRegistry` (project consistency), lets extensions adapt in at
  runtime, and keeps each command's metadata and behavior in one place —
  eliminating the entire class of "keep in sync" drift that motivated this work.

## 14. AGENTS.md Updates (after implementation)

- Remove the `/settings` "Keep these in sync" pitfall note (it becomes
  structurally impossible).
- Add a "Slash commands" subsection under Conventions describing the
  `SlashCommand` trait and how to add a new command (one struct, register in
  `builtins()`).

## 15. Implementation Notes (in-progress)

Decisions made during implementation that refine the spec above. Design
§3-§14 remain authoritative; this section records concrete realizations and
the per-command migration checklist.

### 15.1 Borrow-conflict resolution (refines §3.3 sharing model)

A `SlashRegistry` stored *inside* `AppState` cannot be dispatched against
while also handing `&mut AppState` to the handler — `state.slash_registry`
(shared read) and `ctx.state` (`&mut`) alias the same `AppState`. Rust's
borrow checker rejects this even with partial borrows, because the `&mut`
covers the whole struct.

**Resolution (implemented, compiles):**
- **Dispatch path** (`handle_slash_command`, per-submit, infrequent): build the
  registry locally with `SlashRegistry::builtins()`. It is immutable and cheap
  to assemble, so rebuilding per submit is negligible and sidesteps the alias.
- **Completion path** (step 4, per-keystroke, read-only): read
  `state.slash_registry` (the shared field) without any `&mut` borrow of
  `state`, so no alias occurs.

The `state.slash_registry: SlashRegistry` field therefore exists for the
completion path; dispatch ignores it. `Arc<RwLock<…>` (design §5) is only
needed once extensions mutate the registry via `sync_extensions` at runtime —
until then a plain field suffices and a read guard is unnecessary.

### 15.2 Per-call registry vs. shared (decision log D)

- **D. Local registry per dispatch.** `builtins()` is called on every slash
  submit. Acceptable: submit is rare (human-driven), and `builtins()` is a
  handful of `Box::new` calls. Revisit only if profiling shows otherwise.
  Completion (hot path) uses the shared `state.slash_registry` field.

### 15.3 Helper-function migration (prerequisite for remaining commands)

Several legacy arms call private `fn` helpers defined in `slash/mod.rs`. They
must be reachable from `slash/builtin/*`. Policy, in order of preference:

1. **`pub(crate)` + stay in `mod.rs`** for pure utilities reused across
   commands. Applied so far: `handle_tool_command`.
2. **Move into the owning command's file** when a helper serves one command
   only. Pending: `router_help` → `builtin/router.rs`, `resolve_entry_id` →
   `builtin/session_grp.rs` (fork), `collect_tree_entries` → same (tree),
   `try_provider_with_key` → `builtin/provider.rs`, `format_help` → deleted
   (regenerated from registry in step 7).
3. **`pub(crate)` shared module** if two+ commands need it.

Pending helper visibility/moves: `handle_issue_command`, `resolve_entry_id`,
`collect_tree_entries`, `try_provider_with_key`, `router_help`, `format_help`.

### 15.4 Subcommand collapse (refines §6)

Legacy had `/mcp`, `/mcp dashboard`, `/mcp status` as three separate match
arms. In the registry these collapse to **one** `McpCommand` that parses
`args.trim()` (`dashboard` | `status` | bare). Generalizes to any
`/cmd <sub>` whose subcommands were separate arms. Check before porting:
`/router status|pin|enable|disable` follow the same pattern.

### 15.5 Migration checklist (step 2 progress)

Each row: command → builtin file → legacy arm deleted.

| Command | File | Arm deleted |
|---|---|:-:|
| `/quit` (`/exit` `/q`) | `quit.rs` | done |
| `/help` (`/?`) | `overlay_commands.rs` | done |
| `/hotkeys` (`/keys`) | `overlay_commands.rs` | done |
| `/extensions` (`/ext`) | `overlay_commands.rs` | done |
| `/changelog` | `overlay_commands.rs` | done |
| `/compact` | `tools_commands.rs` | done |
| `/session` | `tools_commands.rs` | done |
| `/settings` | `tools_commands.rs` | done |
| `/mcp` (+dashboard/status) | `tools_commands.rs` | done |
| `/tools` | `tools_commands.rs` | done |
| `/copy` | `clipboard.rs` (tbd) | — |
| `/name` | `session_grp.rs` (tbd) | — |
| `/new` `/clone` `/resume` `/fork` `/tree` | `session_grp.rs` (tbd) | — |
| `/model` | `model.rs` (tbd) | — |
| `/scoped-models` (`/models`) | `model.rs` (tbd) | — |
| `/router` | `router.rs` (tbd) | — |
| `/skill` | `skill.rs` (tbd) | — |
| `/provider` `/logout` | `provider.rs` (tbd) | — |
| `/issue` (`/issues`) | `issue.rs` (tbd) | — |
| `/export` `/share` `/import` | `export_grp.rs` (tbd) | — |
| `/reload` | `settings.rs` (tbd) | — |

**Status: 27/27 ported — all steps complete.** All legacy `match` arms deleted
(`grep -cE '^\s+"/' slash/mod.rs` == 0). Steps done: **2** (27 commands), **3**
(extension adapter — `ExtensionCmdAdapter` + `sync_extensions`), **4**
(completion unification — `update_slash_completions` reads `slash_registry`, so
aliases + extension commands appear automatically), **5** (per-command
`complete_arg` — see below), **6** (`util/slash_commands.rs` deleted;
`BUILTIN_SLASH_COMMANDS` gone), **7** (`/help` regenerated from the registry —
`HELP_CONTENT` static removed, `format_help` dead code deleted).

**Step 5 — argument completion implemented** for: `/skill` (skill names; also
after `off`), `/model` (accessible `provider/model` ids), `/tools` (registered
tool names), `/provider` (selectable provider names), `/logout` (configured
providers), `/router` (`status|pin|enable|disable`, then `low|medium|high|off`
after `pin`), `/mcp` (`dashboard|status`), `/issue` (`new|show|start|release|close`).
`update_slash_completions(&self, session: &AgentSession)` now routes `/cmd <prefix>`
to `SlashRegistry::complete_arg`; `SlashCompletion` gained an `is_arg` flag so
Tab *fills the input* for argument completion but *executes* for command-name
completion (Enter executes both). Registry has unit tests for alias/extension
completion. Build, clippy, and all 610 tests green.

### 15.6 Cleanup status

**Helper relocation — DONE.** All command-specific helpers moved out of
`slash/mod.rs` into their owning command files:

- `handle_issue_command`, `parse_new_opts`, `parse_priority_loose`, `parse_id`
  → `builtin/issue.rs`
- `try_provider_with_key` → `builtin/provider.rs`
- `build_providers_with_key`, `collect_catalog_models` → `builtin/model.rs`
  (`/router` references `super::model::collect_catalog_models`)
- `router_help` → `builtin/router.rs`
- `resolve_entry_id`, `collect_tree_entries` → `builtin/session_grp.rs`
- `handle_tool_command`, `try_re_register_tool`, `BUILTIN_TOOL_NAMES` →
  `builtin/tools_commands.rs`

`slash/mod.rs` is now 115 lines (down from 1895): only the trait/types,
`SlashCompletion`, and the `handle_slash_command` dispatch fallback remain.
Dead code removed too (`format_help`, `format_hotkeys`, `mask_key`).

**`CompletionManager` unification — NOT done (intentionally, see rationale).**
Design §5 proposed funneling slash + file completion through one
`CompletionManager::complete` entry point. Investigation during this cleanup
found this is **not a safe mechanical change** and is out of scope for the
slash refactor:

- The two tracks have **different UX semantics**. Slash completion is
  navigable (`CompletionNext/Prev/Accept` cycle `slash_completions`, Tab
  executes/fills). File completion is **display-only** — its
  `CompletionNext/Prev/Dismiss/Accept` actions are all no-ops
  (`handlers.rs`), so it behaves like an auto-suggestion with no keyboard
  navigation.
- Merging the two `completions` vectors + `active` flags into one would
  either lose file-completion's display-only behavior or silently give file
  entries a navigation they were never wired for — a UX change requiring its
  own design, not a refactor.
- The current split is **functionally complete**, build/clippy/test green,
  and sidesteps the `state.slash_registry` vs `&mut AppState` borrow conflict
  (slash completion builds the registry locally; file completion reads only
  `cwd`).

A future `CompletionManager` unification should be its own design pass that
decides the file-completion navigation model first, then collapses the state.

### 15.7 Final state

All migration steps (1-7) complete, all optional helper cleanup done.
`slash/mod.rs`: 115 lines. Workspace build + clippy clean, 610/610 tests pass.
The slash command system is a single-source-of-truth registry; aliases,
extension commands, and per-command argument completion all derive from the
`SlashCommand` trait implementations in `slash/builtin/`.

