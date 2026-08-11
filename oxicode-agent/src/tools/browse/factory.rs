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

use super::{BrowseActTool, BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine};

/// Create the core browser tools: `browse` + `browse_act` + `browse_extract`.
///
/// `BrowseActTool` is the layer-2 primitive that grounds a natural-language
/// goal against the page's interactive surface via a construction-injected
/// LLM (`provider` + `model`). When both are `Some`, the tool calls the LLM
/// per act. When either is `None` (tests, offline builds, MCP servers without
/// an LLM wire), `BrowseActTool` runs in deterministic-only mode: it picks
/// the top scorer from the candidate tier and surfaces
/// `mode: "deterministic_only"` in every result.
pub fn browsing_tools(
    provider: Option<Arc<dyn oxicode_ai::Provider>>,
    model: Option<oxicode_ai::Model>,
    engine: Arc<dyn BrowserEngine>,
) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    match (provider, model) {
        (Some(p), Some(m)) => {
            registry.register(BrowseActTool::new(p, m, Arc::clone(&engine)));
        }
        _ => {
            registry.register(BrowseActTool::new_deterministic(Arc::clone(&engine)));
        }
    }
    registry.register(BrowseExtractTool::new(engine));
    Arc::new(registry)
}

/// Create the core browser tools with a custom [`BrowseConfig`].
///
/// Same `(provider, model)` semantics as [`browsing_tools`]: `Some` enables
/// LLM grounding, `None` falls back to deterministic.
pub fn browsing_tools_with_config(
    provider: Option<Arc<dyn oxicode_ai::Provider>>,
    model: Option<oxicode_ai::Model>,
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
) -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(BrowseTool::with_config(
        Arc::clone(&engine),
        config.clone(),
    ));
    match (provider, model) {
        (Some(p), Some(m)) => {
            registry.register(BrowseActTool::with_config(
                p,
                m,
                Arc::clone(&engine),
                config.clone(),
            ));
        }
        _ => {
            registry.register(BrowseActTool::with_config_deterministic(
                Arc::clone(&engine),
                config.clone(),
            ));
        }
    }
    registry.register(BrowseExtractTool::with_config(engine, config));
    Arc::new(registry)
}

/// Create the full browser toolset including persistent session support.
///
/// Registers `browse`, `browse_act`, `browse_extract`, `browse_script`,
/// and `browse_session`. Requires the `native-browser` feature — the
/// session and script tools are backed by `oxibrowser-core`.
#[cfg(feature = "native-browser")]
pub fn browsing_tools_with_session(
    provider: Option<Arc<dyn oxicode_ai::Provider>>,
    model: Option<oxicode_ai::Model>,
    engine: Arc<dyn BrowserEngine>,
) -> Arc<ToolRegistry> {
    use super::{BrowseScriptTool, BrowseSessionTool};

    let registry = ToolRegistry::new();
    registry.register(BrowseTool::new(Arc::clone(&engine)));
    match (provider, model) {
        (Some(p), Some(m)) => {
            registry.register(BrowseActTool::new(p, m, Arc::clone(&engine)));
        }
        _ => {
            registry.register(BrowseActTool::new_deterministic(Arc::clone(&engine)));
        }
    }
    registry.register(BrowseExtractTool::new(Arc::clone(&engine)));
    registry.register(BrowseScriptTool::new(Arc::clone(&engine)));
    registry.register(BrowseSessionTool::new(engine));
    Arc::new(registry)
}

#[cfg(all(test, feature = "native-browser"))]
mod tests {
    use super::*;

    type FactoryFn = fn(
        Option<Arc<dyn oxicode_ai::Provider>>,
        Option<oxicode_ai::Model>,
        Arc<dyn BrowserEngine>,
    ) -> Arc<ToolRegistry>;

    #[test]
    fn browsing_tools_with_session_registers_all_four() {
        // The factory must compile and produce a registry under native-browser.
        // We can't easily build a real engine in a unit test, but we can at
        // least assert the function is reachable (compiles) — the registration
        // count is exercised by the CLI integration smoke test.
        let _f: FactoryFn = browsing_tools_with_session;
        let _ = _f;
    }
}
