# Brief 06: Extension Safety — Unsafe Block Audit

**Area:** `unsafe` blocks across the workspace — FFI loading, process signals, TUI pointer manipulation
**Severity:** 🟡 High
**Estimated scope:** 15 `unsafe` blocks across 5 files, 3 categories of risk

---

## Context

oxi has 15 `unsafe` blocks in production code (excluding the keyword appearing in syntax highlighting strings). These fall into three distinct categories with different risk profiles:

### Category 1: Extension FFI Loading (oxi-cli/src/extensions/loading.rs) — 4 blocks

These load native shared libraries (`.dylib`/`.so`/`.dll`) via `libloading`:

```rust
let library = unsafe { Library::new(path) }          // Load .dylib/.so
let create: Symbol<CreateFn> = unsafe { library.get(ENTRY_SYMBOL) }  // Get symbol
let raw_ptr = unsafe { create() }                     // Call FFI entry point
let extension: Arc<dyn Extension> = unsafe {          // Take ownership of raw pointer
    let boxed: Box<dyn Extension> = Box::from_raw(raw_ptr);
    Arc::from(boxed)
};
```

**Risk:** High if a malicious or buggy extension is loaded. The `Box::from_raw` call assumes the pointer was allocated by `Box::new` on the other side. Memory safety depends entirely on the extension's correctness.

**Mitigating factors:**
- Extensions are user-installed, not network-loaded
- The `null` check on `raw_ptr` is present
- `tracing::info!` logs the load event

### Category 2: Process Signal Handling (3 blocks)

- `oxi-agent/src/tools/bash.rs`: `libc::kill(pgid, SIGKILL)` — kill process group
- `oxi-agent/src/tools/subagent.rs`: `libc::kill(pid, SIGTERM)` — terminate child process
- `oxi-agent/src/mcp/client.rs`: `libc::kill(id, SIGTERM)` — terminate MCP server

**Risk:** Low. These are standard Unix signal calls with well-defined semantics. The `unsafe` is required because `libc::kill` is an FFI call. The PIDs are obtained from `child.id()` which returns valid process IDs.

### Category 3: TUI State Mutation via Raw Pointer (6 blocks)

- `oxi-cli/src/tui/overlay/settings.rs`: 2 blocks — dereference `app_state` lock to call `add_notification`
- `oxi-cli/src/tui/overlay/factories.rs`: 4 blocks — same pattern for notifications

**Risk:** Medium. The pattern is:
```rust
if let Ok(ptr) = self.app_state.lock() {
    unsafe {
        if let Some(ref mut app) = (*ptr).as_mut() {
            app.add_notification(...)
        }
    }
}
```

This dereferences a `MutexGuard` to call `as_mut()` on an `Option<Arc<Mutex<...>>>` or similar. The `unsafe` is used to call an `unsafe fn as_mut()` on a `MutexGuard`. **This pattern is potentially unsound** if the `MutexGuard` lifetime is not correctly managed, though in practice the guard is held for the duration of the block.

---

## Objective

Audit all 15 `unsafe` blocks for soundness, add `// SAFETY:` comments to every one, and fix the TUI overlay pattern if it's unsound.

This does NOT mean:
- ❌ Removing all `unsafe` (some is necessary for FFI and signals)
- ❌ Replacing `libloading` with a different approach
- ❌ Removing process kill/cleanup functionality
- ❌ Adding `unsafe` auditing lints (`#![deny(unsafe_code)]`) globally

It DOES mean:
- ✅ Every `unsafe` block has a `// SAFETY:` comment explaining why it's sound
- ✅ The TUI overlay pattern is audited and confirmed safe (or fixed)
- ✅ Extension loading has clear safety documentation

---

## Approach

### Phase 1: Audit (read-only)

1. Read each `unsafe` block in context (5-10 lines before and after).
2. For **Category 1** (FFI): Verify the `Box::from_raw` ownership model is documented and the extension trait has clear allocation requirements.
3. For **Category 2** (signals): Verify PID validity — are there race conditions where the process could have exited and PID been recycled?
4. For **Category 3** (TUI): Understand the `app_state` type and why `as_mut()` requires `unsafe`. Determine if `DerefMut` on `MutexGuard` would eliminate the need.

### Phase 2: Add SAFETY comments

For every `unsafe` block, add a `// SAFETY:` comment:
```rust
// SAFETY: The raw pointer was created by `Box::new` in the extension's
// `oxi_extension_create` entry point. We take ownership via Box::from_raw,
// matching the original allocation. The null check above ensures the pointer
// is valid.
let boxed: Box<dyn Extension> = Box::from_raw(raw_ptr);
```

### Phase 3: Fix TUI pattern (if needed)

1. If the `unsafe` in overlay files is due to calling `as_mut()` on a smart pointer, investigate if a safe alternative exists:
   - Could `MutexGuard` deref_mut be used instead?
   - Is there a safe wrapper around this pattern?
2. If the `unsafe` is truly necessary, add a `SAFETY` comment explaining the invariant.

### Phase 4: Document extension safety contract

In `oxi-cli/src/extensions/loading.rs`, add a module-level doc comment explaining:
- The `Box<dyn Extension>` ownership transfer contract
- Memory safety requirements for extension implementors
- What happens if an extension violates the contract

### Phase 5: Verify

1. `cargo check --workspace` — compiles
2. `cargo nextest run --workspace` — 2131 tests pass
3. `rg "unsafe \{" --type rust | grep -v target/ | grep -v "SAFETY"` — every `unsafe` block has a nearby `SAFETY` comment

---

## Constraints

- **Do not** add `#![deny(unsafe_code)]` — extensions and signals need `unsafe`.
- **Do not** change the extension loading architecture.
- **Preserve** process cleanup behavior (kill on timeout).
- **Do not** wrap `unsafe` blocks in safe abstractions unless the abstraction is clearly better.

## Verification

1. `cargo nextest run --workspace` — 2131 tests pass
2. Every `unsafe {` in production code has a corresponding `// SAFETY:` comment within 3 lines above it.
3. The TUI overlay pattern is either confirmed safe with a comment, or refactored to a safe alternative.
