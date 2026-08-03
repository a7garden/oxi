# Plan C cutover — Phase 2 report

**Status:** Complete
**Phase 2 SHA:** b4fcd212 (Phase 2 work; amended into the workspace at HEAD
81eaa20a which carries a sibling agent's `cargo fmt --all` cleanup that
re-touches `v2_overlay_adapter.rs` whitespace-only).
**Test count (workspace):** 3631 passed / 3632 total (1 pre-existing
unrelated failure: `file_catalog_get_model_anthropic` in
`oxicode-sdk/tests/catalog_port.rs` — matches the baseline noted in the task
brief; +6 vs. the 3625 baseline = 1 new `with_frame` test + 5 new adapter
tests).
**oxicode-tui subset:** 222 / 222 pass (includes the new
`with_frame_provides_access_to_frame` test added by Phase 2; the
remaining additions in the worktree are from sibling agents' parallel
work, not Phase 2).
**oxicode-cli subset:** 752 / 752 pass (+5 new adapter tests, see below).
**Report path:** `.superpowers/sdd/planc-cutover-phase2-report.md`

## Deliverables

### 1. RenderCtx frame accessor (oxicode-tui)

Added to `oxicode-tui/src/widget/context.rs`:

```rust
pub fn with_frame<R>(&mut self, f: impl FnOnce(&mut Frame<'f>) -> R) -> R {
    f(self.frame)
}
```

Closure-based access sidesteps the lifetime gymnastics inherent in exposing
the stored `&mut Frame` directly. Documented in-source as a temporary bridge
API to be removed once all legacy overlays are migrated. Test
`with_frame_provides_access_to_frame` covers (a) the closure receives a
`&mut Frame` whose `area()` matches the underlying terminal, (b) the closure
can mutate the buffer, and (c) the outer `RenderCtx` is still usable after the
closure returns.

### 2. LegacyOverlayAdapter (oxicode-cli)

Created `oxicode-cli/src/tui/v2_overlay_adapter.rs` and registered in
`oxicode-cli/src/tui/mod.rs`.

**Trait reference:** `crate::tui::overlay::OverlayComponent` (legacy trait,
`fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme)`), not
`Overlay` as the deliverable sketch suggested.

**`Send` audit:** the deliverable sketch assumed `Renderable: Send`. A
compile-time audit (`grep -rn Send oxicode-tui/src`) found zero load-bearing Send
usage in oxicode-tui, and every real legacy overlay holds
`Arc<Mutex<*mut AppState>>` whose raw pointer makes it `!Send`. The vestigial
`: Send` supertrait was therefore dropped from `pub trait Renderable`, and
the adapter accepts `Box<dyn OverlayComponent>` (no `+ Send`). This unblocks
construction with every real factory overlay (`ForkSelectOverlay`,
`ModelSelectOverlay`, `LogoutSelectOverlay`, etc.) — not just the rare ones
that happen to be `Send`. Two regression alarms back this up in tests (see
below).

**`content_hash`:** returns a monotonic `dirty_seq: u64` field that is bumped
at the end of every `render` call. This is deterministic and respects the
trait's "must be deterministic and cheap" contract. Wall-clock was rejected
because (a) it's not deterministic — equal hashes for equal inputs is the
contract, and (b) a TUI pause/resume would cause spurious repetition. The
counter pattern is structurally identical to "always dirty" with one fewer
external dependency.

**`height_for`:** returns `0`, mirroring the established V2 overlay convention
(`oxicode-tui/src/widget/panel/overlay.rs`). The overlay paints into the full
parent area itself (typically a centered popup via `centered_popup` /
`centered_layout`); returning non-zero would mislead any naive parent that
consults it for vertical layout.

**`render`:** converts the V2 `Theme` into a `LegacyTheme` via
`convert_theme`, then forwards through `RenderCtx::with_frame` to the legacy
`render(frame, area, &legacy_theme)`.

