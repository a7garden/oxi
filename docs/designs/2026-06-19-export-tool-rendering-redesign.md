# HTML Export Tool Rendering Redesign (C1)

**Date:** 2026-06-19
**Status:** Implemented
**Scope:** `oxi-cli/src/storage/export.rs`
**Related:** emoji audit (same session), review findings (same session)

## 1. Problem

`export.rs` claims to render tool calls in exported HTML, but the implementation
is structurally broken:

1. **Structural data is ignored.** Three structured tool representations are
   already persisted in sessions and never rendered by export:
   - `AssistantContentBlock::ToolCall { id, name, arguments }` — assistant
     message block, **skipped** (only `Text` blocks are extracted).
   - `AgentMessage::ToolResult { content, tool_call_id }` — **falls through to
     the `_ =>` branch and renders as a generic "System" message**; the
     `tool_call_id` is discarded.
   - `AgentMessage::BashExecution { command, output, exit_code, cancelled,
     truncated, full_output_path }` — same system fallback; rich metadata is
     discarded. **However, `BashExecution` is never produced by any code path**
     (verified: no `AgentMessage::BashExecution` construction exists in
     `oxi-cli`, `oxi-agent`, or `oxi-sdk`). It is a dead variant. Rendering it
     is therefore deferred to a future change that first adds a producer
     (see §3.7).

2. **A dead text parser runs instead.** `render_tool_blocks()` scans the
   assistant's free text for emoji-prefixed lines (`🔧 Running bash`, `📝 Writing`,
   `📄 Reading`, `✏️ Editing`, `🔍 Searching`, `📤 result:`). Nothing in the
   codebase emits that format — verified by grep across `oxi-cli`, `oxi-tui`,
   `oxi-agent`. The only producer is `export.rs`'s own unit tests
   (self-fulfilling).

3. **A second dead parser duplicates the same idea.** `render_markdown_with_options`
   matches `🔧 ` / `📤 ` prefixes to wrap lines in `.tool-call` /
   `.tool-result` divs. Same problem: no producer.

4. **Emoji as protocol markers is an anti-pattern.** Font-dependent, collides
   with LLM free-text output, unparseable on terminals without emoji support.

Meanwhile, `oxi-tui/src/widgets/tool_renderer.rs` already implements the correct
pattern — rendering from structured `(name, arguments)` and result text, used by
the live TUI. The fix is to bring export in line with that pattern.

This redesign covers `ToolCall` and `ToolResult` — the two structured
representations that are **actually produced** by `agent_session.rs`.
`BashExecution` is deferred (§3.7).

## 2. Goals / Non-Goals

**Goals**
- Render tool calls and results **from structured session data only**.
- Remove 100% of emoji-parsing code paths in `export.rs`.
- Reuse the existing CSS asset base (`.tool-call`, `.tool-result`, `.tool-bash`,
  `.tool-label`, `.tool-block`) — no visual regression.
- Honor `include_tool_calls` with a precise new meaning.
- Preserve the existing public API (`export_html_with_options`, `HtmlExportOptions`).

**Non-Goals**
- Changing what gets persisted in sessions (data shape is already correct).
- Touching `oxi-tui` rendering (already correct).
- Designing new export formats (HTML only).
- **Rendering `BashExecution` entries.** No code path produces them. Rendering
  is deferred until a producer is added (§3.7).
- Backward-compatible parsing of pre-redesign export HTML (out of scope — the old
  parser never matched real data anyway).
- Migrating old session JSONL that may embed emoji in assistant text (no such
  sessions are known to exist; the parser never matched, so nothing changes
  visually for them).

## 3. Architecture — Approach A (entry-order preserving, inline)

Tool calls and results are rendered **as they appear in the entry stream**,
mirroring the TUI's live behavior. No `tool_call_id` join pass is required,
because sessions are append-only and a `ToolResult` entry always immediately
follows the `Assistant` entry that contains the matching `ToolCall`.

