# Fix 2+3: File tools store and use cwd, coding_tools() accepts cwd

## Summary

All 7 file tools (ReadTool, WriteTool, EditTool, LsTool, GrepTool, FindTool, BashTool) now store a `root_dir: PathBuf` field and use it instead of `std::env::current_dir()`. Each tool has a `with_cwd(cwd: PathBuf)` constructor. The `coding_tools()` and `readonly_tools()` factory functions now accept a `cwd: &Path` parameter.

## Changes Made

### Tool Structs (oxi-agent/src/tools/)

**read.rs** — ReadTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `new()` delegates to `with_cwd(std::env::current_dir()...)`
- `execute()` uses `PathGuard::new(&self.root_dir)` instead of hardcoded current_dir

**write.rs** — WriteTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `write_file_impl()` now takes `root_dir: &Path` as first parameter
- `execute()` passes `&self.root_dir` to `write_file_impl()`
- All test calls updated to pass `Path::new(".")` as root_dir

**edit.rs** — EditTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `apply_edits()` now takes `root_dir: &Path` as first parameter
- `execute()` passes `&self.root_dir` to `apply_edits()`
- All test calls updated to pass `Path::new(".")` as root_dir

**ls.rs** — LsTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `ls_impl()` now takes `root_dir: &Path` as first parameter
- `execute()` passes `&self.root_dir` to `ls_impl()`
- All test calls updated to pass `Path::new(".")` as root_dir

**grep.rs** — GrepTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `grep_impl()` now takes `root_dir: &Path` as first parameter
- `execute()` passes `&self.root_dir` to `grep_impl()`

**find.rs** — FindTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `find_impl()` now takes `root_dir: &Path` as first parameter
- `execute()` passes `&self.root_dir` to `find_impl()`

**bash.rs** — BashTool:
- Added `root_dir: PathBuf` field
- Added `with_cwd(cwd: PathBuf)` constructor
- `run_command()` now takes `root_dir: &Path` as first parameter
- Uses `root_dir` as workspace for `validate_cwd()` (security constraint)
- Defaults to `root_dir` as working directory when no cwd param provided
- Test `test_working_directory` updated to use `BashTool::with_cwd(PathBuf::from("/tmp"))`

### ToolRegistry (oxi-agent/src/tools.rs)

- `with_builtins_cwd()` now passes `cwd.clone()` to all 7 tools via `Tool::with_cwd(cwd.clone())`

### Tool Factory (oxi-sdk/src/tool_factory.rs)

- `coding_tools(cwd: &Path)` — accepts cwd parameter, passes to each tool's `with_cwd()`
- `readonly_tools(cwd: &Path)` — accepts cwd parameter, passes to each tool's `with_cwd()`

### SDK Re-exports (oxi-sdk/src/lib.rs)

- Removed function re-exports: `lookup_model as oxi_lookup_model`, `get_models`, `get_providers`, `get_provider`
- Kept all type re-exports: `Provider`, `Model`, `Context`, `Message`, etc.

## Verification

```
cargo check --workspace --lib  — 0 errors, 0 warnings
cargo test --workspace --lib   — 1309 tests, 0 failures
```
