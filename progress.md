# Progress

## Status
In Progress

---

### Port pi-mono's sdk.ts to Rust — DONE

Created `oxi-cli/src/sdk.rs` — full SDK module ported from `pi-mono/packages/coding-agent/src/core/sdk.ts` (413 lines).

#### What was ported:

1. **`SdkConfig`** — configuration struct with all options from `CreateAgentSessionOptions`:
   - `cwd`, `agent_dir` — path configuration
   - `auth_storage`, `model_registry` — dependency injection
   - `model`, `thinking_level` — model selection
   - `scoped_models` — model cycling support (Ctrl+P)
   - `no_tools`, `tools` — tool enable/disable modes
   - `session_manager`, `settings` — session and settings injection

2. **`NoTools` enum** — tool suppression modes (`All`, `Builtin`)

3. **`ScopedModel`** — model with optional thinking level override for cycling

4. **`Sdk` struct** — main SDK entry point wrapping Agent:
   - `new(config)` — async factory with full initialization pipeline
   - `run_prompt()` / `run_prompt_streaming()` / `run_prompt_channel()` — prompt execution
   - `register_tool()` / `tool_registry()` — tool registration
   - `enable_tools()` / `disable_tools()` / `active_tools()` — tool management
   - `switch_model()` / `model_id()` — model management
   - `session_id()` / `session_manager()` — session access
   - `reset()` / `cwd()` / `settings()` / `agent()` / `agent_state()` — general accessors
   - `set_system_prompt()` — prompt customization

5. **Model resolution** — `resolve_model()` matching pi-mono logic:
   - Explicit config → existing session → settings default → first available
   - Fallback warnings when model can't be restored

6. **Provider creation** — `create_provider_for_model()` factory

7. **Tool configuration** — `resolve_tool_config()` matching pi-mono logic:
   - Explicit allowlist → no-tools modes → default builtin tools

8. **Factory functions** — convenience constructors:
   - `create_sdk()` — minimal defaults
   - `create_sdk_with_model()` — specific model
   - `create_sdk_with_cwd()` — specific working directory
   - `create_readonly_sdk()` — read-only mode

9. **Re-exports** — SessionEvent, tool types, settings

#### Helper functions ported:
- `default_agent_dir()` → `~/.oxi`
- `build_sdk_system_prompt()` — thinking-level-based prompts
- `find_default_model()` — settings/registry fallback chain
- `get_attribution_headers()` — OpenRouter/Cloudflare headers (kept for future use)

## Files Changed
- `oxi-cli/src/sdk.rs` — NEW: ~580 lines, full SDK module
- `oxi-cli/src/lib.rs` — added `pub mod sdk;`

#### Also fixed (pre-existing issues):
- `oxi-ai/src/high_level.rs` — added missing `text_signature: None` to TextContent
- `oxi-ai/src/transform.rs` — added `text_signature: None` to TextContent instances + `transform_messages_for_model()` function

