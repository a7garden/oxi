# oxi-cli Crate — Deep Analysis Report

**Crate:** `oxi-cli` v0.5.0  
**Lines:** 48,574 across 70 files  
**Date:** 2026-05-06  
**Reviewer:** Automated code review

---

## Executive Summary

oxi-cli is the core application crate for a terminal-based AI coding assistant. It implements multi-provider LLM integration, streaming-first communication, session management with tree-structured persistence, a dynamic extension system, and a full TUI. The codebase is ambitious and broadly functional but suffers from significant technical debt: duplicated logic across modules, pervasive `TODO` placeholders (224), missing doc comments (208 "TODO: document"), no permission enforcement for extensions, and several god-object files exceeding 2,000 lines.

---

## 1. Architecture: **B+**

### Module Structure

```
oxi-cli/src/
├── main.rs (1,050 lines) — CLI entry point, subcommand dispatch
├── lib.rs (577 lines) — App struct, InteractiveSession, module declarations
├── cli.rs (458 lines) — Clap argument definitions
├── agent_session.rs (2,164 lines) — Core session wrapper around Agent
├── agent_session_runtime.rs (832 lines) — Service container, runtime factory
├── session.rs (3,016 lines) — JSONL session persistence, tree navigation
├── session_navigation.rs (1,341 lines) — Branching, forking, tree traversal
├── rpc_mode.rs (3,225 lines) — Headless JSONL-over-stdio protocol
├── packages.rs (2,917 lines) — Package management (npm, git, local)
├── export.rs (2,167 lines) — HTML session export
├── resource_loader.rs (1,823 lines) — Skill/extension/theme discovery
├── model_registry.rs (1,692 lines) — Built-in + custom model definitions
├── model_resolver.rs (1,440 lines) — Model name resolution, aliases
├── auth_storage.rs (1,637 lines) — API key / OAuth / session token storage
├── settings.rs (1,478 lines) — Layered configuration (TOML/JSON)
├── extensions/ — Extension system (registry, types, context, loading)
├── tui/ — Terminal UI (app, handlers, render, slash commands)
├── ... (40+ additional utility modules)
```

### Dependency Graph (Logical Layers)

```
main.rs → cli.rs, App (lib.rs)
    ↓
AgentSession ← AgentSessionRuntime ← AgentSessionServices
    ↓
oxi_agent::Agent → oxi_ai::Provider
    ↓
SessionManager (JSONL persistence)
ExtensionRegistry → libloading (dynamic .so/.dylib/.dll)
```

**Strengths:**
- Clean separation between session wrapper (`AgentSession`) and runtime container (`AgentSessionRuntime`)
- Extension system is well-factored into types, context, registry, and loading submodules
- Settings use layered loading (defaults → global → project → env → CLI)
- RPC mode provides both JSONL and JSON-RPC 2.0 protocols

**Weaknesses:**
- **Duplicated `build_system_prompt`** — exists in 3 places: `lib.rs`, `agent_session_runtime.rs`, and `system_prompt.rs` with different signatures and behavior. Only the one in `system_prompt.rs` is the full version.
- **Duplicated `parse_model_id`** — in both `lib.rs` and `agent_session_runtime.rs`
- `App` struct in `lib.rs` and `AgentSession` in `agent_session.rs` serve overlapping purposes. `App` appears to be legacy scaffolding while `AgentSession` is the real implementation.
- `main.rs` contains ~500 lines of dead code (interactive readline mode, `handle_command`, etc.) gated by `#[allow(dead_code)]`
- No clear separation between "library API" and "internal implementation" — `lib.rs` re-exports everything publicly

### Key Architectural Concerns

1. **Three separate session abstractions**: `InteractiveSession` (lib.rs), `AgentSession` (agent_session.rs), and `SessionManager` (session.rs) with overlapping responsibilities
2. **No dependency injection container** — services are manually wired in `create_agent_session_services()`
3. **rpc_mode.rs at 3,225 lines** contains protocol types, framing, JSON-RPC mapping, AND command execution — should be split into 3-4 modules

---

## 2. Quality: **B-**

### Correctness

| Issue | Severity | Location |
|-------|----------|----------|
| `persist_session()` accesses `Agent::state()` which returns a clone, then tries to count "new" messages vs `persisted_count` — race condition potential | Medium | `agent_session.rs:1092-1184` |
| `ListenerGuard::drop()` replaces with no-op instead of removing — listeners vector grows unboundedly | Low | `agent_session.rs:1392-1398` |
| `session_stats()` computes `input_tokens` and `output_tokens` as hardcoded 0 — token stats are never populated | Medium | `agent_session.rs:277-310` |
| `is_compacting()` uses `try_lock()` on tokio Mutex which is not reliable for state checks | Low | `agent_session.rs:250-258` |
| `handle_command` in main.rs uses `truncate()` which panics on non-ASCII boundaries | Low | `main.rs` |
| BashExecutor wraps commands with `{}; __oxi_cwd=$(pwd)` but doesn't parse the captured cwd back out | Medium | `bash_executor.rs:139` |
| Auth credentials stored as plaintext JSON in `~/.config/oxi/auth.json` (doc says "encrypted file storage" but implementation is plaintext) | High | `auth_storage.rs` |

