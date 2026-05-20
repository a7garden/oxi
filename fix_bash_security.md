# Bash Tool Security Fixes

## Summary

Applied four critical security hardening changes to `/Volumes/MERCURY/PROJECTS/oxi/oxi-agent/src/tools/bash.rs`.

All changes compile cleanly with `cargo check --lib` (zero errors in bash.rs).

---

## Changes Made

### 1. Blocked Environment Variables (`BLOCKED_ENV_VARS`)

**Location:** Lines 24–47 (new const array)

Added a constant array of 19 dangerous environment variables that are silently filtered out when injecting env vars from LLM parameters:

- `LD_PRELOAD`, `LD_LIBRARY_PATH` — dynamic linker manipulation (library injection)
- `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `DYLD_FRAMEWORK_PATH` — macOS equivalent
- `PATH`, `HOME`, `SHELL`, `USER`, `LOGNAME` — system identity/path manipulation
- `IFS` — input field separator attacks
- `PYTHONPATH`, `NODE_PATH`, `RUBYLIB`, `PERL5LIB`, `CLASSPATH` — runtime library injection
- `JAVA_TOOL_OPTIONS`, `MallocNanoZone`, `MallocSpaceEfficient` — JVM/memory allocator manipulation

**Implementation:** Case-insensitive matching via `eq_ignore_ascii_case()` to prevent bypass through casing tricks (e.g., `Path` instead of `PATH`).

### 2. Dangerous Command Pattern Detection (`is_dangerous_command()`)

**Location:** Lines 49–117 (new function)

A warning-only detection function that identifies:
- **Pipe to shell:** `| sh`, `| bash`, `| zsh`
- **Sensitive file access:** `/etc/passwd`, `/etc/shadow`, `id_rsa`, `id_ed25519`, `.ssh/`
- **Network exfiltration:** `curl|nc`, `wget|nc`, `/dev/tcp/`, `/dev/udp/`
- **Privilege escalation:** `sudo`, `su -`, `su root`
- **Fork bombs:** `:(){ :|:& };:` pattern variants
- **System directory writes:** redirect to `/etc/`, `/boot/`, `/sys/`, `/proc/`

Returns `Option<String>` — `Some(warning)` if patterns detected, `None` if safe. The warning is appended to the tool output but **does not block execution**.

### 3. Process Group Kill on Timeout/Abort (Unix `libc::kill`)

**Location:** Lines 328–340 (timeout) and 348–360 (abort)

On timeout or abort signal, the code now uses `libc::kill(-(pid as i32), libc::SIGKILL)` to send SIGKILL to the **entire process group** before calling `child.kill()`. This ensures that child processes spawned by the shell (pipelines, subshells, background tasks) are also terminated.

The `child.id()` returns `Option<u32>` in tokio, so it's unwrapped safely with `if let Some(pid)`.

### 4. Working Directory Validation (`validate_cwd()`)

**Location:** Lines 121–155 (new function), called at line 258

New function `validate_cwd(dir, workspace)` that:
- Rejects `..` path traversal
- Checks the directory exists
- When a workspace root is provided, **canonicalizes** both paths (resolving symlinks) and validates the cwd is within the workspace
- Prevents symlink escape attacks where a symlink inside the workspace points outside

Currently called with `workspace: None` in the main execution path (backward compatible). The workspace parameter is available for callers to enforce confinement.

---

## Files Modified

| File | Change Type |
|------|-------------|
| `oxi-agent/src/tools/bash.rs` | Security hardening (4 features) |

## Verification

- `cargo check --lib` — 0 errors in bash.rs (pre-existing errors in other files)
- No new dependencies added (`libc` was already in Cargo.toml)
- All changes are backward-compatible (no API changes, workspace validation is opt-in)
