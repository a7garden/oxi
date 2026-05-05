//! Extension system for oxi
//!
//! Extensions allow custom tools, commands, and event hooks to be loaded
//! dynamically at runtime. Extensions can be loaded from shared libraries
//! (.so/.dll/.dylib) via the `-e`/`--extension` CLI flag.

pub mod context;
pub mod loading;
pub mod registry;
pub mod types;

// Re-export types from submodules for public API
pub use crate::extensions::types::{
    ExtensionManifest, ExtensionPermission, ExtensionError, ExtensionErrorRecord,
    ExtensionState, Command,
    SessionSwitchReason, SessionShutdownReason, ModelSelectSource, InputSource,
    InputEventResult,
    SessionBeforeSwitchEvent, SessionBeforeForkEvent, SessionBeforeCompactEvent,
    SessionCompactEvent, SessionShutdownEvent, SessionBeforeTreeEvent, SessionTreeEvent,
    ContextEvent, BeforeProviderRequestEvent, AfterProviderResponseEvent,
    ModelSelectEvent, ThinkingLevelSelectEvent, BashEvent, InputEvent,
    ToolCallEmitResult, ToolResultEmitResult, ContextEmitResult,
    ProviderRequestEmitResult, SessionBeforeEmitResult,
    ExtensionErrorListener,
};

pub use crate::extensions::context::{ExtensionContext, ExtensionContextBuilder};

pub use crate::extensions::registry::{
    ExtensionRegistry, ExtensionRunner, ExtensionErrorHandle,
};

pub use crate::extensions::loading::{
    load_extension, load_extensions, discover_extensions, discover_extensions_in_dir,
};

// Re-export common traits from oxi-agent
pub use oxi_agent::{AgentEvent, AgentTool, AgentToolResult};

// The Extension trait - re-exported from registry
pub use crate::extensions::registry::Extension;

// Built-in noop extension
pub struct NoopExtension;

impl Extension for NoopExtension {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "Built-in no-op extension"
    }
}