### Error Handling

- **613 `unwrap()` calls** across the crate — many in paths that could fail (file I/O, parsing)
- `SessionManager::create()` silently ignores directory creation failures via `let _ = fs::create_dir_all()`
- Extension loading uses `call_hook_safe` with `catch_unwind` for panic protection — good
- Error types are well-defined (`AuthError`, `ExtensionError`, `FsWatchError`, `OAuthError`) with proper `thiserror` derives
- `RetryConfig` with exponential backoff is well-implemented

### Readability

- **208 "TODO: document"** markers on public items (primarily in rpc_mode.rs and extensions)
- Inconsistent doc style — some modules have excellent module-level docs (agent_session.rs, agent_session_runtime.rs), others have none
- Heavy use of `parking_lot::RwLock` and `Arc` makes data flow hard to follow
- `session.rs` at 3,016 lines with 136 functions is a god module

### Test Quality

| Metric | Value |
|--------|-------|
| Unit tests (inline `#[test]`) | 895 |
| Integration test files | 2 (101 lines total) |
| Mock provider for tests | Yes (in agent_session.rs) |
| Test coverage of critical paths | Medium — agent_session.rs has excellent tests; session.rs, rpc_mode.rs, packages.rs have minimal testing |

**Strengths:**
- `agent_session.rs` has ~50 thorough tests covering model cycling, thinking levels, queues, compaction, events
- `cli.rs` has comprehensive CLI parsing tests
- Extension registry tests include panic safety verification

**Weaknesses:**
- `session.rs` (3,016 lines) has zero inline tests — only a thin integration test file
- `rpc_mode.rs` (3,225 lines) has zero tests
- `packages.rs` (2,917 lines) has zero tests
- No property-based or fuzz testing for JSONL parsing
- `main.rs` dead code (interactive mode) is `#[allow(dead_code)]` with no tests

---

## 3. Performance: **B**

### Session Management

- Sessions are stored as append-only JSONL files — efficient for writes
- `SessionManager` reads the entire file on open and holds entries in memory (`Vec<FileEntry>` under `RwLock`) — could be problematic for very long sessions
- `persist_session()` serializes to JSON on every call; no batching
- `SessionManager` has a `persisted_count` optimization to avoid re-appending already-persisted messages

### File Watching

- `fs_watch.rs` (163 lines) is clean and minimal — uses `notify` crate with polling interval of 1 second
- Proper cleanup via `Drop` implementation
- Error handler callback pattern for graceful degradation

### Extension Loading

- `libloading` used for dynamic shared library loading — standard approach
- **No sandboxing** — extensions run in-process with full access to the host
- `call_hook_safe` wraps every hook invocation in `catch_unwind` — overhead on every event
- Extension loading does file I/O synchronously on the calling thread

### Streaming

- `prompt_streaming()` correctly uses `spawn_blocking` + `LocalSet` to handle `!Send` futures from `parking_lot::RwLock`
- Events forwarded via `mpsc::unbounded_channel` — no backpressure mechanism
- Auto-compaction runs synchronously in the event processing path, blocking further event handling

### Concerns

| Issue | Impact |
|-------|--------|
| `serde_json::to_string(messages)` in `check_auto_compaction()` serializes all messages to estimate tokens | O(n) serialization per response |
| No LRU or size limit on in-memory session entries | Unbounded memory growth |
| `ResourceLoader` scans directories on every access (no caching flag visible) | Filesystem I/O per prompt |

---

## 4. Security: **C+**

### Auth Storage

- **Critical: Credentials stored as plaintext JSON** — `FileAuthStorage` reads/writes `auth.json` with no encryption. The module documentation claims "encrypted file storage" but no encryption is implemented.
- `AuthCredential::ApiKey { key }` stores raw API key strings
- `AuthCredential::OAuth { access_token, refresh_token }` stores raw tokens
- No keyring integration despite `AuthError::KeyringError` variant existing
- Environment variable discovery (`find_env_keys`) is well-implemented and safe
- Token expiry checking is properly implemented

### OAuth

