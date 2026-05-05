# Progress

## Status
✅ Complete — All critical bugs fixed, major improvements applied.

## Completed Tasks
- **#24: oxi-cli dead_code audit** — Audited all 20 `#[allow(dead_code)]` suppressions across oxi-cli source files. Added documentation comments to 5 items that lacked rationale. All suppressions are intentionally kept (see `/tmp/fix24-cli-cleanup.md`).
- **#19: oxi-ai dead_code audit** — Reduced `#[allow(dead_code)]` count from 81 to 64 (21% reduction). Removed 3 unused functions, 2 unused structs, 1 dead field. Consolidated per-field annotations to per-struct. Documented all remaining suppressions with rationale. See `/tmp/fix19-deadcode.md`.

## Files Changed
- `oxi-cli/src/export.rs` — Documented `ToolOp` enum, `render_markdown()`, `render_markdown_with_options()` suppressions
- `oxi-cli/src/fs_watch.rs` — Documented `watcher` field suppression
- `oxi-cli/src/auto_compaction.rs` — Documented `llm` field suppression
- `oxi-cli/src/settings.rs` — Documented `ENV_PREFIX` const suppression
- `oxi-ai/src/providers/mod.rs` — Removed unused `provider_names()` and `providers()` functions
- `oxi-ai/src/tools.rs` — Removed unused `create_schema()` function
- `oxi-ai/src/transform.rs` — Removed dead `api` field from `IntermediateMessage::Assistant`
- `oxi-ai/src/compaction.rs` — Renamed `provider` → `_provider` in `LlmCompactor`
- `oxi-ai/src/providers/openai_responses.rs` — Removed unused `CompletedResponse`/`IncompleteResponse` structs
- `oxi-ai/src/providers/*.rs` — Consolidated serde struct annotations, documented public API annotations

## Notes
- Build passes with 0 errors, 0 warnings in oxi-ai. 424 tests pass.
- All 20 dead_code suppressions in oxi-cli are legitimate: readline fallback code, theme fields for future use, library lifetime management, future API surface.
- All 64 remaining dead_code suppressions in oxi-ai are legitimate: serde deserialization structs (~40) and public API methods for external consumers (~24).
