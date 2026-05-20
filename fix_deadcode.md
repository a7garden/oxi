# Dead Code Analysis - oxi workspace

Generated: 2026-05-15

## Compilation Status
- ✅ `cargo check --workspace --lib` - Compiles successfully
- ❌ `cargo check --workspace` - Binary has compilation errors (pre-existing, unrelated to this analysis)

## Summary
553 warnings in oxi-cli lib, plus additional in oxi-tui and oxi-store. Many of these are missing docs, but many are genuine dead code.

---

## 1. UNUSED MODULES (Entire file is dead code)

### oxi-cli/src/ui/keybindings.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely
- **Files depending on it**: None

### oxi-cli/src/ui/footer_data.rs
- **Status**: Entire file unused (0 external refs)
- **Note**: Contains `git_utils::get_current_branch` usage but that function is called via `crate::util::git_utils`
- **Action**: Can be removed entirely, but `git_utils::get_current_branch` is used elsewhere

### oxi-cli/src/ui/timings.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/ui/theme.rs
- **Status**: Entire file unused (0 external refs)
- **Note**: TUI uses `oxi_tui::theme::Theme` instead
- **Action**: Can be removed entirely

### oxi-cli/src/util/messages.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/util/tmux_detect.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/util/telemetry.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/util/defaults.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/util/slash_commands.rs
- **Status**: Partially used (only `BUILTIN_SLASH_COMMANDS` used)
- **Action**: Keep only what's used, remove rest

### oxi-cli/src/util/sleep.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/util/paths.rs
- **Status**: Entire file unused (0 external refs)
- **Action**: Can be removed entirely

### oxi-cli/src/util/http_client.rs
- **Status**: Partially used (2 functions used: `shared_http_client`, `get_http_client`)
- **Action**: Keep only what's used, remove rest

---

## 2. UNUSED INFRA MODULE (All submodules dead)

The `infra/` module is completely unused from outside except for `error_recovery`:
- `bash_executor.rs` - 0 external refs
- `child_process.rs` - 0 external refs  
- `event_bus.rs` - 0 external refs
- `fs_watch.rs` - 0 external refs
- `output_guard.rs` - 0 external refs
- `tools_manager.rs` - 0 external refs
- `version_check.rs` - 0 external refs
- `diagnostics.rs` - 0 external refs
- `shutdown.rs` - 0 external refs

Only `error_recovery.rs` is used (re-exported in lib.rs).

**Action**: Keep only `error_recovery.rs`, remove all other infra modules.

---

## 3. UNUSED OAUTH MODULE

### oxi-cli/src/oauth_server.rs
- **Status**: Module is referenced in lib.rs but never used from main binary
- **Structs never constructed**: `OAuthCallbackData`, `OAuthCallbackServer`
- **Functions never used**: `parse_oauth_callback`, `authorize_with_browser`, `open_browser`, `run_server`, `find_available_port`
- **Constants never used**: `DEFAULT_PORT_RANGE_START`, `DEFAULT_PORT_RANGE_END`
- **Action**: Can be removed entirely if OAuth flow is not implemented

---

## 4. SPECIFIC ITEMS TO CLEAN

### oxi-tui/src/widgets/chat.rs
- `LayoutKind::Label` variant never constructed
- Unused import: `unicode_width::UnicodeWidthStr`
- Unused variable: `y` assigned but never read
- Unused variable: `inner_x`

### oxi-tui/src/widgets/tool_renderer.rs
- Unused variable: `preview_lines`

### oxi-cli/src/tui/app.rs
- Unused fields: `turn_number` (in TurnStart, TurnEnd)
- Unused field: `masked_cursor` in EnterApiKey

### oxi-cli/src/context/auto_compaction.rs
- Multiple associated items never used
- Structs never constructed: `CompactedContext`, `CompactionSelection`, `CompactionNotification`, `AutoCompactor`

### oxi-cli/src/context/branch_summarization.rs
- Struct `BranchSummaryResult` never constructed
- Associated functions never used

### oxi-cli/src/storage/resource_loader_compat.rs
- Functions never used: `prompts_dir`, `resolve_path_impl`
- Structs never constructed: `ResourceWatcher`, `ResourceChange`
- Enum `ChangeKind` never used
- Struct `LoadAllResourcesResult` never constructed

### oxi-cli/src/storage/export.rs
- Functions never used: `ansi_lines_to_html`, `export_html`

---

## 5. ITEMS TO KEEP (Public API or behind feature flags)

### Keep with `#[allow(dead_code)]`
These are part of public API or may be needed for completeness:
- Error enum variants (don't remove)
- Test helper code
- Conditional compilation behind `#[cfg(feature = "...")]`

### Keep without changes
- `oxi_tui` types (used by TUI)
- `oxi_store` types (used by session management)
- `oxi_ai` core types
- `oxi_agent` core types

---

## 6. COMPACTION-RELATED DEAD CODE

Large amount of dead code in compaction subsystem:
- `context/auto_compaction.rs` - entire module unused
- `context/branch_summarization.rs` - entire module unused
- `context/compaction_utils.rs` - entire module unused

These appear to be alternate compaction implementations that were never integrated.

---

## Recommended Actions

1. **High Priority** (Safe to remove):
   - Remove entire `ui/keybindings.rs`
   - Remove entire `ui/footer_data.rs`
   - Remove entire `ui/timings.rs`
   - Remove entire `ui/theme.rs`
   - Remove entire `util/messages.rs`
   - Remove entire `util/tmux_detect.rs`
   - Remove entire `util/telemetry.rs`
   - Remove entire `util/defaults.rs`
   - Remove entire `util/sleep.rs`
   - Remove entire `util/paths.rs`
   - Remove unused infra modules (keep only error_recovery)
   - Remove entire `oauth_server.rs` if OAuth not implemented

2. **Medium Priority** (Keep partial):
   - Clean `util/slash_commands.rs` - keep only `BUILTIN_SLASH_COMMANDS`
   - Clean `util/http_client.rs` - keep only used functions
   - Clean `util/git_utils.rs` - keep only `get_current_branch`
   - Clean `storage/resource_loader_compat.rs`
   - Clean `storage/export.rs`

3. **Low Priority** (Manual cleanup):
   - Fix unused imports and variables in oxi-tui
   - Clean unused fields in structs
   - Document or remove compaction-related modules

---

## Notes

- The binary (main.rs) has pre-existing compilation errors unrelated to dead code analysis
- All analysis performed on `--lib` only (which compiles)
- Some code may be dead due to incomplete feature implementation
- Code behind feature flags should be left as-is