- `OAuthCallbackServer` binds to `localhost` — correct
- Port range 8787-8887 with auto-selection — reasonable
- State parameter for CSRF is captured but **not verified** in `parse_oauth_callback`
- 10-minute timeout on callback — appropriate
- `open_browser()` function is cross-platform and safe

### Extension Sandboxing

- **No permission enforcement** — `ExtensionManifest::has_permission()` exists but is never checked before executing extension actions
- Extensions loaded via `unsafe { Library::new() }` and `unsafe { Box::from_raw() }` — this is inherently unsafe
- Extensions can call any function in the process (no capability isolation)
- `ExtensionContext` provides `read_file()`, `send_message()`, `set_model()` — extensions have full agent control
- Hot-reload via `hot_reload()` re-opens shared libraries without validation

### Input Validation

- RPC mode parses arbitrary JSON from stdin — no input size limits
- `parse_oauth_callback()` does basic URL parsing but minimal validation
- Bash executor passes user commands directly to `/bin/bash -c` — this is intentional (tool) but has no sanitization

### Recommendations

1. **Implement actual encryption** for `auth.json` or use the system keychain
2. **Enforce extension permissions** — check `has_permission()` before allowing file/network/bash access
3. **Verify OAuth state parameter** to prevent CSRF
4. Add input size limits to RPC command parsing

---

## 5. Maintainability: **C+**

### Code Organization

| Problem | Detail |
|---------|--------|
| **God modules** | `session.rs` (3,016), `rpc_mode.rs` (3,225), `packages.rs` (2,917) |
| **Dead code** | ~500 lines in `main.rs` (readline mode), `CommandResult` enum |
| **Duplicated logic** | 3× `build_system_prompt`, 2× `parse_model_id`, overlapping `App`/`AgentSession` |
| **208 "TODO: document"** | Primarily in `rpc_mode.rs` (all `RpcCommand` variants) |
| **13 `#[allow(dead_code)]`** | Gating unused code that should be removed or feature-gated |
| **Inconsistent patterns** | Some modules use builder pattern (ExtensionContext), others use Options structs |

### Specific Maintainability Issues

1. **`session.rs` needs splitting**: Session entry types, JSONL I/O, tree operations, migration logic, and SessionManager should be separate modules
2. **`rpc_mode.rs` needs splitting**: Protocol types, JSON-RPC mapping, command execution, and server loop should be separate modules
3. **`packages.rs` needs splitting**: Source parsing, npm resolution, git operations, and PackageManager should be separate modules
4. **`main.rs` interactive mode** is dead code that should be removed or moved to a feature flag
5. **`App` struct in lib.rs** is partially dead — `run_interactive()`, `InteractiveSession`, `InteractiveLoop` are not used in the TUI path
6. Extension trait has **37 methods** — consider grouping into smaller traits or providing default no-op impls (some defaults exist but are inconsistent)

### Positive Patterns

- Builder pattern for `ExtensionContext` and `RetryConfig`
- RAII guards for listener subscription (`SessionListenerGuard`)
- Panic-safe extension hooks (`call_hook_safe`)
- Proper `thiserror` usage for error types
- Good module-level documentation in core modules (agent_session, agent_session_runtime)

---

## File-by-File Summary

### Tier 1: Critical Path (>1000 lines)

| File | Lines | Purpose | Concerns |
|------|-------|---------|----------|
| `rpc_mode.rs` | 3,225 | Headless JSONL protocol | **No tests.** 458-line `execute_command` function. 100+ "TODO: document". Needs module split. |
| `session.rs` | 3,016 | JSONL session persistence | **No inline tests.** 136 functions. Migration logic, tree ops, I/O all in one file. |
| `packages.rs` | 2,917 | Package management | **No tests.** Complex source parsing (npm, git, URL). Lockfile management. |
| `agent_session.rs` | 2,164 | Session wrapper | Well-tested. Duplicated system prompt logic. Token stats always return 0. |
| `export.rs` | 2,167 | HTML export | ANSI-to-HTML conversion is excellent. Self-contained HTML with JS/CSS. |
| `resource_loader.rs` | 1,823 | Resource discovery | Complex but well-structured. Context file loading. Deduplication. |
| `model_registry.rs` | 1,692 | Model definitions | Static model catalog. Custom model support. |
| `auth_storage.rs` | 1,637 | Auth credential storage | **Plaintext storage despite claiming encryption.** Good env var discovery. |
| `keybindings.rs` | 1,533 | Key binding configuration | Large config file. Reasonable for a TUI app. |
| `theme.rs` | 1,517 | Theme system | Color definitions and styling. |
| `settings.rs` | 1,478 | Layered configuration | Good layer system. Migration support. |
| `model_resolver.rs` | 1,440 | Model name resolution | Alias system, fuzzy matching. |
| `session_navigation.rs` | 1,341 | Tree navigation | Branch/merge logic. |

