# TUI Upstream Ideas (omp 18.0.4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the 11 approved TUI ideas from upstream omp 18.0.4 into oxicode's production TUI (`oxicode-cli/src/tui_vt/` + `oxicode-vtui`).

**Architecture:** oxicode renders chat via ratatui `Viewport::Inline` on the main screen; finalized transcript rows are committed to native scrollback through `Terminal::insert_before` (`commit_scrollback`, main_loop.rs). Upstream omp's answers to the same problems (explicit history batches, backpressure, allocation ladder, incremental markdown, git TUI, keybindings, PTY, kitty images) are adapted onto this existing architecture — no rewrites of the render driver.

**Tech Stack:** Rust 2024, ratatui + crossterm, pulldown-cmark + syntect, unicode-width, tokio, cargo-nextest.

## Global Constraints

- Run from worktree `.worktrees/tui-omp-ideas`, branch `feat/tui-omp-ideas`.
- Every task ends green on: `cargo fmt --all` (no check — write), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run -p <affected crates>`, and for cli-touching tasks additionally `cargo clippy -p oxicode-cli -- -D warnings` (native-browser default-feature path).
- Conventional commit per task, message given verbatim in the task.
- Library crates (oxicode-vtui, oxicode-agent): typed errors only if public API needs them; tests may relax ONLY `clippy::unwrap_used` and `clippy::field_reassign_with_default` via existing crate-root cfg_attr.
- No emojis anywhere. No new public API without need; prefer crate-internal functions with `#[cfg(test)]`-reachable visibility (pub(crate) + unit tests in-module).
- Hardcoded counts forbidden in docs/comments; refer to symbols.
- `main_loop.rs` is owned by ONE task at a time — tasks T2..T11 that touch it run strictly in plan order.

---

### Task 1: Verify/fix Hangul Compatibility Jamo width

**Files:**
- Read: `Cargo.toml` (workspace deps), `Cargo.lock`
- Modify (only if bump needed): workspace `Cargo.toml` + affected `Cargo.lock`
- Test (only if bump needed): `oxicode-textarea` in-module tests

**Interfaces:**
- Produces: confirmed jamo-width behavior; nothing downstream depends on code changes here (likely a no-op verification task).

- [ ] **Step 1: Determine current unicode-width version**

Run: `cargo tree -i unicode-width --workspace 2>/dev/null | head -20; grep -A2 'name = "unicode-width"' Cargo.lock | head -6`
Expected: version(s) listed. If >= 0.2.3 (first release shipping Hangul Compatibility Jamo two-cell width fix), go to Step 3 (no-op exit). If older, continue to Step 2.

- [ ] **Step 2: (conditional) Write failing test then bump**

In `oxicode-textarea`, add a unit test asserting `unicode_width::UnicodeWidthChar::width('\u{3131}') == Some(2)` (Hangul Compatibility Jamo Kiyeok, U+3131 — omp 18.0.4 fixed this exact class for Orca cursor drift). Run `cargo nextest run -p oxicode-textarea` — if the test already passes with the current version, record no-op and skip to Step 3. If it fails, bump `unicode-width` to latest 0.2.x in the workspace dependency table (`cargo update -p unicode-width` may suffice if a single lock version exists), re-run until green.

- [ ] **Step 3: Record outcome**

Report states: either "no-op: unicode-width X.Y already carries jamo fix (test green)" or "bumped to X.Y, test green". No commit if no code changed; commit only if Cargo.toml/lock changed:

```bash
git add Cargo.toml Cargo.lock
git commit -m "fix(deps): bump unicode-width for Hangul jamo two-cell width"
```

### Task 2: Write-path width invariant (code block wrapping + final-stage clamp)

**Files:**
- Modify: `oxicode-vtui/src/tui/ui/markdown/mod.rs` (`render_code_block`, `render_markdown`)
- Create: `oxicode-vtui/src/tui/ui/clamp.rs`
- Modify: `oxicode-vtui/src/tui/ui/mod.rs` (module decl), `oxicode-cli/src/tui_vt/main_loop.rs` (clamp at write exits: `render_committed_chunk` + live viewport text rows)
- Test: in-module `#[cfg(test)]` in `markdown/mod.rs`, `clamp.rs`, and `main_loop.rs` test module

