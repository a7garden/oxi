# Dead Code Cleanup #3 — Findings

**Directory:** `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/`

---

## context/ — Dead Modules Check

### `context/branch_summarization.rs`
- **Status:** Does not exist. Already removed.

### `context/compaction_utils.rs`
- **Status:** Does not exist. Already removed.

### `context/auto_compaction.rs`
- **Status:** ACTIVE — In heavy use:
  - `app/agent_session.rs`: `CompactionConfig`, `CompactionReason`
  - `tui/handlers.rs`: `CompactionReason`
  - `tui/app.rs`: `CompactionReason`
  - `rpc_mode/handlers.rs`, `rpc_mode/protocol.rs`, `rpc_mode/state.rs`, `rpc_mode/utils.rs`: auto_compaction_enabled field and commands
  - `lib.rs`, `main.rs`: settings.auto_compaction usage
- **Conclusion:** Keep. No dead code here.

---

## infra/ — Dead Modules Check

**Current contents of infra/:** `error_recovery.rs`, `mod.rs` only.

All other modules listed (bash_executor, child_process, event_bus, fs_watch, tools_manager, version_check, diagnostics, shutdown) **DO NOT EXIST**. They were previously removed or never existed.

- `output_guard.rs` — listed as "KEEP" in task; does not exist.
- `bash_executor.rs` — does not exist.
- `child_process.rs` — does not exist.
- `event_bus.rs` — does not exist.
- `fs_watch.rs` — does not exist.
- `tools_manager.rs` — does not exist.
- `version_check.rs` — does not exist.
- `diagnostics.rs` — does not exist.
- `shutdown.rs` — does not exist.

**Conclusion:** Nothing to remove. infra/mod.rs only contains `error_recovery`.

---

## storage/ — Dead Code Check

### `resource_loader_compat.rs` — Functions/Types Usage

| Symbol | Status | Usage |
|--------|--------|-------|
| `ResourceType` | ACTIVE | Re-exported by resource_loader.rs |
| `Resource` | UNUSED | Only defined, never referenced externally |
| `LoadResult<T>` | ACTIVE | Used by load_* functions (needed for diagnostics) |
| `LoadError` | ACTIVE | Needed by `LoadResult` |
| `ResourceDiagnostic` | ACTIVE | Needed by `LoadResult` |
| `DiagnosticSeverity` | ACTIVE | Used in `AgentSessionRuntimeDiagnostic` (agent_session_runtime.rs) |
| `ResourcePaths` | UNUSED | Only defined, never referenced anywhere |
| `default_resource_dir()` | ACTIVE | Used by ResourceLoaderOptions::default() |
| `skills_dir()` | ACTIVE | Used in load_all_resources_impl |
| `extensions_dir()` | UNUSED | Only defined, never called externally |
| `themes_dir()` | ACTIVE | Used in load_all_resources_impl |
| `prompts_dir()` | ACTIVE | Used in load_all_resources_impl |
| `load_skills_from_dir_impl()` | ACTIVE | Wrapped by resource_loader.rs |
| `load_skill_impl()` | ACTIVE | Wrapped by resource_loader.rs |
| `load_themes_from_dir_impl()` | ACTIVE | Wrapped by resource_loader.rs |
| `load_theme_impl()` | ACTIVE | Wrapped by resource_loader.rs |
| `load_prompts_from_dir_impl()` | ACTIVE | Wrapped by resource_loader.rs |
| `load_prompt_impl()` | ACTIVE | Wrapped by resource_loader.rs |
| `Skill` | ACTIVE | Re-exported via resource_loader.rs |
| `Theme` | ACTIVE | Re-exported via resource_loader.rs |
| `Prompt` | ACTIVE | Re-exported via resource_loader.rs |
| `extract_yaml_field()` | ACTIVE | Used by load_skill_impl |
| `resolve_path_impl()` | ACTIVE | Wrapped by resource_loader.rs (resolve_path) |
| `ResourceWatcher` | UNUSED | Only defined, never referenced anywhere |
| `ResourceChange` | UNUSED | Only used by ResourceWatcher |
| `ChangeKind` | UNUSED | Only used by ResourceWatcher |
| `load_all_resources_impl()` | ACTIVE | Wrapped by resource_loader.rs (load_all_resources) |
| `LoadAllResourcesResult` | ACTIVE | Re-exported by resource_loader.rs |

