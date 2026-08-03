# Task 5 Brief — Overlay, Completion, Input, and Rich-Media Integration

Plan: `docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`
Prerequisites: Tasks 1-4 complete and reviewed.

## Files
- Modify terminal_host, tape_render, app, render, tape transcript/engine as required.

## Required behavior
1. Representative overlays (settings, model, ask, issue) open in alternate screen, receive existing events, close once, and return to intact main-screen transcript.
2. Multi-line input, slash/file completion, queue, todo, notifications, footer, kill/yank/yank-pop, undo, bracketed paste, SGR mouse, and Kitty-enhanced key events remain behaviorally intact.
3. Completion popup and sticky content are tape rows during normal mode; modal overlays remain ratatui.
4. Images are protocol-safe. Ordinary line terminators MUST NOT be appended inside Kitty/iTerm2 payloads. Introduce typed raw rows or equivalent explicit metadata. Unsupported terminals retain current textual fallback.
5. Overlay return must not ED3-clear or duplicate finalized transcript rows.

## TDD
- Overlay transition tests first, then sticky interaction tests, then raw image row tests; observe RED for missing behavior.
- GREEN: focused module tests plus `cargo nextest run -p oxicode-cli --test pty_e2e` tests relevant to overlays if already available.
- Lint/format: oxicode-cli and oxicode-tui clippy all-targets, fmt check.

## Report
Write task-5-report.md in this plan workspace with status, RED/GREEN evidence, image row design, overlay transition proof, commits, self-review, concerns. Commit changes.