## Notes
- `cargo check -p oxi-cli` passes (0 errors)
- SDK uses `ResourceLoader` (not DefaultResourceLoader — that TS concept doesn't exist in Rust)
- `AuthStorage::new()` uses default file backend (same as TS `AuthStorage.create()`)
- `ModelRegistry::create()` matches TS `ModelRegistry.create()`
- `SessionManager::create()` matches TS `SessionManager.create()`
- Session ID is parsed from SessionManager's string ID (TS uses string everywhere)

---

### Port pi-mono's session-manager.ts to Rust — DONE

Replaced/enhanced `oxi-cli/src/session.rs` with full implementation ported from pi-mono's `packages/coding-agent/src/core/session-manager.ts` (1425 lines).

#### What was ported:

1. **SessionEntry types** — All entry types from pi-mono:
   - SessionMessageEntry (user, assistant, toolResult messages)
   - ThinkingLevelChangeEntry
   - ModelChangeEntry
   - CompactionEntry (with summary, firstKeptEntryId, tokensBefore, details)
   - BranchSummaryEntry (with fromId, summary, details, fromHook)
   - CustomEntry (for extensions)
   - CustomMessageEntry (for extensions injecting into LLM context)
   - LabelEntry (bookmarks/markers)
   - SessionInfoEntry (metadata like display name)

2. **SessionHeader** with version migration (v1→v2, v2→v3)
   - CURRENT_SESSION_VERSION = 3
   - `migrate_v1_to_v2()`: adds id/parentId tree structure
   - `migrate_v2_to_v3()`: renames hookMessage role to custom
   - `migrate_to_current_version()`: runs all migrations

3. **JSONL format** read/write (one JSON object per line)
   - `load_entries_from_file()` reads JSONL
   - `_rewrite_file()` writes JSONL
   - `_persist()` does append-only writes
   - `parse_session_entries()` for parsing content strings
   - `is_valid_session_file()` for validation
   - `find_most_recent_session()` for resuming

4. **SessionManager** with full API:
   - **Constructors**: `create()`, `open()`, `continue_recent()`, `in_memory()`, `new()` (async compat)
   - **Append methods**: `append_message()`, `append_thinking_level_change()`, `append_model_change()`, `append_compaction()`, `append_custom_entry()`, `append_session_info()`, `append_custom_message_entry()`
   - **Tree traversal**: `get_branch()`, `get_children()`, `get_parent()`, `get_path_to_root()`, `get_ancestry()`, `get_depth()`, `get_tree()`
   - **Branching**: `branch()`, `reset_leaf()`, `branch_with_summary()`, `createBranchedSession()`
   - **Labels**: `add_label()`, `remove_label()`, `get_label()`
   - **Compaction**: `get_latest_compaction_entry()`, `get_compaction_entries()`
   - **Stats**: `get_session_stats()` (token counts, message counts)
   - **Session management**: `list()`, `list_all()`, `delete_session()`, `rename_session()`, `fork_from()`
   - **Context building**: `build_session_context()`
   - **Info**: `get_session_name()`, `get_header()`, `get_entries()`, `get_leaf_id()`, `get_leaf_entry()`, `get_entry()`

5. **AgentMessage types** matching pi-mono:
   - User, Assistant, ToolResult, System
   - BashExecution (with command, output, exitCode, truncated, etc.)
   - Custom (extension-injected with customType, display, details)
   - BranchSummary, CompactionSummary

6. **Content types**: ContentValue (String or Blocks), ContentBlock (Text/Image), AssistantContentBlock (Text/Thinking/ToolCall/ToolPlan/ImageResult/Refusal)

7. **Backward compatibility**: SessionMeta, BranchInfo, and async method wrappers for main.rs compatibility

#### Tests included:
- `test_session_creation` — basic creation
- `test_append_message` — message appending
- `test_tree_traversal` — get_branch, get_children, get_parent
- `test_branching` — branch creation and tree structure
- `test_session_context` — context building
- `test_compaction_entry` — compaction support
- `test_labels` — label add/remove

## Files Changed
- `oxi-cli/src/session.rs` — COMPLETE REWRITE: ~2100 lines, full session manager port
- `oxi-cli/src/export.rs` — updated render_entry for new AgentMessage types
- `oxi-cli/src/branch_summarization.rs` — updated for String-based IDs and new message types
- `oxi-cli/src/compaction_utils.rs` — updated for new AgentMessage types
- `oxi-cli/src/lib.rs` — updated SessionEntry usage for new API
- `oxi-cli/src/agent_session.rs` — updated save_session for new API
- `oxi-cli/src/main.rs` — updated SessionManager usage

## Notes
- `cargo check -p oxi-cli` passes (0 errors, some warnings)
- Entry IDs changed from Uuid to String for compatibility with pi-mono's 8-char hex IDs
- JSONL format matches pi-mono's format exactly
- Internal FileEntry/SessionEntryEnum types handle JSONL serialization/deserialization
- SessionEntry simple struct provides backward-compatible API for existing code
- SessionMeta kept for backward compatibility with main.rs session listing
- Pre-existing issues in extensions.rs and agent_session.rs are unrelated to this port