### 3.1 What gets removed

| Symbol | Location | Reason |
|---|---|---|
| `render_tool_blocks` | ~L446–628 | Dead emoji parser, no producer. |
| `extract_path_from_line` | ~L629–652 | Helper for `render_tool_blocks` only. |
| `ToolOp` enum | ~L420–444 | Helper for `render_tool_blocks` only. |
| Emoji branch in `render_markdown_with_options` | ~L1131–1153 | Second dead emoji parser. |

> Line numbers are approximate and will drift. Identify symbols by name when
> implementing.

### 3.2 What is kept (reused as-is)

- All CSS (`.tool-call`, `.tool-result`, `.tool-block`, `.tool-label`,
  `.tool-bash`, dark/light variants) — untouched.
- `HtmlExportOptions`, `export_html_with_options`, `render_entry` signature —
  public surface unchanged.

The five old fused renderers (`render_bash_tool`, `render_file_read_tool`,
`render_file_write_tool`, `render_file_edit_tool`, `render_search_tool`) are
**removed**, not reused. They were built around the (command, output) fused
shape that approach A splits apart (§3.4). Their CSS class knowledge lives on
in the new `render_tool_call_block`.

### 3.3 What is added

Two focused renderers, each taking structured input:

```rust
/// Render an `AssistantContentBlock::ToolCall` as a `.tool-call` block.
/// `name` dispatches to the per-tool format (bash/read/write/edit/search);
/// unknown tools fall back to a generic label + JSON arguments.
fn render_tool_call_block(name: &str, arguments: &Value) -> String;

/// Render an `AgentMessage::ToolResult`'s content as a `.tool-result` block.
/// `tool_call_id` is emitted as a `data-tool-call-id` attribute so future
/// CSS/JS can visually group adjacent call/result pairs without a join pass.
fn render_tool_result_block(content: &ContentValue, tool_call_id: &str) -> String;
```

### 3.4 Call/result split

The old `render_bash_tool(command, output)` fused call and result into one
`.tool-block`. Under approach A they are separate entries, so the fused
functions are removed and replaced by call-only / result-only renderers:

- **`ToolCall` block** renders **only the call** (command / path / query).
  Gets `.tool-call` (+ `.tool-bash` for bash calls).
- **`ToolResult` block** renders **only the result** (output / content).
  Gets `.tool-result`.

### 3.5 `render_entry` restructure

```
match &entry.message {
    AgentMessage::User { content }            => (unchanged — text only)
    AgentMessage::Assistant { content, .. }   => iterate blocks IN ORDER:
        Text      → render_markdown_with_options
        ToolCall  → render_tool_call_block  (gated by include_tool_calls)
        Thinking  → existing <details> handling
        ImageResult / ToolPlan / Refusal → skip (out of scope)
    AgentMessage::ToolResult { content, tool_call_id }
                                              => render_tool_result_block
                                                 (gated by include_tool_calls)
    AgentMessage::System { content }          => (unchanged)
    AgentMessage::BashExecution { .. }        => existing _ => fallback
                                                 (deferred — see §3.7)
    AgentMessage::Custom { .. }               => existing _ => fallback
    _                                         => existing _ => fallback
}
```

The `_ =>` fallback shrinks: `ToolResult` gets an explicit arm.
`BashExecution` stays in the `_ =>` fallback until a producer exists (§3.7).

**Wrapper strategy.** Every entry currently renders as a
`<div class="msg msg-*">` wrapper. The redesign preserves this for
`User`/`Assistant`/`System`. `ToolResult` is the exception: it renders as a
**bare `<div class="tool-result">` block with no `msg-*` wrapper** and no role
header. Rationale: a tool result is a continuation of the tool-call flow, not a
conversational message. It should visually attach to the preceding assistant
message, not appear as an independent bubble. The `data-tool-call-id` attribute
is the stable grouping key.

### 3.6 Tool name dispatch

`render_tool_call_block(name, arguments)`:

