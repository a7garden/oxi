# oxicode Settings Panel Overhaul — Design Spec (Phase 1 of 3)

- **Status**: Design complete, pre-implementation. Implementation deferred to a
  separate git worktree / session (explicit user decision — token budget).
- **Scope**: `oxicode-cli`'s `/settings` overlay + the keybinding-dispatch layer
  it depends on. Nothing outside `oxicode-cli/src/tui_vt/` and
  `oxicode-cli/src/store/settings.rs` is touched.
- **Not in this doc**: model/provider-picker richness, slash-command surface
  expansion. See §14 Roadmap — recorded as pointers only, not designed.

## 1. Problem Statement

oxicode is a Rust port of `oh-my-pi` (omp, TypeScript, upstream ~18.x). omp's
`/settings` is a schema-driven, tabbed, searchable panel covering ~hundreds of
settings. oxicode's `/settings` (`oxicode-cli/src/tui_vt/slash/registry.rs:168-260`,
`SettingsCommand::execute`) is a **flat, hand-written list of 6 items**
(Model / Thinking / Auto-compaction / Auto-retry / Advisor / Icons), while
`Settings` (`oxicode-cli/src/store/settings.rs:104-305`) actually declares
**~30 fields** (plus `AdvisorSettings`'s 4 sub-fields and `Vec<HookSpec>`).
Everything not in the 6-item list is invisible in the TUI — only reachable by
hand-editing `~/.oxicode/settings.toml` or the `oxicode config` CLI subcommand.
This is the concrete "설정 패널이 너무 약해" gap.

## 2. Goals

1. Every scalar field on `Settings` (bool/enum/number/string) is visible and
   editable from `/settings` without leaving the TUI.
2. Structure (tabs + groups + search) scales to 30+ settings without becoming
   an unscannable wall of text — port omp's proven IA, not its visual chrome.
3. Live-apply semantics stay identical to the current 6-item panel (no "Save"
   button, no restart-required states except where structurally unavoidable —
   see §9).
4. `keybindings: HashMap<String, Vec<String>>` becomes a real, live-editable
   keymap instead of a persisted-but-unread field.

## Non-Goals

- No alternate-screen / fullscreen-overlay mode. `AGENTS.md` is explicit:
  *"Rendering is ratatui on the main screen (no tape engine, no alt screen);
  the single render driver is `tui_vt/main_loop.rs::render_frame`."* This
  design extends the existing in-frame `OverlayState`/`render_overlay`
  machinery — it does not introduce a second rendering mode.
- No mouse support (omp's settings panel has extensive mouse hit-testing;
  oxicode's TUI has no mouse layer today and adding one is out of scope).
- No touching composer text input, vim mode, or per-character key routing.
  Only the **global single-shot shortcuts** (Ctrl+C/M/P/;/E, Ctrl+Enter) become
  keymap-driven — see §9.1 for the exact boundary.
- No animated wizard chrome (splash/transition/outro screens) — that belongs
  to Phase 2 (setup wizard), not this panel.

## 3. Prior Art — omp's Settings System (condensed)

Full research citations in the session transcript; key facts this design
relies on:

- **Schema-as-table**: `SETTINGS_SCHEMA` (`settings-schema.ts`, ~6000 lines) —
  one literal `{type, default, ui:{tab,group,label,description,warning,
  condition,options,secret,ordered}}` per setting path. A pure adapter
  (`settings-defs.ts`) maps schema entries to 6 widget kinds. No rendering
  logic lives in the schema.
- **10 fixed tabs × named groups**; groups render as non-interactive heading
  rows; a group-contiguity invariant is unit-tested
  (`settings-layout.test.ts:19-60`).
- **Live-apply, no save button**: every change calls `settings.set()`
  synchronously, persisted via a 100ms-debounced atomic write
  (`settings.ts:597-641`). Visual-only settings (theme) preview-then-commit;
  everything else applies immediately.
- **Split-pane layout**: ≥2 sections + ≥60 cols → left sidebar (group names)
  + right item list, non-active-section rows dimmed; falls back to flat
  inline headings when narrow or searching (`settings-list.ts:518-740`).
- **Global fuzzy search** across all tabs, ranked, with per-tab result
  headings and tab-bar match counts (`settings-selector.ts:835-987`).
- **Condition-gated rows**: a small named-predicate registry
  (`CONDITIONS`, `settings-defs.ts:75-124`) hides/shows rows live (e.g.
  advisor sub-settings only when `advisor.enabled`).

oxicode already has the *primitive* this whole system builds on:
`InlineListItem{title, subtitle, badge, indent, selection: Option<...>,
search_value}` (`oxicode-vtui-compat/src/ui_protocol/selection.rs:114-121`)
and `InlineListSearchConfig{label, placeholder}`
(`oxicode-vtui-compat/src/ui_protocol/types.rs:34-37`) — a non-interactive
row is already expressible today (`selection: None`, used for the read-only
"Model" row in the current `/settings`). This design is additive to that
primitive, not a replacement.

## 4. Architecture — Declarative `SettingDef` Table

New file: `oxicode-cli/src/tui_vt/settings_defs.rs`.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    ThinkingLevel, AutoCompaction, AutoRetry, GlyphSet, EditFormat,
    ExtensionsEnabled, SessionHistorySize, ToolTimeoutSecs, AskTimeoutSecs,
    DisabledTools, CommitToolEnabled, TodoPanelEnabled, AgentHubEnabled,
    MermaidRenderEnabled, SnapcompactEnabled, MemoryEnabled, TtsrEnabled,
    TtsrInterruptMode, AdvisorEnabled, AdvisorSyncBacklog, AdvisorImmuneTurns,
    ModelRoles, Keybindings,
    // Read-only pointer rows (widget = Pointer, see §5):
    Theme, Model, CustomProviders, Hooks, ExtensionPaths, SkillPaths,
    PromptPaths, ThemePaths,
}

pub struct SettingDef {
    pub key: SettingKey,
    pub tab: SettingsTab,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub widget: SettingWidget,          // §5
    pub condition: Option<fn(&Settings) -> bool>,
}

pub const SETTING_DEFS: &[SettingDef] = &[ /* ~35 entries, hand-authored, see §8 */ ];

// The only two functions that touch `Settings` fields directly — every
// other consumer (rendering, search, condition eval) goes through the
// table + these two match arms.
pub fn get_display_value(key: SettingKey, s: &Settings) -> String { /* match */ }
pub fn apply_change(key: SettingKey, s: &mut Settings, new: SettingValue) -> Result<()> { /* match */ }
```

Adding a setting = one `SETTING_DEFS` entry + one arm each in `get_display_value`
/ `apply_change`. No new rendering code, no new search/grouping code — this
mirrors omp's schema/adapter split (`settings-schema.ts` + `settings-defs.ts`)
at a scale that doesn't need a runtime schema language: a `const` array is
enough for ~35 entries where omp needed one for ~hundreds.

## 5. Widget Catalog

omp has 6 widget kinds; oxicode needs 7 (its `ProviderLimits` special-case
doesn't apply — nothing in `Settings` needs a per-key numeric map with a
"clear all" action — but `Settings.keybindings` and `.model_roles` are both
`HashMap<String, Vec<String>>` / `HashMap<String, String>`, which omp doesn't
have an analog for and which need a dedicated map editor):

| Widget | Behavior | Example fields |
|---|---|---|
| **Toggle** | Enter flips bool, live-applies | `auto_compaction`, `memory_enabled`, `todo_panel_enabled`, `agent_hub_enabled`, `mermaid_render_enabled`, `snapcompact_enabled`, `commit_tool_enabled`, `extensions_enabled`, `ttsr_enabled`, `advisor.enabled` |
| **Cycle** | Enter advances to next enum value in place (no submenu — matches current `thinking_level`/`glyph_set` UX) | `thinking_level`, `glyph_set`, `edit_format` |
| **Submenu-Select** | Enter opens a child `InlineListItem` list (reuses `show_list_modal`); selecting commits | `advisor.sync_backlog` (off/sync/async), `ttsr_interrupt_mode` (finite string set) |
| **Text** | Enter opens a single-line prompt (unmasked — nothing here is a credential); numeric fields parse-validate before commit, reject with inline error | `session_history_size`, `tool_timeout_seconds`, `ask_timeout_secs`, `advisor.immune_turns` |
| **Multiselect** | Space toggles membership; unordered (●/○); commits on close | `disabled_tools` — options sourced live from `ToolRegistry` names, not hardcoded |
| **Map-Editor** | 2-level: key list (Enter=edit value, `n`=new key, `d`=delete) → value submenu/text per entry | `model_roles` (key=role name, value=model pattern text), `keybindings` (key=`GlobalAction` name, value=KeyCapture list, see §9.4) |
| **Pointer** (read-only) | Non-interactive row (`selection: None`), badge shows current value or count, description names the owning command | `theme` → "use /theme", `last_used_model`/`last_used_provider` → "use /model", `custom_providers` → "use /providers", `hooks` → "use /hooks" (new, §10), `extensions`/`skills`/`prompts`/`themes` path lists → "use oxicode config" |

`max_tokens`/`temperature`/`default_temperature`/`max_response_tokens`: the
struct comment marks the first two as superseded by the latter two. The panel
surfaces only `default_temperature` and `max_response_tokens` as **Text**
widgets under Model tab; the deprecated pair stays hidden (no `SettingDef`
entry) — don't resurrect dead config surface into a shiny new UI.

## 6. Layout & Rendering

Extend, don't replace, `OverlayState`/`render_overlay`
(`oxicode-cli/src/tui_vt/main_loop.rs:588-596`, `4507`):

```rust
pub struct OverlayState {
    pub title: String,
    pub lines: Vec<String>,
    pub items: Vec<OverlayListItem>,
    pub selected: usize,
    pub search: Option<OverlaySearchState>,
    pub secure_input: Option<OverlaySecureInput>,
    // New, both default-empty for every non-settings overlay:
    pub tabs: Vec<String>,
    pub active_tab: usize,
    pub sections: Vec<String>,      // group names for the *active tab*
    pub active_section: usize,
}
```

`OverlayListItem` (`main_loop.rs:573-577`) gains no new fields — a heading
row is already representable as an item with no badge/selection and a
distinct render style keyed off `sections` membership computed at render
time (mirrors how `InlineListItem{selection: None}` already renders
non-interactively elsewhere).

`render_overlay` gains one branch, inserted before the existing flat-list
path:

- `tabs.len() > 1` → render a one-line tab bar (◂ Model ▸ style, active tab
  bold+accent) above the content area, ←/→ switches `active_tab` and rebuilds
  `items`/`sections` for that tab.
- `sections.len() >= 2 && area.width >= 60` → split: left column
  `width = min(22, longest_section_name) + 4` lists section names (active
  bold+accent, `active_section` synced to the group of the currently
  selected item), right column is the existing item-list renderer with rows
  outside `active_section` painted with `Modifier::DIM`. Falls back to the
  current flat single-column renderer (with non-interactive heading rows
  inserted between groups) when narrower or while `search` is `Some`.
- Row anatomy unchanged from today (cursor `> `, label + right-aligned
  badge); a changed-from-default value gets `styles.primary` instead of
  `styles.secondary` — no new theme fields needed.

This is deliberately a **subset** of omp's layout system: no mouse, no
preview/cancel machinery beyond what `theme` already does via `/theme`, no
stable-height padding trick (oxicode's overlay already reflows the whole
frame per render, unlike omp's fixed-viewport terminal renderer).

## 7. Interaction Model

- **Live-apply**: identical to today's `ConfigAction` handling in
  `main_loop.rs` (`Settings::load()` → mutate via `settings_defs::apply_change`
  → `save()`) — every commit immediately persists, no batch/save step.
- **Search**: reuse `InlineListSearchConfig` (already wired for `/settings`
  as "Filter settings"); extend the search corpus to *all tabs*, not just the
  active one, when a query is non-empty — matches produce a flat list with
  per-tab heading rows (same `sections`-as-headings mechanism, driven by tab
  name instead of group name for this one view).
- **Condition gating**: `SettingDef.condition` re-evaluated every time the
  panel rebuilds its item list (tab switch, value change, search edit).
  Exactly two gates needed for Phase 1: `advisor.sync_backlog`/
  `advisor.immune_turns` require `advisor.enabled`; `ttsr_interrupt_mode`
  requires `ttsr_enabled`.
- **Esc layering**: submenu/map-editor open → close it, restore list
  selection by `SettingKey` (not index — matches omp's id-based restore
  rationale, indices shift under filtering); else search non-empty → clear
  it; else panel → close (existing `handle.close_overlay()` path).

## 8. Field → Tab/Group Mapping

7 tabs (omp's 10, collapsed — oxicode has ~1/10th the settings surface, more
tabs would be empty shelves):

| Tab | Group | Fields |
|---|---|---|
| **General** | Behavior | `extensions_enabled`, `session_history_size`, `edit_format` |
| **Model** | Defaults | `thinking_level`, `default_temperature`, `max_response_tokens`, `model_roles` (map-editor) |
| | Pointer | `theme` → `/theme`, current model → `/model` |
| **Interaction** | Compaction | `auto_compaction` |
| | Timeouts | `tool_timeout_seconds`, `ask_timeout_secs` |
| **Tools** | — | `disabled_tools` (multiselect), `commit_tool_enabled` |
| | Pointer | `custom_providers` → `/providers`, `hooks` → `/hooks` |
| **UI** | Appearance | `glyph_set` |
| | Panels | `todo_panel_enabled`, `agent_hub_enabled`, `mermaid_render_enabled`, `snapcompact_enabled` |
| **Advisor & Memory** | Advisor | `advisor.enabled`, `advisor.sync_backlog` (cond.), `advisor.immune_turns` (cond.) |
| | Memory/TTSR | `memory_enabled`, `ttsr_enabled`, `ttsr_interrupt_mode` (cond.) |
| **Keybindings** | — | `keybindings` (map-editor, §9.4) |
| **Advanced** | Pointer | `extensions`/`skills`/`prompts`/`themes` path lists → `oxicode config` |

## 9. Keymap-Driven Dispatch Refactor

### 9.1 Scope boundary — read this before touching `main_loop.rs`

`main_loop.rs`'s raw input loop hardcodes exactly **6 global shortcuts** as
`if key.code == KeyCode::Char(x) && key.modifiers.contains(KeyModifiers::CONTROL)`
checks around lines 3279-3345: Ctrl+C (interrupt), Ctrl+M (toggle multiline),
Ctrl+P (command palette), Ctrl+; (queue panel), Ctrl+E (fold/expand), and
Ctrl+Enter (send-now). **Only these six become keymap-driven.** Everything
else in that loop — composer character input (`KeyCode::Char(ch)` general
typing at line 3609+), vim-mode key routing, secure-input character filtering,
confirmation-modal y/n/x, overlay navigation — stays exactly as-is. These are
text-entry and modal-navigation paths, not "shortcuts"; action-ifying them
would break IME/CJK input and add indirection with no user-facing benefit.

### 9.2 New module: `oxicode-cli/src/tui_vt/keymap.rs`

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GlobalAction {
    Interrupt, ToggleMultiline, OpenCommandPalette,
    ToggleQueuePanel, FoldAll, SendNow,
}

// NOTE: `Shift+E` (expand-all, `RenderState::expand_all`) is deliberately
// NOT a `GlobalAction`. It's handled inside the composer's bare-character
// branch (`main_loop.rs:3707`, alongside vim-style `'e'`/`'J'`/`'K'` block
// navigation), not the six `KeyModifiers::CONTROL`-gated checks this table
// replaces (`main_loop.rs:3279-3345`). Per §9.1's scope boundary, that
// context-sensitive cluster stays untouched in Phase 1 — only `Ctrl+E`
// (fold-all) is a genuine global shortcut.
pub const DEFAULT_KEYBINDINGS: &[(GlobalAction, &str)] = &[
    (GlobalAction::Interrupt, "Ctrl+c"),
    (GlobalAction::ToggleMultiline, "Ctrl+m"),
    (GlobalAction::OpenCommandPalette, "Ctrl+p"),
    (GlobalAction::ToggleQueuePanel, "Ctrl+;"),
    (GlobalAction::FoldAll, "Ctrl+e"),
    (GlobalAction::SendNow, "Ctrl+Enter"),
];

pub struct Keymap { bindings: HashMap<GlobalAction, Vec<KeyCombo>> }

impl Keymap {
    /// Defaults + user overrides from `settings.keybindings` (additive —
    /// a user entry adds combos, it does not replace the default unless
    /// the user's list is non-empty for that action, matching omp's
    /// "merge user bindings over defaults" behavior).
    pub fn from_settings(overrides: &HashMap<String, Vec<String>>) -> Self;
    pub fn resolve(&self, key: KeyEvent) -> Option<GlobalAction>;
}
```

`KeyCombo` is a small parseable struct (`"Ctrl+p"` → modifiers + code) shared
by both the resolver and the KeyCapture widget's serialization — one parser,
two directions.

### 9.3 Migration

`RenderState` gains `pub keymap: Arc<ArcSwap<Keymap>>` (or an `RwLock` —
`ArcSwap` avoids lock contention on the input-loop hot path), built once at
TUI startup from `Settings::load().keybindings` and swapped wholesale
whenever the Keybindings tab commits a change (no restart needed — the input
loop reads through the `Arc` on every keystroke).

In `spawn_input_thread`'s event loop, replace the six `if key.code == ... &&
modifiers.contains(...)` blocks with:

```rust
if let Some(action) = keymap.load().resolve(key) {
    match action {
        GlobalAction::Interrupt => { /* existing body, verbatim */ }
        GlobalAction::ToggleMultiline => { /* existing body, verbatim */ }
        // ...
    }
    continue;
}
```

The bodies do not change — only the trigger condition moves from a literal
key comparison to a table lookup. `route_cancel`/`handle_ctrl_c_key`'s
streaming-vs-idle state machine (`main_loop.rs:1450-1453`, `3133-3166`) is
**unchanged**: remapping controls *which key fires `GlobalAction::Interrupt`*,
never what Interrupt does once fired.

### 9.4 Keybindings tab UI

Map-editor over `GlobalAction` (6 fixed keys, not free-form — unlike
`model_roles` there's no "add a new action", only "rebind an existing one").
Selecting an action opens a **KeyCapture** submenu: renders "Press a key
combo (Esc to cancel)…", the next `KeyEvent` received is serialized via the
same `KeyCombo` parser and appended to that action's binding list; a second
capture on the same action lets multiple combos coexist (matches
`DEFAULT_KEYBINDINGS` + overrides being additive, §9.2). `d` on a listed
combo removes it (guard: refuse to remove the last binding for an action —
an action with zero keys is a silent trap, not a feature).

## 10. New Command: `/hooks`

Needed because `Settings.hooks: Vec<HookSpec>` has no editor anywhere today
(not in `/settings`, not in a dedicated command) and the Advanced-tab Pointer
row for it needs somewhere real to point. Minimal scope for Phase 1:
`/hooks` lists configured hooks (event, command, project-approval status)
read-only, mirroring `/mcp`'s dashboard pattern
(`oxicode-cli/src/tui_vt/slash/commands.rs:557-626`) — not a full add/edit/
remove flow (that's `oxicode config`'s job, same division of labor as
`custom_providers` already has between `/providers` and hand-edited TOML).

## 11. Data Model Changes Summary

| File | Change |
|---|---|
| `oxicode-cli/src/tui_vt/settings_defs.rs` | **New.** `SettingKey`, `SettingDef`, `SETTING_DEFS`, `get_display_value`, `apply_change` |
| `oxicode-cli/src/tui_vt/keymap.rs` | **New.** `GlobalAction`, `Keymap`, `KeyCombo`, `DEFAULT_KEYBINDINGS` |
| `oxicode-cli/src/tui_vt/main_loop.rs` | `OverlayState` +4 fields; `render_overlay` +tab-bar/sidebar branch; `RenderState` +`keymap: Arc<ArcSwap<Keymap>>`; input loop's 6 hardcoded shortcut checks → `keymap.resolve()` dispatch; `SettingsCommand::execute` rewritten to build from `SETTING_DEFS` instead of the literal 6-item `vec![]` |
| `oxicode-vtui-compat/src/ui_protocol/selection.rs` | `InlineListSelection` +`SettingsTab(usize)`, +`SettingsSection(usize)`, +`SettingKeyCapture(SettingKey)` variants for overlay-submission routing |
| `oxicode-cli/src/tui_vt/slash/commands.rs` | **New** `HooksCommand` (§10) |
| `oxicode-cli/src/store/settings.rs` | No field changes — this design only adds *access*, not new persisted state |

## 12. Testing & Verification Plan

- **Unit** (`settings_defs.rs`): every `SETTING_DEFS` entry's group is
  contiguous within its tab (port of `settings-layout.test.ts:19-60`'s
  invariant); `get_display_value`/`apply_change` round-trip for every
  `SettingKey`; condition gates flip correctly when their dependency changes.
- **Unit** (`keymap.rs`): default-only resolve; user-override adds rather
  than replaces; two actions never silently share a combo (constructor
  rejects/warns on collision); `KeyCombo` parse/serialize round-trip for
  `"Ctrl+p"`, `"Ctrl+Shift+e"`, `"Ctrl+Enter"`.
- **Integration** (`main_loop.rs` `#[cfg(test)]`, existing pattern e.g.
  `handle_overlay_key`/`handle_confirmation_key` tests at ~7064/7332): tab
  switch rebuilds `items`/`sections`; section-sidebar selection sync;
  multiselect toggle persists to `disabled_tools`; map-editor add/delete for
  `model_roles`; KeyCapture commit rewires `Keymap` live (send the new combo
  immediately after rebind, assert the *old* combo no longer resolves).
