# Fix: PathGuard Integration & input.rs Panic Fix

## Summary

Two critical fixes applied:
1. **input.rs panic** — Removed `unimplemented!()` that would crash at runtime
2. **PathGuard integration** — Replaced ad-hoc `..` traversal checks with structured PathGuard validation across all 6 file tools

## Fix 1: input.rs `text_mut()` Panic

### Problem
`InputState::text_mut()` in `oxi-tui/src/widgets/input.rs` called `unimplemented!()`, which would panic at runtime if ever invoked. The method's return type (`&mut String`) was impossible to satisfy since `ratatui-textarea::TextArea` does not expose mutable text access.

### Solution
Removed the method entirely. No callers existed in the codebase. The alternatives (`set_text()`, `insert_str()`, `clear()`) already provide all needed text mutation.

### File Changed
- `oxi-tui/src/widgets/input.rs` — deleted `text_mut()` method

## Fix 2: PathGuard Applied to File Tools

### Problem
All 6 file tools (read, write, edit, ls, find, grep) had inline `..` component checks:
```rust
if path.components().any(|c| c.as_os_str() == "..") {
    return Err("Path traversal not allowed".to_string());
}
```
This was inconsistent, didn't canonicalize paths, and didn't leverage the existing `PathGuard` infrastructure.

### Solution

1. **Added `validate_traversal()` to PathGuard** (`path_security.rs`):
   - Checks `..` traversal (same as before)
   - Canonicalizes existing paths (new — resolves symlinks)
   - Does NOT enforce workspace boundary (tools need to access temp dirs, system files, etc.)
   - The existing `validate()` method with full workspace checking remains for sandboxed contexts

2. **Updated all 6 file tools** to use `PathGuard::validate_traversal()`:
   - Creates `PathGuard` from `std::env::current_dir()` 
   - Replaces inline traversal check with structured validation
   - Uses validated (canonicalized for existing paths) `PathBuf` for operations

### Files Changed
| File | Change |
|------|--------|
| `oxi-agent/src/tools/path_security.rs` | Added `validate_traversal()` method |
| `oxi-agent/src/tools/read.rs` | Added import, replaced `..` check with `PathGuard::validate_traversal()` |
| `oxi-agent/src/tools/write.rs` | Added import, replaced `..` check |
| `oxi-agent/src/tools/edit.rs` | Added import, replaced `..` check |
| `oxi-agent/src/tools/ls.rs` | Added import, replaced `..` check |
| `oxi-agent/src/tools/find.rs` | Added import, replaced `..` check |
| `oxi-agent/src/tools/grep.rs` | Added import, replaced `..` check |

### Pattern Applied
Before:
```rust
let path = Path::new(path_str);
if path.components().any(|c| c.as_os_str() == "..") {
    return Err("Path traversal not allowed".to_string());
}
```

After:
```rust
let guard = PathGuard::new(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
let validated = guard.validate_traversal(Path::new(path_str)).map_err(|e| e.to_string())?;
let path = validated.as_path(); // or &validated for PathBuf
```

## Design Decision: `validate_traversal` vs `validate`

The original `PathGuard::validate()` enforces workspace boundary (paths must be under the root). This is too strict for file tools that legitimately access:
- Temp directories (`/var/folders/...`)
- System files (`/etc/config`)
- User home directories (`~/...`)

`validate_traversal()` provides the same traversal protection and canonicalization without the workspace boundary restriction, matching the original behavior while adding the benefit of canonicalized paths for existing files.

## Test Results

All existing tests pass:
- `tools::read` — 17 passed ✅
- `tools::write` — 18 passed ✅
- `tools::edit` — 19 passed ✅
- `tools::ls` — 11 passed ✅
- `tools::find` — 6 passed ✅
- `tools::path_security` — 3 passed ✅
- `widgets::input` — 6 passed ✅

## Compilation
Zero errors across all modified packages (`oxi-tui`, `oxi-agent`).
