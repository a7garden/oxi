//! Shared callback management for browser tools.
//!
//! All four browse tools (BrowseTool, BrowseSessionTool, BrowseExtractTool,
//! BrowseScriptTool) use the same pattern:
//!
//! 1. `AgentTool::on_progress` stores a `ProgressCallback`
//! 2. `AgentTool::on_browse_progress` stores a `BrowseProgressCallback`
//! 3. `execute` opens a tab and registers both callbacks
//!
//! `BrowseCallbacks` eliminates the per-tool boilerplate.

use super::engine::{BrowseProgressCallback, TabCallbackRegistry};
use crate::tools::ProgressCallback;
use parking_lot::Mutex;

/// Shared callback state for browser tools.
///
/// Each browse tool holds one of these. The agent loop calls
/// `store_progress` / `store_browse` before `execute`; the tool's
/// `execute` method calls `register_on_tab` or `register_on_registry`
/// to wire the callbacks to the actual tab.
pub(crate) struct BrowseCallbacks {
    progress: Mutex<Option<ProgressCallback>>,
    browse: Mutex<Option<BrowseProgressCallback>>,
}

impl BrowseCallbacks {
    /// Create with no callbacks.
    pub fn new() -> Self {
        Self {
            progress: Mutex::new(None),
            browse: Mutex::new(None),
        }
    }

    /// Store a string progress callback (from `on_progress`).
    pub fn store_progress(&self, cb: ProgressCallback) {
        *self.progress.lock() = Some(cb);
    }

    /// Store a structured browse progress callback (from `on_browse_progress`).
    pub fn store_browse(&self, cb: BrowseProgressCallback) {
        *self.browse.lock() = Some(cb);
    }

    /// Register both pending callbacks on the engine's `TabCallbackRegistry`.
    ///
    /// Used by tools that manage their own tab lifecycle via the registry
    /// (BrowseSessionTool, BrowseExtractTool, BrowseScriptTool).
    pub fn register_on_registry(&self, tab_id: uuid::Uuid, registry: &TabCallbackRegistry) {
        if let Some(cb) = self.progress.lock().take() {
            registry.set(tab_id, cb);
        }
        if let Some(bcb) = self.browse.lock().take() {
            registry.set_browse(tab_id, bcb);
        }
    }

    /// Register both pending callbacks on a tab via downcast.
    ///
    /// Used by `BrowseTool` which uses the `OxicodeTab` downcast path
    /// (tab-local callback storage).
    pub fn register_on_tab(&self, _tab: &dyn super::engine::BrowserTab) {
        #[cfg(feature = "native-browser")]
        {
            use super::oxibrowser_backend::OxicodeTab;
            if let Some(oxicode_tab) = _tab.as_any().downcast_ref::<OxicodeTab>() {
                if let Some(cb) = self.progress.lock().take() {
                    oxicode_tab.set_progress_callback(cb);
                }
                if let Some(bcb) = self.browse.lock().take() {
                    oxicode_tab.set_browse_progress_callback_impl(bcb);
                }
            }
        }
        #[cfg(not(feature = "native-browser"))]
        {
            // Drop callbacks — no backend to register on.
            self.progress.lock().take();
            self.browse.lock().take();
        }
    }

    /// Register browse callback on registry only, if pending.
    /// Used by `BrowseSessionTool::on_browse_progress` when tab is already open.
    pub fn register_browse_on_registry(&self, tab_id: uuid::Uuid, registry: &TabCallbackRegistry) {
        if let Some(bcb) = self.browse.lock().take() {
            registry.set_browse(tab_id, bcb);
        }
    }

    /// Register progress callback on registry only, if pending.
    /// Used by `BrowseSessionTool::on_progress` when tab is already open.
    pub fn register_progress_on_registry(
        &self,
        tab_id: uuid::Uuid,
        registry: &TabCallbackRegistry,
    ) {
        if let Some(cb) = self.progress.lock().take() {
            registry.set(tab_id, cb);
        }
    }

    /// Take the pending progress callback without registering.
    /// Used by `BrowseScriptTool` which needs the callback for step-level
    /// progress emission.
    #[cfg(feature = "native-browser")]
    pub fn take_progress(&self) -> Option<ProgressCallback> {
        self.progress.lock().take()
    }
}

impl Default for BrowseCallbacks {
    fn default() -> Self {
        Self::new()
    }
}
