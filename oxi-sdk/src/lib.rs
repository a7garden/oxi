//! oxi SDK - Programmatic API for building AI agents
//!
//! # Example
//! ```
//! use oxi_sdk::{OxiBuilder, AgentConfig};
//!
//! let oxi = OxiBuilder::new().with_builtins().build();
//! let agent = oxi.agent(AgentConfig { 
//!     model_id: "anthropic/claude-sonnet-4-20250514".into(), 
//!     max_iterations: 20,
//!     ..Default::default() 
//! }).build().unwrap();
//! ```

pub mod builder;
pub mod agent_builder;
pub mod tool_factory;
pub mod prelude;

// Re-export from oxi-ai
pub use oxi_ai::{
    Provider, Model, Context, Message, ContentBlock,
    ProviderEvent, StreamOptions, CompactionStrategy,
    ProviderError, Api, Cost, InputModality,
    ModelRegistry,
    lookup_model as oxi_lookup_model, get_models, get_providers,
    get_provider,
};

// Re-export from oxi-agent  
pub use oxi_agent::{
    Agent, AgentLoop, AgentLoopConfig, AgentConfig,
    AgentEvent, AgentState, SharedState,
    ToolRegistry, AgentTool, AgentToolResult, ToolError,
    AgentHooks, ToolExecutionMode, AgentError,
};