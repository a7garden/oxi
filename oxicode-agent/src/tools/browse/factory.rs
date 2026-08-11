//! Browser tool assembly factories.
//!
//! Pure assembly over [`BrowserEngine`] and the browse tool types — no backend
//! dependency of its own. Lives in the **agent layer** (not the SDK) because
//! browsing is a product capability, not an SDK contract: the SDK defines port
//! traits and lets products register their own tooling. Products that want the
//! built-in browsing surface call these factories; the SDK no longer re-exports
//! browser tooling.
//!
//! For the native `oxibrowser-core` backend, enable the `native-browser`
//! feature and construct [`OxicodeBrowserEngine`](super::OxicodeBrowserEngine).

use std::sync::Arc;

use crate::ToolRegistry;

use super::{BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine};

/// Create the core browser tools: `browse` + `browse_extract`.
///
/// Requires a [`BrowserEngine`] implementation. Use
/// [`browsing_tools_with_session`] when the `native-browser` feature is enabled
/// and you also want the persistent-session/script tools.
pub fn browsing_tools(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    registry.register(BrowseExtractTool::new(engine));
    Arc::new(registry)
}

/// Create the core browser tools with a custom [`BrowseConfig`].
pub fn browsing_tools_with_config(
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::with_config(Arc::clone(&engine), config.clone()));
    registry.register(BrowseExtractTool::with_config(engine, config));
    Arc::new(registry)
}

/// Create the full browser toolset including persistent session support.
///
/// Registers `browse`, `browse_extract`, `browse_script`, and `browse_session`.
/// Requires the `native-browser` feature — the session and script tools are
/// backed by `oxibrowser-core`.
#[cfg(feature = "native-browser")]
pub fn browsing_tools_with_session(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> {
    use super::{BrowseScriptTool, BrowseSessionTool};

    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    registry.register(BrowseExtractTool::new(Arc::clone(&engine)));
    registry.register(BrowseScriptTool::new(Arc::clone(&engine)));
    registry.register(BrowseSessionTool::new(engine));
    Arc::new(registry)
}

#[cfg(all(test, feature = "native-browser"))]
mod tests {
    use super::*;

    #[test]
    fn browsing_tools_with_session_registers_all_four() {
        // The factory must compile and produce a registry under native-browser.
        // We can't easily build a real engine in a unit test, but we can at
        // least assert the function is reachable (compiles) — the registration
        // count is exercised by the CLI integration smoke test.
        let _f: fn(Arc<dyn BrowserEngine>) -> Arc<ToolRegistry> = browsing_tools_with_session;
        let _ = _f;
    }
}
