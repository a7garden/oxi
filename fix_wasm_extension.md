# WASM Extension Security Fixes

## Fix 1: Implement actual timeout for oxi_exec ✅

**Problem**: The `host_oxi_exec` function had a `timeout` field but only used it as informational metadata — commands would run indefinitely.

**Solution**: Implemented proper timeout enforcement using a polling loop with `try_wait()`:

1. Clamp timeout to 1-30 seconds (`req.timeout.max(1000).min(30000)`) to prevent abuse
2. Use `spawn()` instead of `output()` to get a child handle
3. Poll with `try_wait()` every 50ms, checking elapsed time
4. On timeout: call `child.kill()` then `child.wait()` to reap the zombie
5. Return `exit_code: -2` and `"timed_out": true` for killed processes

**Changes to `host_oxi_exec`**:
- Added `use std::io::Read as _` for reading stdout/stderr buffers
- Added `use std::time::{Duration, Instant}` for timing
- Changed from blocking `Command::output()` to polling `try_wait()` loop
- Process is killed and cleaned up on timeout
- Exit code -2 specifically indicates timeout (vs -1 for execution failure)

---

## Fix 2: Namespace the KV store ✅

**Problem**: Global `KV_STORE` had no namespace isolation between extensions. Extensions A and B could read/write each other's data.

**Solution**: Added thread-local extension identity tracking:

1. **Thread-local storage**: Added `CURRENT_EXTENSION` thread-local `RefCell<Option<String>>`
2. **Context management**: Set context before `plugin.call()` in:
   - `load()` — before `register_tools()` and `register_commands()`
   - `execute_tool()` — before `execute_tool` WASM call
   - `execute_command()` — before `execute_command` WASM call
3. **Namespaced operations**: KV host functions now prefix keys with `{extension}:` using:
   - `current_extension_name()` — reads from thread-local storage
   - `kv_namespaced_get(extension, key)` — looks up `ext:key`
   - `kv_namespaced_set(extension, key, value)` — stores `ext:key`

4. **Fallback**: Unknown extensions (before init completes) use `"__unknown__"` namespace

**Changes**:
- Added `CURRENT_EXTENSION` thread-local and helper functions
- Modified `host_oxi_kv_get` to use `kv_namespaced_get(&current_extension_name(), &req.key)`
- Modified `host_oxi_kv_set` to use `kv_namespaced_set(&current_extension_name(), &req.key, &req.value)`
- Wrapped plugin calls in context setup/cleanup in `load()`, `execute_tool()`, `execute_command()`

---

## Fix 3: Review unsafe Send/Sync impl ✅

**Problem**: `unsafe impl Send for WasmExtensionManager {}` and `unsafe impl Sync for WasmExtensionManager {}` claimed `extism::Plugin` is Send+Sync, but this was not verified.

**Solution**: Replaced unsafe impls with a Mutex wrapper and removed the unsafe code:

1. Changed `plugins: Arc<RwLock<HashMap<String, extism::Plugin>>>` to `Arc<Mutex<HashMap<String, extism::Plugin>>>`
   - Mutex provides exclusive locking which is safer for unknown Send+Sync bounds
2. Changed all `plugins.write()` calls to `plugins.lock()` 
3. Removed the `unsafe impl` blocks entirely
4. Updated documentation to reflect Mutex-based serialization

**Why Mutex over RwLock**: Since we're doing exclusive access anyway (need `get_mut()` for plugin calls), Mutex is simpler and provides stronger guarantees. The `parking_lot::Mutex` is faster than `std::sync::Mutex`.

---

## Test Considerations

To verify these fixes:
1. **Timeout**: Create a test extension that runs a long-running command (e.g., `sleep 60`) and verify:
   - It returns `timed_out: true` with `exit_code: -2`
   - The process is actually killed (check process list)
   
2. **KV Namespacing**: Load two extensions, have each store a key with the same name, verify they don't collide.

3. **Send/Sync**: Load multiple extensions and call their tools concurrently from multiple threads — no panics should occur.