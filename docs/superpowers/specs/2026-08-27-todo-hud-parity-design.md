# Todo HUD/Session Parity with omp — Design

Status: draft, pending user approval
Author: agent session, 2026-08-27

## 1. Problem

`oxicode-agent/src/tools/todo.rs` is a faithful port of omp's `tools/todo.ts`
op contract (9 ops, phase normalization, in-progress auto-promotion, markdown
round-trip, stop-reminder cap, subagent-description matching helper). The data
model already carries `TodoItem.notes: Option<Vec<String>>` and
`TodoItem.block_reason: Option<String>`.

The TUI presentation layer consumes almost none of this. `render_todo_pane`
(`oxicode-cli/src/tui_vt/main_loop.rs:5901-5940`) flattens every phase into one
list, prints `marker + text`, and strikes through only `Completed`. There is no
phase grouping, no progress visualization, no collapse, no compact mode, no
`/todo` command, no subagent linkage, no auto-clear, and no eager/mid-run
session policy. This spec closes that gap end-to-end, matching the reference
behavior in `can1357/oh-my-pi` (`packages/coding-agent/src/modes/interactive-mode.ts`,
`todo-tracker.ts`, `todo-command-controller.ts`, `todo-reminder.ts`,
`tools/todo.ts`).

## 2. Scope

Five components, all in this spec, implemented and landed together:

- **A. HUD renderer rewrite** — phase tree, progress spine, collapse/expand,
  compact mode, blocked/notes display.
- **B. `/todo` slash command** — show/expand/collapse/copy/export/import/
  append/start/done/drop/rm, fuzzy phase/task matching.
- **C. Subagent linkage** — live "matched" highlight + auto-reconcile on
  subagent completion.
- **D. Session policy** — eager-todo prelude (incl. provider `tool_choice`
  forcing), mid-run nudge, transcript-committed stop-reminder banner,
  auto-clear timer.
- **E. Settings** — `todo_eager_mode`, `todo_reminders_enabled`,
  `todo_reminders_max` (replaces the `MAX_TODO_STOP_REMINDERS` constant),
  `todo_clear_delay_secs`.

Non-goals: `/todo edit` in `$EDITOR` (oxicode's TUI has no external-editor
integration point today — export/import to `TODO.md` covers the same need
without inventing one); web/collab renderer (no oxicode-cli web surface);
ACP mode (oxicode-cli has no ACP mode).

## 3. Component A — HUD renderer

### 3.1 Data flow

Replace `flatten_todo_items` (drops phase/notes/block_reason) with a renderer
that consumes `&[TodoPhase]` directly. `RenderState.todo_items` becomes
`RenderState.todo_phases: Vec<TodoPhase>` (the type already returned by
`TodoStateProvider::get_phases()` — no new type needed, just stop flattening).

### 3.2 Layout (ports `interactive-mode.ts:2529-2670`)

```
TODO
 ├─ I. Foundation · 2/2
 │   ✓ Scaffold workspace
 │   ✓ Wire CI
 ├─ II. Auth  · 1/3
 │   ✓ Port credential store
 │   ▸ Wire OAuth providers
 │   ☐ Handle token refresh (blocked: waiting on vendor sandbox)
 │     … 1 more todo
 └──── (spine fills proportionally to closed/total across all phases)
```

- Single-phase lists: no roman-numeral header, no phase progress row — plain
  task list under the `TODO` header (matches `multiPhase` branch).
