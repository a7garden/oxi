# Progress

## Status
Completed

## Tasks
- [x] Port tools-manager.ts to Rust
- [x] Port auto-retry logic from pi-mono agent-session to oxi-agent
- [x] Port session tree navigation (navigateTree) to Rust

## Files Changed
- `oxi/oxi-cli/src/tools_manager.rs` — New file: Rust port of tools-manager.ts (575 lines)
  - `ToolName` enum (Fd, Rg) with `key()` and `config()` methods
  - `ToolConfig` struct with platform-specific asset name resolution
  - `get_tool_path(tool: ToolName) -> Option<PathBuf>` — checks local dir first, then system PATH
  - `ensure_tool(tool: ToolName) -> Result<PathBuf>` — downloads from GitHub releases if not found
  - `command_exists(cmd: &str) -> bool` — checks if command is available on system PATH
  - `download_tool()` — downloads and extracts .tar.gz / .zip archives from GitHub
  - `extract_tar_gz()` / `extract_zip()` — archive extraction via flate2+tar and zip crates
  - Platform support: macOS (aarch64, x86_64), Linux (aarch64, x86_64), Windows (x86_64)
  - Offline mode via `OXI_OFFLINE` env var
  - 12 unit tests (all passing)
- `oxi/oxi-cli/Cargo.toml` — Added `flate2 = "1"`, `tar = "0.4"`, `zip = "2"` dependencies
- `oxi/oxi-cli/src/lib.rs` — Added `pub mod tools_manager;`
- `oxi/oxi-agent/src/events.rs` — Added `AutoRetryStart` and `AutoRetryEnd` event variants
  - `AutoRetryStart { attempt, max_attempts, delay_ms, error_message }`
  - `AutoRetryEnd { success, attempt, final_error }`
  - Updated `type_name()` to return `"auto_retry_start"` / `"auto_retry_end"`
- `oxi/oxi-agent/src/agent_loop.rs` — Enhanced auto-retry logic (port of pi-mono agent-session retry)
  - Added `AgentLoopConfig` fields: `auto_retry_enabled`, `auto_retry_max_attempts`, `auto_retry_base_delay_ms`
  - Added `AgentLoop` fields: `auto_retry_attempt` (AtomicUsize), `auto_retry_cancel` (RwLock<bool>)
  - `is_retryable_error(message)` — regex-based detection of overloaded/rate-limit/server/network/timeout errors
  - `handle_retryable_error()` — exponential backoff with abort support, emits AutoRetryStart/End events
  - `cancel_auto_retry()` — public method to cancel in-progress retry
  - `auto_retry_attempt()` — read current attempt counter
  - `run_loop()` now checks assistant messages for retryable errors before returning, retries with backoff
  - On successful response after retry, emits `AutoRetryEnd { success: true }` and resets counter
  - All existing tests continue to pass
- `oxi/oxi-cli/src/session_navigation.rs` — New file: Rust port of navigateTree() and helpers (~950 lines)
  - `SessionEntryType` enum (Message, BranchSummary, Compaction, Label, SessionInfo, Custom, CustomMessage)
  - `NavigationOptions` struct for navigate options (summarize, custom_instructions, label)
  - `NavigationResult` struct (editor_text, cancelled, aborted, summary_entry_id)
  - `TreePreparation` struct for extension hooks
  - `Summarizer` trait for LLM-based summarization (callback-based)
  - `SessionNavigator` struct with:
    - `navigate_tree()` — main navigation method (port of navigateTree)
    - `collect_entries_for_branch_summary()` — find entries from old leaf to common ancestor
    - `determine_leaf_and_editor()` — entry type detection and leaf switching logic
    - `branch()`, `reset_leaf()`, `branch_with_summary()` — tree traversal helpers
    - `append_label_change()` — label attachment
    - `get_branch()`, `get_children()`, `get_entry()` — tree traversal
  - Utility functions: `extract_user_message_text()`, `is_user_message()`, `is_custom_message()`, etc.
  - 6 unit tests covering navigation, summarization, labels
- `oxi/oxi-cli/src/lib.rs` — Added `pub mod session_navigation;`

## Notes
- `cargo check -p oxi-cli --lib` compiles cleanly for the lib target
- Pre-existing broken modules in the repo are unrelated to these changes
- The `Summarizer` trait uses async/await allowing real LLM integration or mock implementations
- Extension hooks use `Fn(TreePreparation) -> BeforeTreeHookResult` callback pattern
