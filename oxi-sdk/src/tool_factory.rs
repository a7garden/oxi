//! Tool factories for common tool sets

use std::path::Path;
use std::sync::Arc;

use oxi_agent::{
    ToolRegistry, AgentTool,
    tools::{ReadTool, WriteTool, EditTool, LsTool},
};

/// Create the standard coding tools: read, write, edit, ls
/// Note: These tools use the current working directory
pub fn coding_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    registry.register(WriteTool::new());
    registry.register(EditTool::new());
    registry.register(LsTool::new());
    Arc::new(registry)
}

/// Create read-only tools: read, ls
/// Note: These tools use the current working directory
pub fn readonly_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    registry.register(LsTool::new());
    Arc::new(registry)
}