**Dead code candidates in resource_loader_compat.rs:**
1. `Resource` struct (unused)
2. `ResourcePaths` struct (unused)
3. `extensions_dir()` function (unused)
4. `ResourceWatcher` struct + impl (unused)
5. `ResourceChange` struct (unused, only used by ResourceWatcher)
6. `ChangeKind` enum (unused, only used by ResourceWatcher)

### `export.rs` — Functions Usage

| Symbol | Status | Usage |
|--------|--------|-------|
| `HtmlExportOptions` | ACTIVE | Used in tui/slash.rs, rpc_mode/utils.rs |
| `ExportMeta` | ACTIVE | Used in tui/slash.rs |
| `TreeNode` | ACTIVE | Defined but used in tests and RPC serialization |
| `ansi_to_html()` | ACTIVE | Used internally by render_search_tool and tests |
| `ansi_lines_to_html()` | ACTIVE | Public API, no external use but tests confirm it works |
| `export_html()` | ACTIVE | Used by tests |
| `export_html_with_options()` | ACTIVE | Core implementation, used by export_to_html |
| `export_to_html()` | ACTIVE | Used by tui/slash.rs:279 |
| Internal rendering fns | KEEP | `render_markdown`, `render_markdown_with_options`, `render_inline`, `render_tool_blocks`, tool renderers |

**Conclusion:** export.rs is fully used. No dead code.

---

## AgentSessionRuntime — Usage Check

**Defined in:** `app/agent_session_runtime.rs`

| Type/Symbol | Status | Usage |
|-------------|--------|-------|
| `AgentSessionRuntime` | ACTIVE | Used by `create_agent_session_runtime` factory (own tests) |
| `AgentSessionRuntimeDiagnostic` | ACTIVE | Used in `AgentSessionServices` and `CreateAgentSessionRuntimeResult` |
| `DiagnosticSeverity` | ACTIVE | Used in `AgentSessionRuntimeDiagnostic` |
| `AgentSessionServices` | ACTIVE | Used by `create_agent_session_from_services` and `create_agent_session_services` |
| `CreateAgentSessionServicesOptions` | ACTIVE | Used in tui/app.rs:671 |
| `CreateAgentSessionFromServicesOptions` | ACTIVE | Used in tui/app.rs:676 |
| `CreateAgentSessionRuntimeResult` | ACTIVE | Used in factory functions |
| `CreateRuntimeFactory` | ACTIVE | Type alias used in factory |
| `CreateRuntimeOptions` | ACTIVE | Used in factory |
| `SessionSwitchReason` | ACTIVE | Used in `AgentSessionRuntime` methods (teardown_current, new_session, switch_session, fork, import_from_jsonl, dispose) |
| `SessionImportFileNotFoundError` | ACTIVE | Used in `import_from_jsonl` error handling |
| `ForkPosition` | ACTIVE | Parameter of `fork()` method; `#[allow(dead_code)]` on `_position` |
| `create_agent_session_services()` | ACTIVE | Used in tui/app.rs:671 and in its own default_create_runtime_factory test |
| `create_agent_session_from_services()` | ACTIVE | Used in tui/app.rs:676 and in its own tests |
| `create_agent_session_runtime()` | ACTIVE | Used in its own tests and potentially for RPC mode |
| `default_create_runtime_factory()` | ACTIVE | Used in its own test block |
| `get_default_agent_dir()` | ACTIVE | Used in services creation and internal tests |
| `parse_model_id()` | ACTIVE | Used by create_agent_session_from_services |
| `build_system_prompt()` | ACTIVE | Used by create_agent_session_from_services |

**Conclusion:** The entire `AgentSessionRuntime` module is active. Nothing to remove.

---

## Summary of Actual Dead Code

Only the following items in `resource_loader_compat.rs` are unused:

1. `Resource` struct
2. `ResourcePaths` struct  
3. `extensions_dir()` function
4. `ResourceWatcher` struct + impl block
5. `ResourceChange` struct
6. `ChangeKind` enum

All other items in scope are actively used.

---

## Recommended Actions

1. **storage/resource_loader_compat.rs** — Remove:
   - `Resource` struct
   - `ResourcePaths` struct
   - `extensions_dir()` function
   - `ResourceWatcher` struct and impl
   - `ResourceChange` struct
   - `ChangeKind` enum

2. **Everything else** — No changes needed. All other code is in use.