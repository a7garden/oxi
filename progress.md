# Progress

## Status
Completed

## Tasks
- Port pi-mono package-manager.ts to Rust (packages.rs)
- Port pi-mono extension runner.ts to Rust (extensions.rs)

## Latest: Extension Runner Port (extensions.rs)

### Files Changed
- `oxi-cli/src/extensions.rs` — Added ~1000 lines of new code (ExtensionRunner, discovery, wrapping, emit result types, state tracking, tests)

### What was ported from `pi-mono/packages/coding-agent/src/core/extensions/runner.ts`

#### 1. ExtensionRunner struct
- High-level lifecycle manager wrapping ExtensionRegistry
- Extension loading from filesystem with state tracking
- Extension unloading with state cleanup
- Extension reloading (hot-reload via registry)
- Enable/disable with state management
- Ordered extension execution (registration order preserved)

#### 2. Extension State Tracking
- `ExtensionState` enum: Pending, Active, Disabled, Failed, Unloaded
- Per-extension state map with ordering
- State transitions on load/unload/enable/disable
- Serializable state for diagnostics

#### 3. Event Emission with Results
- `emit_tool_call()` → `ToolCallEmitResult` (blocking, errors)
- `emit_tool_result_event()` → `ToolResultEmitResult` (modified output/success)
- `emit_input_event()` → `InputEventResult` (transform/handled/continue)
- `emit_context_event()` → `ContextEmitResult` (modified messages)
- `emit_before_provider_request_event()` → `ProviderRequestEmitResult` (modified payload)
- `emit_session_before_switch_event()` → `SessionBeforeEmitResult` (cancellation)
- `emit_session_before_fork_event()` → `SessionBeforeEmitResult` (cancellation)
- `emit_session_before_compact_event()` → `SessionBeforeEmitResult` (cancellation)
- `emit_session_before_tree_event()` → `SessionBeforeEmitResult` (cancellation)
- `emit_session_shutdown_event()` → bool (whether handlers existed)

#### 4. Error Isolation
- All emit methods catch panics via `catch_unwind`
- Errors recorded to shared error buffer
- Error listener callback pattern (`on_error()` with handle)
- Listeners are panic-safe (wrapped in catch_unwind)

#### 5. Error Listeners
- `on_error(listener)` → `ExtensionErrorHandle`
- `broadcast_error()` to all listeners
- Drop handle to unregister

#### 6. Handler Detection
- `has_handlers(event_type)` — check if any enabled extensions exist
- `has_enabled_extensions()` — any active extensions

#### 7. Extension Discovery
- `discover_extensions_in_dir(dir)` — scan directory for .so/.dylib/.dll
- `discover_extensions(cwd, configured_paths)` — standard locations:
  1. `cwd/.oxi/extensions/`
  2. `~/.oxi/extensions/`
  3. Explicitly configured paths
- Deduplicates resolved paths
- Supports subdirectory `index.so` convention

#### 8. Tool Wrapping
- `wrap_tool(tool)` → `WrappedTool` with extension hooks
- `wrap_tools(tools)` → batch wrapping
- WrappedTool delegates to inner tool while providing hook points

#### 9. Extension Loading from Filesystem
- `load_extension(path, ctx)` — load shared library, register, call on_load
- `load_extensions_from_paths(paths, ctx)` — batch load with error collection
- `unload_extension(name)` — unregister + state cleanup
- `reload_extension(name, ctx)` — hot-reload via registry
- Validates file extension and existence
- Records load errors for diagnostics

#### 10. Emit Result Types
- `ToolCallEmitResult` — blocked, block_reason, errors
- `ToolResultEmitResult` — output, success, errors
- `ContextEmitResult` — modified, messages, errors
- `ProviderRequestEmitResult` — modified, payload, errors
- `SessionBeforeEmitResult` — cancelled, cancelled_by, errors

#### 11. Tests (25 new tests)
- Runner lifecycle: new, default, register, state tracking
- Enable/disable with state transitions
- Unload with state cleanup
- has_handlers detection
- Extension ordering preservation
- Error listener callback
- emit_tool_call, emit_tool_result
- emit_input (continue)
- emit_session_before_switch, emit_session_shutdown
- Load extension (missing file, wrong format)
- Discovery (empty dir, nonexistent dir, finds shared libs, ignores non-libs, subdirectory index)
- ExtensionState display and serialization
- Emit result type defaults
- Debug formatting
- Delegation to registry

