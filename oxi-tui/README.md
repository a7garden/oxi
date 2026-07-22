# oxi-tui v2

Terminal-first rendering pipeline and widget library for oxi. Decomposes ratatui's `Terminal::draw()` to own the frame lifecycle (cursor blink preservation, proactive skip via content_hash memoization). See `docs/superpowers/specs/2026-07-21-tui-render-pipeline-redesign.md` for design.

MIT licensed. Clean-room — no upstream code copied.
