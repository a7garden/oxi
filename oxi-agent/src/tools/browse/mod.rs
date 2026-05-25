//! Browser tools for web page rendering and data extraction.
//!
//! All tools share the same `BrowserEngine` abstraction. The actual backend
//! (oxibrowser-core) is behind `#[cfg(feature = "native-browser")]`.

// Always compiled — trait definitions, helpers, config
pub mod browse_extract_tool;
pub mod browse_tool;
pub mod config;
pub mod engine;
pub mod helpers;
pub mod tab_guard;

// Feature-gated: requires oxibrowser-core + serde_yaml
#[cfg(feature = "native-browser")]
pub mod browse_script_tool;
#[cfg(feature = "native-browser")]
pub mod oxibrowser_backend;

// Re-exports for convenience
pub use browse_extract_tool::BrowseExtractTool;
pub use browse_tool::BrowseTool;
pub use config::BrowseConfig;
pub use engine::{BrowserEngine, BrowserTab, BrowserError, PageContent, LinkInfo, ElementInfo};
pub use tab_guard::TabGuard;

#[cfg(feature = "native-browser")]
pub use browse_script_tool::BrowseScriptTool;
#[cfg(feature = "native-browser")]
pub use oxibrowser_backend::OxiBrowserEngine;