### Notes
- The existing `Extension` trait is unchanged
- The existing `ExtensionRegistry` is unchanged (runner wraps it)
- `cargo check -p oxi-cli` shows 0 errors/warnings in extensions.rs
- Pre-existing compilation errors in other files (session.rs, export.rs, compaction_utils.rs) are NOT from this change

---

## Previous: Package Manager Port (packages.rs)

### Files Changed
- `oxi-cli/src/packages.rs` — Major enhancement (~1800 lines). Ported all missing features from pi-mono's package-manager.ts
- `oxi-cli/Cargo.toml` — Added `semver` dependency

### What was ported

#### 1. Package Sources (ParsedSource enum)
- `npm:<package>[@<version>]` — npm registry packages
- `git://`, `git+ssh://`, `git+https://`, `git@host:path` — git repositories  
- `github:org/repo[@ref]` — GitHub shorthand
- URL archives (`.tar.gz`, `.tgz`, `.zip`, `.tar.bz2`)
- Local path (fallback)

#### 2. Package Installation
- `install_from_source(source, scope)` — universal install from any source type
- `install_npm_pack(spec, scope)` — npm pack + tar extraction approach
- `install_git_sync(source, repo, ref, scope)` — git clone with optional ref checkout + npm dep install
- `install_url(url, scope)` — async HTTP download + archive extraction
- `install_local(path)` — local directory copy (existing, enhanced)

#### 3. Package Update
- `update(name)` — smart update based on lockfile source type
- `update_all()` — batch update all installed packages
- `check_for_updates()` — async check for available npm/git updates
- Git update: fetch → reset --hard → clean -fdx

#### 4. Package Uninstall
- `uninstall(name)` — by package name (existing)
- `uninstall_from_source(source, scope)` — by source specifier, handles git/npm cleanup
- Prune empty parent directories after git removal

#### 5. Package Listing
- `list()` — list all installed manifests
- `list_configured()` — list with ConfiguredPackage metadata (source, scope, installed_path)
- `get_installed_path_for_source(source, scope)` — resolve source → installed path

#### 6. Lockfile (oxi-lock.json)
- `Lockfile` struct with version, packages map
- `LockEntry` with source, name, version, integrity hash, scope, source_type, dependencies
- SHA-256 integrity hash for installed directories
- Read/write JSON lockfile to packages dir

#### 7. Dependency Resolution
- `resolve_dependencies()` — detect missing dependencies across installed packages
- PackageManifest now has `dependencies: BTreeMap<String, String>`

#### 8. Package Types
- ResourceKind: Extension, Skill, Prompt, Theme
- SourceScope: User, Project  
- ResourceOrigin: Package, TopLevel
- ProgressEvent/ProgressAction for install/remove/update/clone/pull

#### 9. Version Constraints (semver)
- `NpmPackageInfo::resolve_version(constraint)` — semver range matching
- `version_satisfies(name, requirement)` — check installed version against constraint
- Uses `semver` crate for version parsing and requirement matching

#### 10. Git Operations
- `git_clone(repo, target, ref)` — clone with optional checkout
- `git_update(repo_dir, ref)` — fetch + reset + clean
- `git_has_update(repo_dir)` — check remote for new commits
- `git_command()` / `git_command_silent()` — low-level git helpers

#### 11. NPM Registry
- `NpmPackageInfo::fetch(name)` — async registry API lookup
- `get_latest_npm_version(name)` — convenience function
- Version resolution with semver ranges

#### 12. Validation
- `validate_package(dir)` — validate manifest, check paths, semver, resources

### Notes
- Pre-existing compilation errors exist in other oxi-cli files (session.rs, agent_session.rs, lib.rs, etc.) — NOT caused by this change
- `cargo check -p oxi-cli` shows 0 errors/warnings in packages.rs
- All existing tests preserved; new tests added for source parsing, lockfile, validation, dependencies, version matching, resolve, and progress callbacks
- Uses shell-out to `git` for git operations (no git2 crate dependency needed)
- Uses `reqwest` for HTTP, `serde_json` for manifests/lockfile, `toml` for package manifests
- Progress callback system matches the TypeScript ProgressCallback pattern
