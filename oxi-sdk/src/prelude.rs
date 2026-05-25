//! Commonly used types
//!
//! Re-exports the most frequently used SDK types for convenient glob imports:
//!
//! ```ignore
//! use oxi_sdk::prelude::*;
//! ```

pub use crate::agent_builder::AgentBuilder;
pub use crate::builder::{Oxi, OxiBuilder};
pub use crate::multi_provider::{MultiProviderBuilder, RoutingConfig};
pub use crate::tool_factory::{browsing_tools, coding_tools, full_tools, readonly_tools};

#[cfg(feature = "native-browser")]
pub use crate::tool_factory::browsing_tools_with_session;

pub use oxi_agent::{
    Agent, AgentConfig, AgentEvent, AgentLoop, AgentState, AgentTool, AgentToolResult,
    CompactionEvent, SearchCache, SharedState, ToolError, ToolExecutionMode, ToolRegistry,
};

pub use oxi_agent::tools::browse::{
    BrowseConfig, BrowseExtractTool, BrowseTool, BrowserEngine, BrowserError, BrowserTab,
    ElementInfo, LinkInfo, PageContent, TabGuard,
};

pub use oxi_ai::circuit_breaker::CircuitBreakerConfig;
pub use oxi_ai::{CompactionStrategy, Model, Provider, UserMessage};
