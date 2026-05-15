# Progress

## Status
In Progress

## Tasks

## Files Changed

## Notes

## 2026-05-15: UTF-8 Safety Fixes

- Fixed byte-based string slicing (`&s[..n]`) that would panic on multibyte characters
- Files modified:
  - `oxi-cli/src/main.rs` - truncate() function
  - `oxi-cli/src/context/auto_compaction.rs` - message truncation and token estimation
  - `oxi-ai/src/compaction.rs` - added safe_truncate() helper, updated 3 truncation sites
- All changes use char_indices() pattern for UTF-8 safe truncation

## 2026-05-15: Tokio Runtime Pooling in TUI

- Replaced per-session Tokio runtime creation with a process-lifetime `OnceLock`-guarded runtime
- File: `oxi-cli/src/tui/app.rs` — added `get_agent_runtime()`, replaced `new_multi_thread()...build()` call
- Eliminates expensive runtime teardown/recreation on every session switch
- `cargo check --package oxi-cli` passes clean
