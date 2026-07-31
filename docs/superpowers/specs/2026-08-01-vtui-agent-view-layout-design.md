# oxi-vtui Agent View Layout — Design Spec

- **Date**: 2026-08-01
- **Status**: Implemented (library committed; production wiring integrated)
- **Source of truth (reference)**: `github.com/xai-org/grok-build` — `xai-grok-pager/src/views/agent.rs`
- **Target crate**: `oxi-vtui` (the **live** TUI crate). `oxi-tui` is orphaned —
  not a workspace member, not depended on by `oxi-cli`, never compiled. It is
  out of scope.

> NOTE: AGENTS.md describes an `oxi-tui` tape-model architecture that no longer
> exists in production. The real production TUI is `oxi-vtui` (ratatui alt-screen
> + an `InlineSession` protocol), consumed by `oxi-cli/src/tui_vt/main_loop.rs`.
> This spec targets the real surface.

---

## 1. Problem

The live `oxi-vtui` render path (`render_frame` in `tui_vt/main_loop.rs`) used a
hard-coded 4-row split: `header(2) / transcript(Min) / composer(3) / footer(1)`.
There was no reusable spatial model, no pane-focus concept, no mouse
hit-testing, and no keyboard-hint bar. grok-build ships a clean, pure-data
layout engine for exactly this. Port it.

## 2. What was ported (library: `oxi-vtui/src/design/layout/`)

The old single-file `design/layout.rs` (147 lines: `LayoutMode` only) was split
into a directory preserving `LayoutMode` and adding grok's primitives:

| Module | Types | Ported from |
|---|---|---|
| `agent.rs` | `AgentViewLayout`, `ActivePane`, `PaneAreas`, `LayoutInput`, `SHORT_TERMINAL_ROWS`, `AUTO_COMPACT_MAX_ROWS`, `effective_compact` | `views/agent.rs` |
| `config.rs` | `LayoutConfig`, `ScrollbarConfig` | `appearance/config.rs` |
| `status_bar.rs` | `StatusBar`, `StatusBarBuilder`, `StatusBarStyling` | `views/status_bar.rs` |
| `shortcuts_bar.rs` | `ShortcutsBar`, `HintItem`, `CompactConfig`, `PendingHint`, `ShortcutBarStyling`, `compute_effective_hints` | `views/shortcuts_bar.rs` |
| `welcome.rs` | `WelcomeLayout`, `WelcomePromptFocus` | `views/welcome/mod.rs` |
| `mod.rs` | re-exports + preserved `LayoutMode` | original `layout.rs` |

### Adaptations from grok (not verbatim copies)

- **`LayoutInput` struct** bundles the ~15 `u16` height parameters so the call
  site names each field — prevents the transposition bugs grok's
  `#[allow(clippy::too_many_arguments)]` compute() risks.
- **`take_optional` / `take_section` / `take_pane` helpers** replace grok's
  repeated `i += 1; let r = chunks[i]; i += 1;` index juggling.
- **`ActivePane::cycle(visible)`** + **`PaneAreas::is_visible`** — focus
  switching that respects which panes currently have non-zero height.
- **Decoupled styling via traits** (`StatusBarStyling`, `ShortcutBarStyling`)
  mirroring oxi-vtui's existing `PanelStyleProvider` pattern — widgets never
  reach into a concrete theme type, so they stay theme-agnostic.
- **ratatui `Widget` impls** (not grok's alt-screen-only widgets, not the dead
  `oxi-tui` tape `Component`). oxi-vtui has no tape engine; ratatui is the
  real surface.

### Vertical stack (top → bottom)

```
StatusBar (1) → [startup warnings] → [tasks/catalog/todo panes]
→ Scrollback (Min 5, dominant) → [btw/queue/turn-status/banner/cta/follow-ups]
→ Prompt (fixed) → ShortcutsBar (1)
```

Short terminals (`height ≤ SHORT_TERMINAL_ROWS=16`) suppress CTA/follow-ups and
drop bottom padding so the prompt and scrollback are never starved. Auto-compact
at `≤ 20` rows.

## 3. Production integration (`oxi-cli/src/tui_vt/`)

A new `frame_layout.rs` bridges the library to the live render path:

- Computes `AgentViewLayout::compute(area, cfg, scrollbar, LayoutInput{...})`,
  compact derived from terminal height via `effective_compact`.
- Renders `StatusBar` into `layout.status_bar`: left = header context
  (app/provider/model/git/tools, previously `render_header`); right = footer
  status + line position (previously `render_footer`).
- Renders `ShortcutsBar` into `layout.shortcuts` with hints **verified against
  `spawn_input_thread`'s real dispatch** (not guesses):
  `Enter:send`, `Esc:cancel`, `Ctrl+C:interrupt`, `↑/↓:scroll`, `PgUp/PgDn:page`.
- `render_frame` becomes a thin caller: bg-fill → `frame_layout::render_chrome`
  → `render_transcript(layout.scrollback)` → `render_composer(layout.prompt)`.

The styling-bridge impls (`ThemeStyles` → `ShortcutBarStyling`) live in
`frame_layout.rs`, keeping `main_loop.rs` changes to the `render_frame` body
only.

## 4. Constraints honored

- **Glyphs**: shortcuts-bar separators (`:`, `/`) are punctuation, not themed
  glyphs; status-bar uses raw spans. No hardcoded theme color leaks into the
  library widgets — they take styles via the styling traits.
- **Theme**: styles flow from `active_styles()` (`ThemeStyles`), never a
  hardcoded `Theme::dark()`.
- **No parallel rendering pipeline**: the layout is pure data consumed by the
  existing ratatui `Frame`; it does not fork a renderer.

## 5. Verification

- `cargo check -p oxi-vtui`, `cargo clippy -p oxi-vtui --all-targets -D warnings`,
  `cargo nextest run -p oxi-vtui` (64 tests incl. 8 layout tests).
- `cargo check --workspace` after integration.
- Hints cross-checked field-by-field against `spawn_input_thread` (lines
  785-860): every advertised key maps to a real `InlineEvent`.