- Multi-phase: roman-numeral header (`phases_to_markdown`'s `ROMAN_PAIRS` /
  `roman_numeral` already exist in `todo.rs` — reuse, don't reimplement) +
  `done/total` count. Active phase (first with pending/in-progress work, else
  last) is bold+accent; others are dimmed and — when collapsed — render only
  the header row.
- Collapsed (default): active phase's tasks run a "walking viewport" —
  `in_progress` first, then up to 4 following `pending`, with a trailing
  `… N more todos` row; one prior closed task is always kept visible directly
  above so a completion is seen, not silently dropped. Port `selectCollapsedTodos`
  / `selectWithinCap` from `todo.ts:242-350` verbatim as a pure function in
  `todo.rs` (`select_collapsed_todos`), unit-testable independent of rendering.
  Non-active phases beyond a cap of 4 collapse into `… N more stages`.
- Expanded (`/todo expand` or `Settings.todo_expanded` session flag — no
  persistence needed beyond the session, matches omp): every phase, every task.
- Blocked: `☐ <content> (blocked: <block_reason>)` in warning color, no
  strikethrough. Falls back to `(blocked)` with no reason when
  `block_reason` is `None`.
- Abandoned: strikethrough in error color (currently missing — bug fix).
  Completed: strikethrough in success/foreground color (existing behavior,
  kept).
- Notes: append a dim marker (`·2` for 2 notes) when `notes` is non-empty.
  Content of notes is not shown inline (matches omp: only a count marker).
- Progress spine: a vertical tree (`├─`/`│`/`└`) to the left of phase/task
  rows, filled in accent color proportional to `closed_tasks / total_tasks`
  across the *whole* list (not just the active phase), tail clamped so it
  never fully empties (`closed_tasks > 0` ⇒ at least one cell lit) or fully
  fills before every task is closed. Add `theme.tree` box-drawing glyphs to
  `oxicode-vtui::theme` if not already present (checked: `oxicode-vtui/src`
  has no existing tree-glyph module — new addition, three chars only: branch
  `├─`, vertical `│`, hook `└─`).

### 3.3 Compact mode

Port `TODO_COMPACT_TERMINAL_ROWS_THRESHOLD = 18` and `isCompactTodoMode()`:
when `frame.area().height < 18`, the todo pane collapses into a single line
merged with the row above the composer: `TODO 3/7 · <current task>` (accent
header, dim count, then the active task's `#formatTodoLine` — or
`✓ done` when nothing is open). Reuses the same "pinned_area" row the full
HUD would otherwise occupy — no extra vertical cost on short terminals.

## 4. Component B — `/todo` slash command

New `oxicode-cli/src/tui_vt/slash/todo_command.rs` (or added to
`commands.rs` per the "catalog/introspection" convention in AGENTS.md — this
is mutation-heavy so a dedicated file matches the `SessionsCommand` precedent
of one file per stateful command). Implements `SlashCommand` with verbs
ported from `todo-command-controller.ts` / `slash-commands/helpers/todo.ts`:

| Verb | Behavior |
|---|---|
| (none) | Print current phases as markdown (`phases_to_markdown`) via `ctx.reply` |
| `expand` / `collapse` | Toggle a new `RenderState.todo_expanded: bool` |
| `copy` | Copy `phases_to_markdown` output to the system clipboard — oxicode-cli has no existing clipboard dependency; add `arboard` (already MIT/permissive, small, cross-platform) or shell out to `pbcopy`/`xclip`/`clip` matching platform. **Decision needed**: see open question §7.1. |
| `export [path]` | Write markdown to `path` (default `TODO.md` in cwd) |
| `import [path]` | Read + `markdown_to_phases`, replace session's todo phases via `TodoState` |
| `append [phase] task...` | Fuzzy-match phase (exact → prefix → substring, ambiguous ⇒ error) or create; title-case new phase names |
| `start <task>` | Fuzzy task match → `TodoOp::Start` |
| `done` / `drop` / `rm` `[task\|phase]` | No arg ⇒ apply to all; else fuzzy task match first, then fuzzy phase match; ambiguous ⇒ error listing candidates |

Fuzzy matching and the quote-aware tokenizer are pure functions ported
verbatim from `todo-command-controller.ts:37-115` into `todo.rs` (already the
module owning `resolve_targets`, which is a superset of the "task substring
match" logic — factor `find_task_fuzzy`/`find_phase_fuzzy` there and reuse
from both the slash command and, where useful, `resolve_targets`'s ambiguity
message). All slash-command mutations route through the same
`TodoStateProvider::apply_ops` the agent tool uses — single source of truth,
matches oxicode's existing `todo_state.rs` design intent.

`/todo help` prints the usage block (ported from `USAGE` in
`todo-command-controller.ts:18-30`, adjusted for oxicode verbs).

## 5. Component C — Subagent linkage

`todo_matches_any_description` (`todo.rs:500`) already exists and is unit
tested but has zero call sites — this wires it in.

- **Matched highlight**: the HUD renderer accepts an `is_matched: impl Fn(&TodoItem) -> bool`
  closure built from `AgentPoolProvider::list_agents()` (already in
  `oxicode-agent/src/tools.rs:194`) filtering to running sub-agents and
  collecting their descriptions, then calling `todo_matches_any_description`.
  A `Pending` task that matches renders in accent instead of dim (mirrors
  `interactive-mode.ts:2543` `isMatched`).
- **Auto-reconcile**: on every frame where the agent pool snapshot changes
  (or simply every frame — the match is cheap and `AgentPoolProvider` is
  already polled per-frame for the Hub overlay per AGENTS.md), scan
  *completed* sub-agents' descriptions and flip any `Pending`/`InProgress`/
  `Blocked` task whose content matches to `Completed` (clearing
  `block_reason`), same rule as `interactive-mode.ts:2369-2404`: only
  successful completions auto-close; failed/aborted subagents leave the task
  open. Add `reconcile_with_subagents(phases, completed_descs) -> (Vec<TodoPhase>, bool)`
  to `todo.rs`, called from the TUI frame-refresh path
  (`main_loop.rs:~1407`, alongside the existing `get_phases()` call) — commits
  back through `TodoStateProvider` only when `bool` (mutated) is true, to
  avoid needless writes.

## 6. Component D — Session policy

### 6.1 Settings (new fields in `oxicode-cli/src/store/settings.rs`,
`omp-adoption-2` block, following the existing `default_true`/`default_false`
helper convention)