- **Smoke**: launch the real TUI via `hub` (PTY), open `/settings`, tab
  through all 7 tabs, toggle one item per widget kind, rebind Ctrl+P to
  another combo and confirm both the command palette opens on the new combo
  and no longer opens on Ctrl+P, confirm `~/.oxicode/settings.toml` reflects
  every change without restarting.

## 13. Risks & Mitigations

- **Keymap refactor touches the hottest path in the app** (every keystroke).
  Mitigation: change is a pure trigger-condition swap (§9.3), bodies
  untouched; the existing `route_cancel`/Ctrl+C state machine tests keep
  covering behavior, new tests only cover trigger resolution.
- **`ArcSwap` dependency** — check it's not already a workspace dep; if
  adding it is undesirable, `parking_lot::RwLock<Arc<Keymap>>` is an
  acceptable fallback (per-keystroke read-lock cost is negligible vs. the
  I/O already happening in that loop) — implementer's call, not a blocking
  decision for this spec.
- **`SettingKey` match-arm duplication risk** (get/apply must stay in sync
  with `SETTING_DEFS`) — mitigated by the unit test that round-trips every
  `SettingKey` variant; a missing arm is a compile error (non-exhaustive
  match), a missing `SETTING_DEFS` entry for a `Settings` field is not
  statically caught — acceptable for Phase 1 (30 fields, code-reviewable),
  revisit with a derive-macro or build-script check only if the field count
  grows materially.

