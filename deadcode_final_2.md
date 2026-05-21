# oxi-cli Dead Code & Warning Fix Report

## Summary

Started with **78 compiler warnings** in `cargo check -p oxi-cli --lib`.  
Ended with **0 warnings**.

## Context

The parallel dead-code-removal agent had already removed some dead types and fields but left the build in a broken state (compile errors). The other agent's changes removed fields from structs/enums that were used by the binary target but not by the lib target alone.

### Key Discovery

`cargo check -p oxi-cli` (full crate, lib+bin) had **0 warnings** from the start. All 78 warnings were `dead_code` warnings only visible with `--lib` — meaning the items were used by the binary target but not by the lib target itself. These should NOT have been removed; they should have been suppressed with `#[allow(dead_code)]`.

## Changes Made

### 1. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/extensions/wasm.rs`
- Added `#[allow(dead_code)]` to `permissions` field of `WasmExtensionManager` struct
- The `with_extension_context` function already had `#[allow(dead_code)]` (from prior agent)

### 2. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/storage/resource_loader.rs`
- Added `#[allow(dead_code)]` to `ExtensionSource` struct (suppresses warnings on `path`, `metadata`, `source_info` fields)
- The following already had `#[allow(dead_code)]` (from prior agent):
  - `SkillSource.metadata`
  - `ThemeSource.metadata`
  - `PromptSource.metadata`
  - `SourceType` enum
  - `Source` struct
  - `extensions_dir` function
  - `load_all_resources` function

### 3. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/storage/resource_loader_compat.rs`
- The `ResourceWatcher` methods and `load_all_resources_impl` already had `#[allow(dead_code)]` from prior agent

### 4. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/storage/export.rs`
- `ansi_lines_to_html` and `export_html` already had `#[allow(dead_code)]` from prior agent

### 5. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/tui/app.rs`
- `stream_text_delta` already had `#[allow(dead_code)]` from prior agent

### 6. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/tui/slash.rs`
- `mask_key` already had `#[allow(dead_code)]` from prior agent

### 7. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/media/clipboard_write.rs`
- `is_clipboard_supported` already had `#[allow(dead_code)]` from prior agent

### 8. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/app/agent_session.rs`
- Prior agent had already:
  - Removed unused `uuid::Uuid` import
  - Added `#[allow(dead_code)]` to `SessionEvent` enum, `CompactionResult`, `ScopedModel`, `PromptOptions`, `StreamingBehavior`, `InputSource`, `SessionStats`, and many methods
  - Removed `CycleDirection`, `default_model_list`, `ModelCycleResult`, `TokenStats`, `cycle_model` and related methods

### 9. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/app/agent_session_runtime.rs`
- Prior agent had already added `#[allow(dead_code)]` to `ForkPosition`, `create_agent_session_runtime`, `default_create_runtime_factory`, and other items

### 10. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/context/auto_compaction.rs`
- Prior agent had already added `#[allow(dead_code)]` to `CompactionReason` variants and `CompactionConfig.threshold`

### 11. `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/util/mod.rs`
- Prior agent had removed `pi_user_agent` and `source_info` module declarations (truly unused by lib)

## Warnings Suppressed by My Edits Only

| File | Item | Fix |
|------|------|-----|
| `extensions/wasm.rs` | `permissions` field | Added `#[allow(dead_code)]` |
| `storage/resource_loader.rs` | `ExtensionSource` struct | Added `#[allow(dead_code)]` |

All other fixes were already applied by the prior agent.

## Verification

```bash
$ cargo check -p oxi-cli --lib 2>&1 | grep '^warning:' | wc -l
0

$ cargo check -p oxi-cli 2>&1 | grep '^warning:' | wc -l
0
```