```rust
/// How strongly to auto-create a todo list on the first turn.
/// "off" (default) | "preferred" (soft nudge) | "always" (forced tool_choice).
#[serde(default)]
pub todo_eager_mode: TodoEagerMode,

/// Remind the agent to finish open todos before it stops. Default: true.
#[serde(default = "default_true")]
pub todo_reminders_enabled: bool,

/// Max stop-time todo reminders per run. Default: 3 (mirrors
/// `MAX_TODO_STOP_REMINDERS`, which this setting now overrides at runtime).
#[serde(default = "default_todo_reminders_max")]
pub todo_reminders_max: u32,

/// Seconds after every todo is closed before the HUD auto-clears.
/// 0 = instant, default: 60. `None`/negative disables auto-clear.
#[serde(default = "default_todo_clear_delay_secs")]
pub todo_clear_delay_secs: i64,
```

`TodoEagerMode` is a 3-value enum (`Off`, `Preferred`, `Always`) — same shape
as omp's `todo.eager` (`default`/`preferred`/`always`), renamed to fit
oxicode's `snake_case` convention and avoid clashing with the Rust keyword
`default` as a variant name.

`MAX_TODO_STOP_REMINDERS` in `todo.rs` becomes the *default* for
`todo_reminders_max`; `build_stop_reminder`'s call site in
`agent_loop/mod.rs:1240` passes `config.todo_reminders_max` instead of the
constant. `agent_loop` gains a `todo_reminders_enabled: bool` /
`todo_clear_delay_secs: i64` on `AgentLoopConfig`, threaded from settings the
same way `todo` (the provider) already is.

### 6.2 Stop-reminder banner (transcript-committed, not just context injection)

Today `build_stop_reminder`'s text is injected into agent context only — the
user never sees *why* the agent kept going. Add a `TodoReminderEvent` variant
(or reuse whatever event enum already carries `agent_loop` → TUI
notifications — confirm during implementation) so `main_loop.rs` commits a
warning-styled block to scrollback via the existing `render_committed_chunk`
path (`main_loop.rs:5182`), mirroring `TodoReminderComponent`
(`todo-reminder.ts`): header `⚠ N incomplete todos — reminder A/B`, then the
unchecked list, inverse/warning background. This is additive to the existing
context-injection behavior, which is unchanged.