## 14. Roadmap (recorded, not designed)

- **Phase 2 — Model/Provider Setup TUI**: port omp's `ModelBrowser` pattern
  (fuzzy search + MRU/role/version-aware sort + context/cost/perf columns +
  role chips + over-context graying) as the shared core behind oxicode's
  existing `/model`, `/models`, and a future compact quick-switch picker.
  Setup-wizard scene chrome (splash/transition/outro) is explicitly
  deprioritized — cosmetic, not functional gap.
- **Phase 3 — Slash Command Surface**: omp ships 74 builtins vs oxicode's 21.
  ~55 categories are missing (session mgmt: `/new /fresh /drop /rename /pin
  /branch /fork /tree /move /add-dir`; context mgmt: `/shake`, `/compact`
  modes; sub-agent work: `/btw /tan /cleanse`; info: `/todo /jobs /usage
  /stats /context`; etc.). Many of these require new backend capabilities
  (goal mode, collab/share, voice) beyond TUI polish — triage into
  "TUI-only wins" vs "new subsystem" is the first task of that phase's
  brainstorming session, not assumed here.

## 15. Explicit Non-Goals / Deferred Decisions for Phase 1

- No mouse support in the settings panel (see §2).
- No animated preview-then-commit for anything except what `/theme` already
  does independently.
- `ArcSwap` vs `RwLock<Arc<>>` for `Keymap` storage — implementer's choice
  (§13).
- Exact `KeyCombo` string grammar (`"Ctrl+Shift+e"` vs `"ctrl-shift-e"`) —
  implementer's choice, must round-trip through serde for `settings.toml`
  persistence and stay human-editable.
