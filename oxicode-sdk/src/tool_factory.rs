//! Tool factories for common tool sets

use std::path::Path;
use std::sync::Arc;

use oxicode_agent::{
    ToolRegistry,
    tools::browse::{BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine},
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

/// Create browser tools: browse, browse_extract.
///
/// Requires a [`BrowserEngine`] implementation. Use `native_browser_tools()`
/// when the `native-browser` feature is enabled.
///
/// # Example
///
/// ```ignore
/// // Requires a BrowserEngine implementation
/// use oxicode_sdk::browsing_tools;
/// let engine: Arc<dyn BrowserEngine> = /* ... */;
/// let tools = browsing_tools(engine);
/// ```
pub fn browsing_tools(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    registry.register(BrowseExtractTool::new(engine));
    Arc::new(registry)
}

/// Create browser tools with custom configuration.
///
/// Like [`browsing_tools()`] but accepts a [`BrowseConfig`] for tuning
/// timeouts, cache, concurrency limits, etc.
pub fn browsing_tools_with_config(
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::with_config(Arc::clone(&engine), config.clone()));
    registry.register(BrowseExtractTool::with_config(engine, config));
    Arc::new(registry)
}

/// Create the full toolset: coding + browser + browse_extract.
///
/// Convenience function that combines [`coding_tools()`] and browser tools
/// into a single registry. The `cwd` is used for file tools, and the
/// `engine` is used for browser tools.
///
/// # Example
///
/// ```ignore
/// // Requires a BrowserEngine implementation
/// use oxicode_sdk::full_tools;
/// let engine: Arc<dyn BrowserEngine> = /* ... */;
/// let tools = full_tools(Path::new("/workspace"), engine);
/// ```
pub fn full_tools(cwd: &Path, engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    // Coding tools
    registry.register(ReadTool::with_cwd(cwd.to_path_buf()));
    registry.register(WriteTool::with_cwd(cwd.to_path_buf()));
    registry.register(EditTool::with_cwd(cwd.to_path_buf()));
    registry.register(LsTool::with_cwd(cwd.to_path_buf()));
    // Browser tools
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    registry.register(BrowseExtractTool::new(engine));
    Arc::new(registry)
}

/// Create browser tools using the native oxibrowser-core backend.
///
/// Only available when the `native-browser` feature is enabled on
/// `oxicode-agent`. Returns an error if the browser engine fails to initialize.
///
/// # Example
///
/// ```ignore
/// // Requires native-browser feature
/// let tools = native_browser_tools().expect("browser init failed");
/// ```
#[cfg(feature = "native-browser")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
pub async fn native_browser_tools() -> anyhow::Result<Arc<ToolRegistry>> {
    let engine = oxicode_agent::tools::browse::OxicodeBrowserEngine::new().await?;
    Ok(browsing_tools(Arc::new(engine)))
}

/// Create browser tools using the native backend with custom config.
#[cfg(feature = "native-browser")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
pub async fn native_browser_tools_with_config(
    config: BrowseConfig,
) -> anyhow::Result<Arc<ToolRegistry>> {
    let engine =
        oxicode_agent::tools::browse::OxicodeBrowserEngine::with_config(config.clone()).await?;
    Ok(browsing_tools_with_config(Arc::new(engine), config))
}

/// Create all browser tools including session support.
///
/// Registers `browse`, `browse_extract`, `browse_script`, and `browse_session`.
/// Only available when the `native-browser` feature is enabled.
#[cfg(feature = "native-browser")]
#[cfg_attr(docsrs, doc(cfg(feature = "native-browser")))]
pub fn browsing_tools_with_session(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    use oxicode_agent::tools::browse::{BrowseScriptTool, BrowseSessionTool};

    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    registry.register(BrowseExtractTool::new(Arc::clone(&engine)));
    registry.register(BrowseScriptTool::new(Arc::clone(&engine)));
    registry.register(BrowseSessionTool::new(engine));
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
