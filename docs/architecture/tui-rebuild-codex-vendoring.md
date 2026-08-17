# TUI rebuild: Codex presentation architecture

## Decision

Replace the current `tui_vt` renderer incrementally with an oxicode-owned
presentation layer based on the public Rust TUI architecture in OpenAI Codex.
This is **source vendoring of small presentation primitives**, not adoption of
the Codex application, protocol, configuration, or agent runtime.

The selected source revision is `openai/codex` commit
`9ded177ce7c1c0bd2047f902936c177612ab3434` (retrieved 2026-08-16), licensed
Apache-2.0. Every derivative file must retain a source header and an entry in
the repository `NOTICE.md`.

## Why this replaces the current approach

The existing `oxicode-vtui` import contains theme, layout, markdown, and
protocol types, but not VT Code's live app/session/widget tree:

- `oxicode-vtui/src/tui/core_tui/mod.rs` exports only types.
- `spawn_core_session()` is explicitly `unimplemented!`.
- the production host recreates terminal setup, input dispatch, transcript,
  composer, overlays, and rendering in
  `oxicode-cli/src/tui_vt/main_loop.rs`.

So treating `oxicode-vtui` as a drop-in TUI is misleading. Completing that
port would import VT Code's application semantics into oxicode and duplicate
the agent boundary. Codex has the right separation for this product: the chat
surface owns presentation state; it receives protocol events and emits user
intent, but it does not run the agent.

## Target ownership

```text
AgentSession / slash commands / approvals
                 │ typed UI events
                 ▼
        tui_vt::controller            owns event routing and side effects
                 │ immutable view model
                 ▼
  oxicode-vtui::presentation          owns cells, bottom pane, overlays, draw
                 │ ratatui Buffer
                 ▼
              terminal
```

The controller must not render. Presentation must not use `App`,
`AgentSession`, stores, or `tokio` channels. The only bridge is a small,
owned view model and intent enum in `oxicode-cli`.

## Vendoring map

| Source in Codex | oxicode destination | Scope |
|---|---|---|
| `render/renderable.rs` | `oxicode-vtui/src/presentation/renderable.rs` | measured cells and vertical composition |
| `history_cell/base.rs` | `presentation/transcript.rs` | plain/composite/wrapped cells only |
| `history_cell/messages.rs` | `presentation/transcript.rs` | user/assistant streaming cells, after adapting to `InlineSegment` |
| `bottom_pane/pending_input_preview.rs` | `presentation/queue.rs` | queued-input display only |
| `bottom_pane/footer.rs` | `presentation/footer.rs` | pure responsive footer rules only |
| `bottom_pane/*`, `chatwidget.rs`, `app/*` | not vendored | coupled to Codex protocol/runtime/config |

The first source slice is now present as
`oxicode-vtui::presentation::{Renderable, Column, TextCell}`. It intentionally
has no dependency on `oxicode-vtui-compat`, proving the new boundary is not
another VT Code protocol surface. The first production migration also landed:
`oxicode_vtui::presentation::visible_items` now owns block
grouping/collapse/truncation and `main_loop::render_transcript` consumes it.
The old draw routine no longer decides which logical transcript lines are
visible.

## Migration sequence

1. Land and test measured cells (complete).
2. Move transcript block grouping/collapse/truncation into the standalone
   presentation model (complete in `presentation/transcript.rs`). Move the
   active streaming tail, wrapping, and scroll measurement into that module; keep existing
   AgentSession events and `InlineSegment` conversion intact.
3. Replace the remaining transcript renderer in `main_loop.rs` with the cell
   viewport. Preserve transcript/search/fold behavior with buffer tests.
4. Move composer/footer/queued-input rendering into `presentation/bottom_pane`.
   Keep `oxicode-textarea` as the editor and preserve the app-owned vim adapter.
5. Move modal/list/file-search drawing into presentation overlays. The
   controller continues to own secure-input persistence and slash actions.
6. Delete dead VT Code session/host/core façade APIs (`HostAdapter`,
   `spawn_core_session`, `InlineSession`) once no production code imports
   them. Retain theme and markdown only if they are still used.
7. Rename `tui_vt` to `tui` only after the old renderer is gone; do not run two
   live TUI implementations indefinitely.

## Non-negotiable checks

- Every vendored file records source revision and Apache-2.0 attribution.
- Every measured cell has `Buffer`/`TestBackend` tests for width, wrapping,
  clipping, and a streaming-tail update.
- Existing PTY tests remain the interactive acceptance gate: normal startup,
  raw-mode cleanup, resize, prompt submit, interrupt, and secure input.
- A migration slice may change presentation but may not change agent,
  tool, session, approval, or persistence semantics.
- No `parking_lot` guard may cross `.await` in the controller.