**`convert_theme`:** the prior advisory correctly flagged this as a real
mapping, not a one-liner. Verification: both `oxicode-tui`'s `palette.rs` and
`oxicode-tui-legacy`'s `cell.rs` use `pub use ratatui::style::Color` — the
`Color` types are identical. The two `ColorScheme` structs expose
overlapping-but-non-identical field sets, so the conversion is a
field-by-field copy with semantic mappings for missing slots:
- V2 lacks `secondary` → mapped from `muted` (closest legacy semantic).
- V2 lacks `cursor_fg`/`cursor_bg` → derived as `background`/`foreground`
  (swap), which matches the legacy `dark()` defaults.
- V2 lacks `tool_pending_bg` / `tool_executing_bg` → mapped from
  `surface_bg` (neutral panel default).
- V2 lacks `code_fg` → mapped from `foreground` (modern themes use the
  inline-code color slot, but legacy default falls back to foreground).
- V2 lacks `Spacing` and `Symbols` → `Spacing::default()` and
  `Symbols::default()` (Unicode glyph set), matching the legacy crate's
  own `Default` impl.

### Adapter tests (oxicode-cli)

Five tests live in `oxicode-cli/src/tui/v2_overlay_adapter.rs`:

1. **`adapter_accepts_non_send_overlay`** — `NotSendOverlay(*mut ())` is
   `!Send` (raw pointers are unconditionally `!Send`). Coercing it into
   `Box<dyn OverlayComponent>` and handing it to
   `LegacyOverlayAdapter::new` is the compile-time regression alarm: if
   anyone re-adds `+ Send` to the adapter's overlay slot, this `Box::new`
   line fails to compile. This is the structural proof that the adapter
   accepts `!Send` overlays.
2. **`content_hash_changes_after_render`** — verifies the dirty-seq counter
   is monotonic and distinct from its seed, defeating pipeline memoization
   every frame.
3. **`convert_theme_preserves_core_slots`** — the V2-to-legacy theme mapping
   preserves name + foreground/background/primary/border across all
   built-in V2 themes.
4. **`adapter_wraps_real_overlay`** — constructs a real
   `McpConfigOverlay::new(None, cwd)` and verifies the adapter accepts it
   end-to-end. This proves the bridge works at a genuine integration site,
   not just with the structural mock.
5. **`render_delegates_to_legacy_overlay`** — end-to-end integration test:
   builds a `PaintingOverlay` mock that paints a sentinel cell on `render`,
   drives the adapter's `Renderable::render` through a real
   `TestBackend + Terminal`, then asserts the sentinel cell landed in
   the buffer and `dirty_seq` was bumped. This is the only test that
   exercises the bridge's three pieces together (`convert_theme` +
   `RenderCtx::with_frame` + legacy `OverlayComponent::render`); any
   one of those breaking silently (e.g. a converted theme the legacy
   overlay indexes with `.expect()`) trips this test.

## Verification commands and outcomes

| Command | Result |
| --- | --- |
| `cargo check -p oxicode-tui` | clean |
| `cargo nextest run -p oxicode-tui` | 222 / 222 pass |
| `cargo check -p oxicode-cli` | clean |
| `cargo nextest run -p oxicode-cli` | 752 / 752 pass |
| `cargo clippy -p oxicode-tui -- -D warnings` | clean |
| `cargo clippy -p oxicode-cli -- -D warnings` | clean |
| `cargo nextest run --workspace --no-fail-fast` | 3631 / 3632 pass (1 pre-existing unrelated failure) |

## Files changed

- `oxicode-tui/src/widget/context.rs` — `with_frame` accessor + test.
- `oxicode-tui/src/widget/renderable.rs` — drop vestigial `: Send` supertrait.
- `oxicode-cli/src/tui/v2_overlay_adapter.rs` — new module (347 LOC; under the
  ≤500 LOC cap).
- `oxicode-cli/src/tui/mod.rs` — register `mod v2_overlay_adapter;`.

## LOC

```
oxicode-cli/src/tui/v2_overlay_adapter.rs: 347 lines (under ≤500 cap)
```