| `name` | Rendering | Argument keys read |
|---|---|---|
| `bash` | `.tool-call.tool-bash` with `$ <command>` | `command`, `timeout?` |
| `read` | `.tool-call` with `read <path>` | `path`, `offset?`, `limit?` |
| `write` | `.tool-call` with `write <path> (new)` | `path` |
| `edit` / `edit_diff` | `.tool-call` with `edit <path>` | `path` |
| `grep` / `find` / `ls` | `.tool-call` with `[G]/[F]/[D] <query>` | `pattern`/`path`/`query` |
| _other_ | `.tool-call` generic label + JSON arguments | (raw display) |

The dispatch table covers the same tools as
`oxi-tui/tool_renderer.rs::format_tool_call` for the common built-in path.
Tools with TUI-specific formatting (`issue`, `web_search`, `generate_image`,
`subagent`, etc.) fall to the generic JSON fallback in export — a conscious
divergence, not a parity claim. The dispatch table is a `match` on `name`, so
extending it for new tools is a single arm.

### 3.7 `BashExecution` — deferred

`AgentMessage::BashExecution` is a structured variant with rich metadata
(command, output, exit_code, cancelled, truncated, full_output_path), but **no
code path produces it**. Both persistence routes in `agent_session.rs`
(`persist_session`, `persist_event_message`) write bash tool output as
`AgentMessage::ToolResult`, not `BashExecution`. The variant exists in the data
model and is matched in `session.rs::content()` and
`tui/overlay/tree_navigator.rs`, but is never constructed.

Rendering `BashExecution` now would reproduce the same anti-pattern this
redesign eliminates: a renderer for data no producer emits. The prerequisite is
to wire a `BashExecution` producer in the session persistence layer first
(e.g., in `agent_session.rs`, detect bash tool results and persist them as
`BashExecution` instead of — or alongside — `ToolResult`). Once a producer
exists, adding `render_bash_execution` and an explicit `render_entry` arm is a
small follow-up.

Until then, `BashExecution` entries (which can only appear in hypothetical old
JSONL files) fall through to the `_ =>` fallback and render as a plain system
message with the output text — identical to current behavior.

## 4. Data Flow

```
SessionEntry.message
   │
   ├─ Assistant { content: [Text, ToolCall, Thinking, ...] }
   │       └ iterate in order → markdown | render_tool_call_block | <details>
   │
   └─ ToolResult { content, tool_call_id }
           └ render_tool_result_block  → <div class="tool-result" data-tool-call-id="…">
```

No two-pass scan, no id join. Ordering falls out of append-only session layout.
`BashExecution` and other variants flow through the existing `_ =>` fallback.

## 5. `include_tool_calls` Semantics (new)

| Value | Behavior |
|---|---|
| `true` (default) | `ToolCall` blocks and `ToolResult` entries render normally. |
| `false` | Both are skipped entirely. Assistant `Text`/`Thinking` still render. User/System still render. |

This is precise and matches user intent ("hide tool noise"). The old emoji-line
filtering in `render_markdown_with_options` is removed; nothing filters on
prefixes anymore.

## 6. Error & Edge Cases

| Case | Handling |
|---|---|
| `ToolCall` with no following `ToolResult` (interrupted session) | Call block renders alone; no error. |
| `ToolResult` with no preceding `ToolCall` (corrupted session) | Result block renders alone; no error. |
| `arguments` is not an object / missing expected key | Fall back to generic block with raw JSON; never panic. |
| Unknown tool `name` | Generic `.tool-call` with `name` label + JSON. |
| `ContentValue::Blocks` with non-text blocks | Extract `Text` blocks only (mirrors User/System handling); ignore image blocks (out of scope). |

## 7. CSS

No CSS additions required. Existing classes cover all new markup:
`.tool-call`, `.tool-result`, `.tool-block`, `.tool-label`, `.tool-bash`.

## 8. Testing Strategy

