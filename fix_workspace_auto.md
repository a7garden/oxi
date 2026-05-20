# Fix: workspace() automatically sets cwd on coding tools

## Problem
When SDK user calls `.workspace("/foo")`, they still need to manually pass the same path to `coding_tools(Path::new("/foo"))`. The two are disconnected.

## Changes Made

### `oxi-sdk/src/agent_builder.rs`

1. **Updated `workspace()` doc comment** — now documents that `.coding_tools()` and `.readonly_tools()` automatically use the workspace directory.

2. **Added `coding_tools()` method** — registers read, write, edit, ls tools using `self.workspace_dir` (falls back to current directory).

3. **Added `readonly_tools()` method** — registers read and ls tools using `self.workspace_dir` (falls back to current directory).

Both methods delegate to `crate::tool_factory::coding_tools()` / `crate::tool_factory::readonly_tools()`, then iterate the returned `Arc<ToolRegistry>` and register each tool into the builder's own `ToolRegistry` via `register_arc()`.

## Usage

```rust
let agent = oxi.agent(config)
    .workspace("/foo")
    .coding_tools()  // ← automatically uses /foo
    .build()?;
```

## Verification
- `cargo check --workspace --lib` — **0 errors**
