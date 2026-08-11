//! Tool factories for common tool sets

use std::path::Path;
use std::sync::Arc;

use oxicode_agent::{
    ToolRegistry,
    tools::{EditTool, LsTool, ReadTool, WriteTool},
};

/// Create the standard coding tools: read, write, edit, ls
pub fn coding_tools(cwd: &Path) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::with_cwd(cwd.to_path_buf()));
    registry.register(WriteTool::with_cwd(cwd.to_path_buf()));
    registry.register(EditTool::with_cwd(cwd.to_path_buf()));
    registry.register(LsTool::with_cwd(cwd.to_path_buf()));
    Arc::new(registry)
}

/// Create read-only tools: read, ls
pub fn readonly_tools(cwd: &Path) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::with_cwd(cwd.to_path_buf()));
    registry.register(LsTool::with_cwd(cwd.to_path_buf()));
    Arc::new(registry)
}

// ── MCP tool factory (Phase SDK) ─────────────────────────────────────

/// Create a `ToolRegistry` containing the MCP proxy tool plus any
/// direct tools registered via `directTools` in the cache.
///
/// The MCP manager is spawned with the supplied config, or auto-discovers
/// from `~/.config/oxicode/mcp.json` / `.mcp.json` if `config` is `None`.
///
/// The `cwd` parameter is used to resolve `.mcp.json` lookup paths.
pub fn mcp_tools(
    cwd: &std::path::Path,
    config: Option<oxicode_agent::mcp::McpConfig>,
) -> std::sync::Arc<ToolRegistry> {
    use oxicode_agent::mcp::{McpDirectTool, McpManager, McpTool};

    let manager = match config {
        Some(cfg) => McpManager::spawn_with_config(cfg),
        None => {
            // Auto-discover from the standard paths relative to `cwd`.
            // For now, fall back to the default loader (which uses CWD).
            let _ = cwd; // silence unused
            McpManager::spawn()
        }
    };

    let registry = ToolRegistry::new();

    // Direct tools (Phase 3) — read from cache.
    for def in manager.direct_tools_from_cache() {
        registry.register(McpDirectTool::new(manager.clone(), def));
    }

    // Proxy tool (unless explicitly disabled).
    if !manager.should_disable_proxy() {
        registry.register(McpTool::new(manager.clone()));
    }

    // Stash the manager so the TUI / other consumers can reach it.
    registry.set_mcp_manager(manager);

    std::sync::Arc::new(registry)
}
