# Task 2 Brief — ANSI Style Serialization and Width-Aware Rich Components

Plan: `docs/superpowers/plans/2026-07-29-tui-tape-production-cutover.md`
Prerequisite: Task 1 is complete and reviewed before dispatch.

## Files
- Create `oxi-tui/src/tape/style.rs`
- Create `oxi-tui/src/tape/markdown.rs`
- Create `oxi-tui/src/tape/transcript.rs`
- Modify `oxi-tui/src/tape/mod.rs`
- Modify only as required: `oxi-tui/src/widgets/chat/markdown.rs`, `highlight.rs`, `widgets/tool_renderer.rs`, `render/mermaid.rs`, `tape/components/tool_call.rs`

## Required interfaces
- `styled_line_to_ansi(line: &ratatui::text::Line<'_>, caps: &TerminalCapabilities) -> String` or an equivalent API that also accepts the color level explicitly if current capability APIs require it.
- `TranscriptRenderer::sync(&mut self, messages: &[ChatMessage], streaming: Option<&StreamingState>, theme: &Theme, caps: &TerminalCapabilities)`.
- `TranscriptRenderer::compose(&mut self, width: u16) -> (&RenderResult, LiveRegion)` or the exact borrowed equivalent established by Task 1.

## Required behavior
1. Serialize ratatui Line/Span style into minimal ANSI transitions, using existing `AnsiTracker` and capability color adaptation. Close style and OSC 8 hyperlink state at row boundaries.
2. Reuse existing markdown/table/code/LaTeX/Mermaid/tool formatting. Do not fork parsers or copy formatting logic. Promote visibility or add thin adapters.
3. Map all `ContentBlock` variants: Text, Thinking, ToolCall, ToolResult, Error, Image, Dashboard.
4. Preserve MessageRole styling and width-aware wrapping, including CJK and tables.
5. Finalized messages are memoized children. Active `StreamingState` has a live boundary at the first unstable rendered row.
6. Theme and glyph values come only from `ThemeStyles`/`Symbols`; no hardcoded SGR colors or UI glyphs.
7. Images retain enough protocol metadata for Task 5; unsupported terminals render the current fallback. Do not yet own stdout or emit protocol bytes directly.

## TDD
- ANSI serializer tests first: fg/bg/modifiers, adjacent equal spans, resets, color downgrade, Unicode, row closure. Observe RED.
- Component matrix tests first: every ContentBlock variant, role, glyph preset, narrow widths/CJK/table/code/LaTeX/Mermaid/tool/thinking/error/dashboard/image. Observe RED.
- GREEN: `cargo nextest run -p oxi-tui tape widgets::chat::markdown widgets::tool_renderer render::mermaid`.
- Lint/format: `cargo clippy -p oxi-tui --all-targets -- -D warnings`; `cargo fmt --all -- --check`.

## Report
Write `.superpowers/sdd/2026-07-29-tui-tape-production-cutover/task-2-report.md` with status, RED evidence, implementation/reuse decisions, GREEN evidence, commits, self-review, concerns. Commit all changes.