### 6.3 Mid-run nudge

Port `MID_RUN_NUDGE_MUTATION_THRESHOLD = 12`, `MID_RUN_NUDGE_MAX_PER_CYCLE = 2`
from `todo-tracker.ts`. Track mutating-tool-call count since the last `todo`
touch in `agent_loop`'s per-turn state (mirrors `#mutationsSinceLastTouch` /
`onToolResult`); when the threshold is hit and budget remains, inject a
hidden developer-role reminder message (same `Message { role: developer, .. }`
shape `build_stop_reminder`'s caller already uses — no new message-role
plumbing needed, unlike the eager prelude below) nudging the agent to
reconcile its todo state. No transcript banner for this one (matches omp:
mid-run nudges are silent).

### 6.4 Eager todo prelude — requires `ToolChoice` on the provider layer

This is the one component that touches `oxicode-ai`, not just `oxicode-agent`/
`oxicode-cli`. Confirmed: `Context` (`oxicode-ai/src/context.rs`) has `tools:
Vec<Tool>` but no `tool_choice` field, and `Provider::stream` has no forcing
concept. Each of the 8 providers builds its tool payload independently
(`build_tools`/`build_anthropic_tools`/`build_bedrock_tool_config`/etc.).

Plan:
1. Add `pub enum ToolChoice { Auto, Named(String) }` to `oxicode-ai` (public,
   `#[oxicode_stable]` since it's a new public API surface) and
   `pub tool_choice: Option<ToolChoice>` to `StreamOptions`
   (`oxicode-ai/src/providers/options.rs:113`) — not `Context`, since
   tool_choice is a per-request override, not conversation state, matching
   where `temperature`/`max_tokens` already live.
2. Per-provider mapping, one match arm each, in the 6 native-function-calling
   providers (openai, openai-responses, anthropic, google/vertex share
   `google_shared.rs::convert_tools`, azure, bedrock): `Auto` ⇒ existing
   behavior (omit the field); `Named(name)` ⇒ that API's forced-tool-choice
   shape (`{"type":"function","function":{"name":..}}` for OpenAI-shaped,
   `{"type":"tool","name":..}` for Anthropic-shaped, etc.). Ollama and the
   in-band XML/owned dialects (per the existing comment in
   `streaming.rs:84-86`, "any tool_choice would error on a tools-less
   request") **do not support forcing** — return `None`/ignore, same as omp's
   `buildNamedToolChoice(model)` returning `undefined` for unsupported
   models.
3. `oxicode-agent`: add `TodoTracker`-equivalent state (new
   `oxicode-agent/src/agent_loop/todo_policy.rs`, ported from
   `todo-tracker.ts`'s eager-prelude half only — reminders/mid-run nudge stay
   in `agent_loop/mod.rs` next to the existing `build_stop_reminder` call per
   §6.2/6.3, avoid inventing a second todo-state owner). On the first
   assistant turn, if `todo_eager_mode != Off`, no todo phases yet, not a
   sub-agent, and the prompt doesn't look like a question (port the regexes
   in `todo-tracker.ts:24-30` — `QUESTION_PROMPT_RE` etc. — as-is, they're
   already language-agnostic via the non-ASCII fallback), inject a developer
   message nudging todo creation; when `Always` *and* the resolved model's
   provider supports `ToolChoice::Named`, also set
   `StreamOptions.tool_choice = Named("todo")` for that one request.
4. New prompt text: `oxicode-agent/src/prompts/eager_todo.md` (or inline
   constant, matching oxicode's existing convention — check whether other
   prompt fragments are files or `const &str` before deciding; follow
   whichever is already used for `tool_guidance` in
   `agent_session_runtime.rs:1020`).

This is the highest-risk component (8-provider surface, new public API). It
ships in this spec per user decision, but lands as its own commit/PR-sized
unit so a regression in one provider's `tool_choice` mapping doesn't block
the other four components.

### 6.5 Auto-clear timer

When `#is_todo_list_settled` (every task `Completed`/`Abandoned`) and
`todo_clear_delay_secs >= 0`: start a timer (reuse whatever timer primitive
the existing `MODEL_CYCLE_TRACK_CLEAR_MS`-equivalent uses in `main_loop.rs` —
check for a precedent before adding `tokio::time::sleep` directly into the
render loop; if none exists, a simple `Instant` deadline checked each frame
tick is enough, no new task needed). On expiry, clear
`RenderState.todo_phases` for HUD purposes only — does **not** mutate the
underlying `TodoState` (a `/todo` or `todo` tool call after auto-clear still
sees the historical phases; this is a HUD-only visibility timer, matching
omp's `this.todoPhases = []` which is `InteractiveMode`-local, not
`AgentSession`-persisted).

## 7. Open questions before implementation

### 7.1 Clipboard for `/todo copy`

oxicode-cli has no clipboard dependency today. Options: (a) add `arboard`
crate (~200KB, no unsafe FFI on macOS/Linux/Windows via native APIs), (b)
shell out to `pbcopy`/`xclip -selection clipboard`/`clip.exe` detected by
`cfg(target_os)`. Recommend (b) — zero new dependencies, matches oxicode's
general preference for avoiding new crates when a single syscall suffices,
and `pbcopy` is guaranteed present on the primary dev platform (macOS per
AGENTS.md workstation).

### 7.2 Hidden developer-role messages for the eager prelude

`build_stop_reminder`'s existing injection already proves oxicode's message
model supports a role that's sent to the model but need not render in the
transcript (confirm exact mechanism during implementation — likely a
`developer`-role message that the TUI's transcript renderer already
skips/dims, since the stop-reminder text isn't currently visible per §6.2's
premise). If no such "invisible" mechanism exists yet, the eager-prelude
message will need the same treatment as the new stop-reminder banner in
reverse: a message the *agent* sees but the *transcript* does not render by
default (as opposed to omp's explicit `display: false` field). Flag during
implementation if this requires new plumbing in `oxicode-agent`'s message
types beyond what §6.2/6.3 already assume.

## 8. Testing

- `todo.rs`: unit tests for `select_collapsed_todos`/`select_within_cap`
  (port omp's collapse-policy test cases: active-task-leads, closed-context
  row, hidden-count summary), `reconcile_with_subagents` (successful match
  closes, failed/aborted does not, idempotent on already-closed tasks),
  `find_task_fuzzy`/`find_phase_fuzzy` (exact → prefix → substring →
  ambiguous-error ladder).
- `main_loop.rs`: existing `flatten_todo_items_preserves_order_and_status`
  test is replaced (flattening goes away) — add a `render_todo_pane` snapshot
  test asserting: phase headers appear for multi-phase, blocked reason
  renders, notes marker renders, compact-mode single-line fallback under 18
  rows.
- `oxicode-ai`: per-provider `tool_choice` mapping tests mirroring the
  existing `build_tools`/`convert_tools` unit tests (one assertion per
  provider: `Auto` omits the field, `Named` produces the provider's exact
  forced shape); a test confirming owned/XML dialects and Ollama silently
  ignore `tool_choice` rather than erroring.
- Slash command: `/todo` verb dispatch tests following the existing
  `SlashCommand` test pattern in `registry.rs`'s `mod tests`.
- Manual TUI smoke test (this surface has no automated visual test per
  AGENTS.md conventions): drive a real session through `browser`-unavailable
  TUI — use the `hub` process launcher to run `cargo run -p oxicode-cli`
  interactively, create a multi-phase todo list, verify tree spine, collapse/
  expand, compact mode (resize terminal < 18 rows), block a task with a
  reason, and confirm the HUD matches the layout in §3.2.

## 9. Rollout

Land as 5 sequential PRs/commits along the component boundaries (A–E), each
independently buildable and testable, in the order A → C → E → B → D (HUD
first since everything else displays through it; D last since it's the
highest-risk/highest-surface-area change). `todo_panel_enabled` (existing
setting) continues to gate the whole HUD; new settings default to
non-disruptive values (`todo_eager_mode: off` preserves today's behavior
exactly until a user opts in).