### Tests to delete (they tested the dead parser)
- `export_renders_tool_calls_and_results` — relies on `🔧`/`📤` input.
- `export_options_skip_tool_calls` — same.
- The `bash_tool_renders_…`, `file_read_tool_…`, `file_write_tool_…`,
  `file_edit_tool_…`, `search_tool_…` unit tests — these test the
  old fused `render_*_tool(command, output)` directly. They are rewritten to
  target the new functions (see below).

### Tests to add (structural-input based)
1. **Assistant `ToolCall` block renders.** Build an `Assistant` entry with one
   `Text` + one `ToolCall { name: "bash", arguments: {command: "ls"} }`. Assert
   the HTML contains `<div class="tool-call` and the escaped command, and that
   the `Text` block still renders as markdown.
2. **`ToolResult` entry renders with `data-tool-call-id`.** Build a
   `ToolResult` entry; assert `.tool-result` and the id attribute appear.
3. **`include_tool_calls: false` skips tool entries.** Build a session
   mixing Assistant(Text+ToolCall), ToolResult; assert none of
   the tool markup appears, but Text does.
4. **Unknown tool name falls back to generic.** `name: "frobnicate"`, assert
   generic `.tool-call` + raw JSON.
5. **Block ordering preserved.** Assistant with `[Text("a"), ToolCall, Text("b")]`;
   assert "a" appears before the tool-call div appears before "b".
6. **`ToolResult` renders as a bare block, no `msg-*` wrapper.** Assert the
   `.tool-result` div is NOT inside a `<div class="msg` wrapper.
7. **Rewritten unit tests for the per-tool call renderers** (replacing the
   old fused-renderer tests), targeting `render_tool_call_block`.

All tests construct real `SessionEntry` / `AgentMessage` values — no string
fixtures that mimic a format nothing produces.

## 9. Migration & Compatibility

- **Session JSONL:** no change. Existing sessions render **better** (tool calls
  that were previously invisible or mis-rendered as "System" now render
  correctly). Sessions with no structured tool data render identically to before
  (the dead parser never matched them).
- **Exported HTML:** old exports are not re-parsed; nothing to migrate.
- **Public API:** `HtmlExportOptions`, `export_html*`, `export_to_html` —
  signatures unchanged.
- **`ToolResult` visual change:** `ToolResult` entries previously rendered as
  `msg-system` bubbles (via the `_ =>` fallback). They now render as bare
  `.tool-result` blocks. This is the intended improvement, not a regression.
- **`BashExecution`:** deferred (§3.7). Behavior unchanged — the `_ =>` fallback
  still handles it identically to today.

## 10. Implementation Order

1. Add `render_tool_call_block` and `render_tool_result_block` (+ shared
   `extract_text(content: &ContentValue)` helper for `ContentValue::Blocks`).
2. Add an explicit `ToolResult` arm in `render_entry`; iterate
   `Assistant` blocks in order with the new `ToolCall` path. `ToolResult`
   renders as a bare `.tool-result` block (no `msg-*` wrapper).
3. Delete `render_tool_blocks`, `extract_path_from_line`, `ToolOp`, and the
   markdown emoji branch.
4. Rewrite the affected tests (delete the emoji-input ones, add the
   structural-input ones).
5. `cargo fmt && cargo clippy --workspace -- -D warnings && cargo nextest run -p oxi-cli`.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Visual regression for the (never-actually-rendered) tool HTML | Old path never matched real data, so there is no real baseline to regress from. New markup reuses identical CSS classes. |
| A real session somewhere genuinely has `🔧`-prefixed assistant text | It will now render as ordinary markdown text (which is what it always was semantically). Acceptable; no data loss. |
| Tool name variants (e.g. `edit_diff` vs `edit`, provider-specific names) | Dispatch table covers known variants; unknown → generic. Extendable. |
| `ToolResult` bare-block wrapper breaks existing CSS layout assumptions | The `.tool-result` CSS class already handles block-level rendering; it was just never used with real data. The wrapper change is additive. |