**Interfaces:**
- Produces: `pub(crate) fn clamp_segments_to_width(segs: &[InlineSegment], width: u16) -> Vec<InlineSegment>` in `oxicode-vtui/src/tui/ui/clamp.rs` (re-exported from `tui::ui` as `pub(crate)`); preserves styles, cuts at display-width boundary using `unicode-width`, zero-width chars never orphaned at line end (simplest correct rule: stop before the char that would overflow).
- Produces: `render_code_block(code: &str, lang: Option<&str>, ss: &SyntaxSet, width: usize)` — new `width` param; hard-wraps each highlighted line (tab expand to 4 spaces first; CJK-aware via unicode-width); `width == 0` preserves old behavior (no wrap). `render_markdown` passes its `width` through.

- [ ] **Step 1: Write failing tests**

In `markdown/mod.rs` tests: `code_block_hard_wraps_to_given_width` — 200-char ASCII line in a ``` fence, `render_markdown(&md, 80)` must yield every output row `UnicodeWidthStr::width(row) <= 80` and content preserved across the wrap (concatenated row text contains the full line). Also `code_block_width_zero_keeps_natural_lines`.

In `clamp.rs` tests (create module + tests first): `clamp_cuts_cjk_at_boundary` (mixed "한글" + ASCII string at width 5), `clamp_preserves_styles_of_kept_segments`, `clamp_zero_width_returns_empty`, `clamp_wider_than_content_is_identity`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p oxicode-vtui`
Expected: FAIL — `render_code_block` has no `width` param (compile error in test = acceptable RED), clamp module missing.

- [ ] **Step 3: Implement**