### Tier 2: Important (400-1000 lines)

| File | Lines | Purpose | Concerns |
|------|-------|---------|----------|
| `main.rs` | 1,050 | CLI entry | ~500 lines dead code (readline mode). Config/Pkg handlers should be in separate modules. |
| `branch_summarization.rs` | 1,000 | Branch summarization | Self-contained. |
| `compaction_utils.rs` | 937 | Compaction helpers | Good utilities. |
| `error_recovery.rs` | 931 | Retry with backoff | Well-implemented. |
| `auto_compaction.rs` | 849 | Auto-compaction engine | Good event system. |
| `agent_session_runtime.rs` | 832 | Service container | Clean factory pattern. Duplicated `build_system_prompt`. |
| `extensions/registry.rs` | 657 | Extension registry | **No permission enforcement.** Unsafe library loading. |
| `tools_manager.rs` | 586 | External tool binaries | fd/rg download from GitHub. |
| `lib.rs` | 577 | App struct, re-exports | Overlapping with AgentSession. |
| `version_check.rs` | 576 | Version checking | GitHub API check. |
| `bash_executor.rs` | 553 | Shell execution | CWD tracking incomplete. Timeout support. |
| `event_bus.rs` | 513 | Event system | Simple pub/sub. |
| `extensions/types.rs` | 506 | Extension type definitions | Well-structured. |
| `messages.rs` | 469 | Message formatting | Utility. |
| `cli.rs` | 458 | CLI argument types | **Excellent test coverage.** |

### Tier 3: Utility (<400 lines)

| File | Lines | Purpose | Verdict |
|------|-------|---------|---------|
| `tui/` (5 files) | 1,476 total | Terminal UI | Clean module split. |
| `tui/app.rs` | 454 | TUI main loop | Standard ratatui app. |
| `tui/slash.rs` | 432 | Slash command UI | Good. |
| `image_resize.rs` | 428 | Image processing | Utility. |
| `diagnostics.rs` | 426 | Diagnostic output | Utility. |
| `git_utils.rs` | 412 | Git operations | Good. |
| `file_processor.rs` | 411 | File processing | Some "TODO" markers. |
| `skills/mod.rs` | 403 | Skill management | Clean. |
| `clipboard_image.rs` | 398 | Clipboard image support | Utility. |
| `frontmatter.rs` | 394 | YAML frontmatter parsing | Good. |
| `print_mode.rs` | 377 | Print mode (non-interactive) | Good. |
| `tmux_detect.rs` | 356 | tmux detection | Utility. |
| `output_guard.rs` | 348 | Output size limiting | Good safety measure. |
| `oauth_server.rs` | 310 | OAuth callback server | CSRF state not verified. |
| `system_prompt.rs` | 301 | System prompt builder | **The canonical version** — others should be removed. |
| `fs_watch.rs` | 163 | Filesystem watching | Clean. Minimal. |
| All others | <200 each | Various utilities | Acceptable. |

---

## Scores Summary

| Category | Score | Rationale |
|----------|-------|-----------|
| **Architecture** | **B+** | Good layering, clean extension system. Marred by duplication and god modules. |
| **Quality** | **B-** | Good error types, extensive tests on core modules. But 613 unwraps, zero tests on 3 largest files, token stats broken. |
| **Performance** | **B** | Streaming architecture is sound. No backpressure, unbounded memory, O(n) token estimation are concerns. |
| **Security** | **C+** | Plaintext credential storage with misleading docs. No extension sandboxing. OAuth CSRF gap. |
| **Maintainability** | **C+** | 224 TODOs, 208 undocumented items, duplicated functions, 3 god modules >2000 lines, dead code in main. |

### Overall: **B-**

---

## Top 10 Priority Fixes

1. **Encrypt auth.json or use OS keychain** — Critical security issue
2. **Enforce extension permissions** — `has_permission()` exists but is never called
3. **Verify OAuth state parameter** — CSRF vulnerability
4. **Remove duplicate `build_system_prompt`** — Keep only `system_prompt.rs` version
5. **Remove dead code in `main.rs`** — ~500 lines of readline mode
6. **Split `session.rs`** into entry_types, jsonl_io, tree_ops, migration, manager
7. **Split `rpc_mode.rs`** into protocol_types, jsonrpc_mapping, command_execution, server
8. **Fix `session_stats()` token counting** — Currently hardcoded to 0
9. **Add tests for `session.rs`, `rpc_mode.rs`, `packages.rs`** — The three largest files have zero or minimal tests
10. **Remove `App`/`InteractiveSession`/`InteractiveLoop` from `lib.rs`** — Superseded by `AgentSession`/`AgentSessionRuntime`