Add `width` param + wrapping in `render_code_block` (wrap AFTER highlight by segment-aware fold: accumulate segments' chars by display width; split segment text when crossing boundary). Create `clamp.rs` with the function. Wire `render_markdown` to pass width. In `main_loop.rs`: import clamp; apply in `render_committed_chunk`'s row emitter and in the live-viewport text-row render exit. Comment each site: `// Width invariant: no row exceeds the physical terminal width (omp tui-core-renderer.md §4).`

- [ ] **Step 4: Verify green + regression**

Run: `cargo nextest run -p oxicode-vtui -p oxicode-cli`
Expected: PASS including existing `table_fits_the_given_width_and_wraps_cells`.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "fix(tui): clamp all transcript rows to terminal width in the write path"
```

### Task 3: Optimistic user message rendering

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (Submit handling in `handle_inline_event`, agent-event reconciliation, `RenderState`)

**Interfaces:**
- Produces: `RenderState` field `pub optimistic_user: VecDeque<String>` (FIFO of unacknowledged echoes) + helper fns `push_optimistic_user(&mut self, text: &str)`, `reconcile_optimistic_user(&mut self, text: &str) -> bool` (front-match removes; returns whether matched), `expire_all_optimistic(&mut self)`.
- Consumes: the existing user-message transcript echo path used when a turn starts (find where a user prompt becomes `TranscriptLine` today — the agent worker event stream; if the TUI already echoes the user row on AgentEvent, reconcile against THAT; if the echo only happens for non-queued prompts, add optimistic row at enqueue and remove on echo).

- [ ] **Step 1: Write failing tests**

In `main_loop.rs` test module: `optimistic_user_pushed_on_submit` (call the Submit branch helper with a prompt while `is_streaming()` true → state contains optimistic entry AND a user-style TranscriptLine was appended), `optimistic_user_reconciled_by_echo` (push then reconcile same text → gone), `optimistic_user_expires_at_turn_end` (push two, call the TurnEnd handler → both gone).

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-cli optimistic` → FAIL (no such helpers).

- [ ] **Step 3: Implement**

At the Submit branch (~main_loop.rs:2756 `if session.is_streaming()`): after mirroring into `queued_inputs`, also `state.push_optimistic_user(&prompt)` and append the visible user row immediately (same rendering helper used for normal user echo), then force an immediate draw (set the same flag the render-tick branch checks; if a `render_now` signal doesn't exist, add `state.redraw_requested = true` consumed at loop top before select!). In the agent-event arm that emits the user echo, call `reconcile_optimistic_user(&text)` and skip duplicate echo when matched. At TurnEnd (where `drain_queue_head` is called, ~main_loop.rs:2291), call `expire_all_optimistic()`.

- [ ] **Step 4: GREEN + full cli tests** — `cargo nextest run -p oxicode-cli` PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(tui): optimistic user message rendering with event reconciliation"
```

### Task 4: Render coalescing + priority immediate draw

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (event loop, render tick arm, draw call site ~1415)

**Interfaces:**
- Produces: `fn coalesce_draw(state: &RenderState, last_draw_at: Instant, min_interval: Duration) -> DrawDecision` with `enum DrawDecision { DrawNow, SkipStale, Defer }` — pure function: `DrawNow` when priority flag set or interval elapsed; `Defer` otherwise. Priority flag: `RenderState::draw_priority: bool` set by user-visible events (submit, overlay open/close, queue pane toggle) and cleared on draw.

- [ ] **Step 1: Failing tests** (pure fn): `defer_within_interval`, `draw_now_on_priority_even_within_interval`, `draw_now_when_interval_elapsed`, `skip_when_no_dirty_state` (add `dirty: bool` to state touched by event handlers if not already trackable — if adding dirty tracking is invasive, drop this 4th case and the flag with it).

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-cli coalesce` FAIL.

- [ ] **Step 3: Implement** — insert decision fn at the draw site; streaming events no longer draw directly, they mark dirty and let the 50ms tick (or priority flag) flush. Verify spinner still animates (tick draws) and submit is instant (Task 3's redraw uses the priority flag).

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-cli` PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "perf(tui): coalesce render requests and add priority immediate draw"`

### Task 5: Pressure-driven tool row allocation ladder

**Files:**
- Create: `oxicode-vtui/src/presentation/allocation.rs`
- Modify: `oxicode-vtui/src/presentation/mod.rs` (decl), `oxicode-cli/src/tui_vt/main_loop.rs` (live-region height budgeting consumes allocation)

**Interfaces:**
- Produces:
```rust
pub struct BlockAlloc { pub rows: usize }  // 0 = hidden, 1 = glyph row, 2 = folded card, 3+ = full
pub fn allocate_rows(block_heights: &[usize], budget: usize) -> Vec<BlockAlloc>
```
Algorithm: if `sum(heights) <= budget` → everyone full (`rows = height`). Else: each block min 1 row; surplus distributed newest-first up to full height; blocks whose allocated rows < 3 render folded (the RENDERER decides how, allocation just returns counts); if `blocks.len() > budget`, allocate 1 row to the newest `budget` blocks, 0 to older, and the caller shows a single `… N earlier blocks hidden` banner row (budget -1 reserved for it).

- [ ] **Step 1: Failing tests** in `allocation.rs`: `roomy_all_full`, `pressure_folds_oldest_first` (budget squeezes oldest to 1, newest keeps full), `emergency_hides_oldest_and_banners`, `empty_inputs_no_panic`, `single_block_taller_than_budget_folds`.

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-vtui allocation` FAIL.

- [ ] **Step 3: Implement** the pure module; wire into `main_loop.rs` live region: compute per-block natural heights from `visible_items`, call `allocate_rows`, render folded card (2-row: `╭─ tool · activity` / `╰─ …`) and glyph row (1-row: `▸ tool · activity` with shared wall-clock pulse) for under-allocated blocks. User-set `BlockDisplayMode` (Collapsed/Expanded) overrides the ladder for that block (manual wins).

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-vtui -p oxicode-cli` PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(tui): pressure-driven tool row allocation ladder"`

### Task 6: Incremental streaming markdown render (line cache)

**Files:**
- Modify: `oxicode-vtui/src/tui/ui/markdown/mod.rs`
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (streaming buffer render path holds the cache)

**Interfaces:**
- Produces: `pub struct MdRenderCache { prev_text: String, prev_width: usize, lines: Vec<Vec<InlineSegment>> }` + `pub fn render_markdown_cached(text: &str, width: usize, cache: &mut MdRenderCache) -> Vec<Vec<InlineSegment>>`. Fast path: if `text` starts with `cache.prev_text` and width unchanged → re-render only by diffing at LINE granularity (prefix of complete lines `[..k]` reused by clone; k = count of leading lines of prev whose content still matches — recompute from line split, cheap), suffix re-rendered. Width change or non-prefix change → full render. Also syntect line-highlight memoization inside `render_code_block` via a `thread_local` LRU (HashMap<(String, String), Vec<InlineSegment>> keyed by (lang, line), cap 4096 entries, clear on theme change — theme epoch counter compared).

- [ ] **Step 1: Failing tests**: `cached_prefix_reuses_lines` (cache.hit counter exposed via `MdRenderCache::debug_hits()` — test asserts hits increment when appending tokens to a stream), `cached_result_equals_fresh_render` (property: for 5 append steps, cached output == `render_markdown(text, width)` output), `width_change_busts_cache`.

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-vtui cached` FAIL.

- [ ] **Step 3: Implement**; wire `main_loop.rs` streaming-assistant render site to keep a `MdRenderCache` in `RenderState` and call the cached variant. Non-streaming (final) renders may bypass cache.

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-vtui -p oxicode-cli` PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "perf(tui): incremental streaming markdown render with line cache"`

### Task 7: Scrollback flush on exit + rebuild on width change

**Files:**
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (`commit_scrollback`, exit path, resize handling)

**Interfaces:**
- Produces: `fn plan_full_flush(display_len: usize) -> usize` (trivial: return all) only if needed — main logic: at exit (each `LoopOutcome::Exit` return site / just before `Tui` guard drop), call `commit_scrollback` in a loop with a `force_all: bool` parameter (new param; when true, boundary = whole finalized prefix, ignoring viewport fit) until no rows remain. Produces: `fn should_rebuild_scrollback(prev_w: u16, new_w: u16, prev_h: u16, new_h: u16) -> bool` — true only when width changed. On rebuild: write `CSI 3J` (clear scrollback; crossterm has no direct API — use `execute!(stdout, CSI('3'), CSI('J'))`? verify — correct form is `ESC [ 3 J` raw: `queue!(w, crossterm::csi!("3J"))` or write b"\x1b[3J"), reset `committed_entries = 0`, and let `commit_scrollback` re-commit the transcript at the new width over subsequent ticks.

- [ ] **Step 1: Failing tests**: `rebuild_only_on_width_change` (pure fn: (80,80,24,30)→false, (80,100,24,24)→true, (100,80,30,24)→true), `force_flush_boundary_is_everything` (plan function returns full length).

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-cli scrollback` FAIL.

- [ ] **Step 3: Implement** — force-flush on exit; width-change detection where `last_terminal_width` updates (find resize event handling); on rebuild: emit `\x1b[3J` once, clear `committed_entries`, mark state dirty so the next ticks re-commit at new width. Height-only resize: no-op (existing behavior).

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-cli` PASS. Smoke: `cargo build -p oxicode-cli` then run binary in a pty if feasible; otherwise document manual smoke as deferred.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(tui): flush transcript on exit and rebuild scrollback on width change"`

### Task 8: User-remappable keybindings

**Files:**
- Create: `oxicode-cli/src/tui_vt/keymap.rs`
- Modify: `oxicode-cli/src/tui_vt/mod.rs` (decl), `oxicode-cli/src/tui_vt/main_loop.rs` (input dispatch: replace hardcoded key comparisons for the core actions with keymap lookups)
- Test: in-module tests in `keymap.rs`

**Interfaces:**
- Produces:
```rust
pub struct Keymap { bindings: HashMap<KeyAction, KeyCombo> }  // KeyCombo: { ctrl: bool, alt: bool, shift: bool, key: KeySpec }
pub enum KeyAction { Interrupt, Submit, SubmitNow, QueueToggle, ScrollUp, ScrollDown, Clear, Help, ModelPicker, ToggleThinking, VimMode }
pub impl Keymap {
    pub fn load_default() -> Self;                       // hardcoded = today's keys
    pub fn load_user_overlay(defaults: Self, path: &Path) -> anyhow::Result<Self>; // ~/.oxicode/keybindings.yml
    pub fn matches(&self, action: KeyAction, key: &crossterm::event::KeyEvent) -> bool;
    pub fn conflicts(&self) -> Vec<(KeyAction, KeyAction)>; // same combo → two actions
}
```
YML format:
```yaml
# ~/.oxicode/keybindings.yml
submit_now: ctrl+enter
queue_toggle: ctrl+;
interrupt: esc
```
Unknown keys or actions: warn-once log line (tracing), ignore binding.

- [ ] **Step 1: Failing tests**: `defaults_match_current_hardcoded_keys` (each action → expected combo), `overlay_overrides_action`, `overlay_ignores_unknown_action`, `conflict_detection_finds_duplicates`, `matches_handles_ctrl_enter`. Overlay test writes a temp yml.

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-cli keymap` FAIL.

- [ ] **Step 3: Implement**; replace the hardcoded comparisons in the input thread's key matching for exactly the KeyAction list (leave char-level composer input untouched). Load: `Keymap::load_user_overlay(Keymap::load_default(), Path::new("~/.oxicode/keybindings.yml"))` at TUI startup; on load error fall back to defaults with a reply-line warning.

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-cli` PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(tui): user-remappable keybindings via keybindings.yml"`

### Task 9: PTY bash with ANSI preservation

**Files:**
- Modify: `oxicode-agent/src/tools/bash.rs`, `oxicode-cli/src/store/settings.rs` (add `bash_pty: bool` setting, default false)
- Modify: `oxicode-agent/Cargo.toml` (add `portable-pty` dep; check `deny.toml` passes)

**Interfaces:**
- Produces: `fn run_in_pty(cmd: &str, cwd: &Path, timeout: Duration) -> Result<PtyOutcome, ToolError>` in bash.rs; `PtyOutcome { output: String, exit_code: Option<i32> }` — output keeps SGR color sequences but strips cursor-motion/screen-control escapes (filter: pass through `ESC [ ... m`, drop other CSI/OSC).
- Setting `bash_pty` (serde default false) gates PTY vs existing pipe path. Env var `OXICODE_BASH_PTY=1|0` overrides for tests.

- [ ] **Step 1: Failing test** (unix-only): `pty_preserves_color_codes` — run `printf '\033[31mred\033[0m'` through `run_in_pty`, assert output contains `\x1b[31m`. And `pty_strips_cursor_motion` — `printf '\033[2Jtext'` yields no `\x1b[2J` but contains `text`.

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-agent pty` FAIL.

- [ ] **Step 3: Implement** with portable-pty (size 80x24, merge stderr into stdout via single reader if simpler: run `bash -c 'exec 2>&1; <cmd>'`). Wire setting + env override into the tool's execute.

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-agent` PASS + `cargo deny check` PASS (new dep).

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(agent): run bash tool in a PTY preserving ANSI colors"`

### Task 10: Inline image previews (kitty/iTerm protocols)

**Files:**
- Create: `oxicode-cli/src/tui_vt/image_preview.rs` (+ decl in `tui_vt/mod.rs`)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (generate_image result rendering hook)

**Interfaces:**
- Produces:
```rust
pub enum ImageSupport { Kitty, Iterm2, None }
pub fn detect_image_support() -> ImageSupport;               // TERM=xterm-kitty|KITTY_WINDOW_ID → Kitty; TERM_PROGRAM=iTerm.app → Iterm2
pub fn kitty_transmit_png(id: u32, png: &[u8]) -> String;     // APC G... payload base64, transmit-once
pub fn kitty_place(id: u32, rows: u16) -> String;             // APC G a=T p=1... placement rows
pub fn iterm_inline_png(png: &[u8]) -> String;                // OSC 1337 File
pub fn text_fallback(path: &str) -> String;                    // "[image: path]"
```
Live viewport shows the image (transmit once, place many); rows committed to scrollback use `text_fallback` only (omp lesson: image pixels must not be expected to survive history).

- [ ] **Step 1: Failing tests** (pure): `detect_kitty_from_env_matrix` (env override via `OXICODE_FORCE_IMAGE_TERM` for testability), `kitty_transmit_contains_escaped_base64` (commas/backslashes escaped per kitty spec `\\`→`\\\\`, `,`→`\\c`), `iterm_osc1337_wraps_base64`, `fallback_format`.

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-cli image` FAIL.

- [ ] **Step 3: Implement** module + hook: when a generate_image tool result renders in the LIVE viewport and support != None, emit transmit (dedup by file hash id) + placement sized to the tool box height; committed rows always use fallback text. Respect a settings kill-switch `inline_images: bool` default true.

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-cli` PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(tui): inline image previews via kitty/iTerm graphics protocols"`

### Task 11: Interactive git TUI

**Files:**
- Create: `oxicode-cli/src/tui_vt/git_tui/mod.rs` (component + key routing), `state.rs` (git status model via `git status --porcelain -z`, `git diff --` piped), `diff_doc.rs` (unified diff → view model), `render.rs` (ratatui panes: sidebar tree, diff pane, minimap, footer), `keys.rs` (keymap enum)
- Modify: `oxicode-cli/src/tui_vt/main_loop.rs` (overlay variant `GitTui(GitTuiState)` + key routing when open), `oxicode-cli/src/tui_vt/slash/commands.rs` (register `/git` via `register_extra`)

**Interfaces:**
- Produces:
```rust
pub struct DiffDocument { pub files: Vec<DiffFile> }         // built by parse_unified_diff(&str) -> DiffDocument
pub struct DiffFile { pub path: String, pub old_path: Option<String>, pub hunks: Vec<Hunk>, pub binary: bool }
pub struct Hunk { pub old_start: u32, pub new_start: u32, pub lines: Vec<DiffLine> }
pub enum DiffLineKind { Context, Added, Removed }
pub enum WhitespaceMode { Off, IgnoreWhitespace, IgnoreFormatting }
pub fn filter_whitespace(doc: &DiffDocument, mode: WhitespaceMode) -> DiffDocument  // IgnoreFormatting demotes import-only + pure-indent/blank hunks to Context (TS/JS/Rust/Go import regex per language)
pub enum DiffViewMode { Split, Inline, Hunks, Files }
pub struct GitTuiState { pub doc: DiffDocument, pub view: DiffViewMode, pub ws: WhitespaceMode, pub selected_file: usize, pub selected_hunk: usize, pub sidebar_focus: bool, pub staged: HashSet<String> }
```
Keys: `j/k/h/l/g/G` nav, `alt+down/up` hunk nav with file rollover, `]/[` file hop, `1-4` view modes, `v` sidebar toggle, `b` whitespace cycle, `w` wrap toggle, `s/u` stage/unstage file (git add/rm --), `c` commit form (reuses existing commit-tool flow via queue prompt), `r` refresh, `q`/`esc` close.

- [ ] **Step 1: Failing tests** in `diff_doc.rs`: `parse_unified_diff_basic` (fixture with 2 files, renames, binary), `hunk_boundaries_correct`, `ignore_whitespace_drops_ws_only_hunks`, `ignore_formatting_drops_import_and_indent_hunks` (TS import line fixture + Rust indent-only fixture), `view_mode_is_orthogonal_to_filter`, plus `state.rs`: `porcelain_z_parse_handles_rename_and_unmerged`.

- [ ] **Step 2: RED** — `cargo nextest run -p oxicode-cli git_tui` FAIL.

- [ ] **Step 3: Implement** — pure modules first (diff_doc, state parsing, whitespace filter, key enum), then render.rs (ratatui panes inside the existing overlay area; minimap = right-edge 2-col added/removed density bar), then main_loop overlay wiring + `/git` slash command. Staging shells out to git CLI (project convention; reuse commit tool's git invocation helpers if exported, else `std::process::Command`).

- [ ] **Step 4: GREEN** — `cargo nextest run -p oxicode-cli` PASS + clippy native-browser variant PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(tui): interactive git TUI with diff viewer and staging"`

---

## Final verification (controller-run, after Task 11)

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo clippy -p oxicode-cli -- -D warnings`
4. `cargo nextest run --workspace`
5. Smoke: `cargo build -p oxicode-cli && ./target/debug/oxicode --version` (binary boot); TUI interactive smoke via pty if the harness supports it, else document.
